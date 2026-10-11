//! Streaming protocols: WebSocket and Server-Sent Events. Each connection runs
//! on its own thread and reports [`StreamEvent`]s through a [`StreamSession`].

use crate::cookies::CookieJar;
use crate::exec::{Client, PreparedRequest};
use crate::model::WorkspaceSettings;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, TryRecvError, channel};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Incoming,
    Outgoing,
    Info,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
pub struct StreamEvent {
    /// Milliseconds since the connection started.
    pub at_ms: f64,
    pub direction: Direction,
    /// `open`, `message`, `close`, an SSE event name, …
    pub label: String,
    pub data: String,
    /// SSE event id, if any.
    pub id: Option<String>,
}

pub(crate) enum Outgoing {
    Text(String),
    Close,
}

/// A live streaming connection.
pub struct StreamSession {
    rx: Receiver<StreamEvent>,
    tx: Option<Sender<Outgoing>>,
    cancel: Arc<AtomicBool>,
    /// Cleared when the connection no longer takes messages (gRPC calls that
    /// are not client-streaming).
    accepting: Arc<AtomicBool>,
    finished: bool,
}

impl StreamSession {
    pub(crate) fn from_parts(
        rx: Receiver<StreamEvent>,
        tx: Option<Sender<Outgoing>>,
        cancel: Arc<AtomicBool>,
        accepting: Arc<AtomicBool>,
    ) -> Self {
        Self {
            rx,
            tx,
            cancel,
            accepting,
            finished: false,
        }
    }

    /// Events received since the last call.
    pub fn poll(&mut self) -> Vec<StreamEvent> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(e) => out.push(e),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.finished = true;
                    break;
                }
            }
        }
        out
    }

    /// Waits up to `timeout` for the next event (tests, headless use).
    pub fn recv_timeout(&mut self, timeout: Duration) -> Option<StreamEvent> {
        match self.rx.recv_timeout(timeout) {
            Ok(e) => Some(e),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                self.finished = true;
                None
            }
            Err(_) => None,
        }
    }

    /// Whether the connection has ended and all events were read.
    pub fn is_finished(&self) -> bool {
        self.finished
    }

    /// Whether messages can be sent (WebSocket only).
    pub fn can_send(&self) -> bool {
        self.tx.is_some() && !self.finished && self.accepting.load(Ordering::Relaxed)
    }

    pub fn send_text(&self, text: impl Into<String>) -> bool {
        self.tx
            .as_ref()
            .is_some_and(|tx| tx.send(Outgoing::Text(text.into())).is_ok())
    }

    /// Half-closes the request side of a gRPC client-stream; the call keeps
    /// running until the server answers.
    pub fn close_requests(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Outgoing::Close);
        }
    }

    /// Closes the connection gracefully (WebSocket) or stops reading (SSE).
    pub fn close(&self) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(Outgoing::Close);
        }
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for StreamSession {
    fn drop(&mut self) {
        self.close();
    }
}

pub(crate) struct Emitter {
    pub(crate) tx: Sender<StreamEvent>,
    pub(crate) started: Instant,
}

impl Emitter {
    pub(crate) fn emit(
        &self,
        direction: Direction,
        label: &str,
        data: impl Into<String>,
        id: Option<String>,
    ) -> bool {
        self.tx
            .send(StreamEvent {
                at_ms: self.started.elapsed().as_secs_f64() * 1000.0,
                direction,
                label: label.to_owned(),
                data: data.into(),
                id,
            })
            .is_ok()
    }
}

// ---------------------------------------------------------------------------
// Server-Sent Events

/// One dispatched SSE message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseMessage {
    pub event: String,
    pub data: String,
    pub id: Option<String>,
}

/// Incremental parser following the HTML "event stream interpretation" rules.
#[derive(Debug, Default)]
pub struct SseParser {
    event: String,
    data: Vec<String>,
    id: Option<String>,
    /// Last `retry:` value in milliseconds.
    pub retry_ms: Option<u64>,
}

impl SseParser {
    /// Feeds one line (without the line terminator).
    pub fn feed(&mut self, line: &str) -> Option<SseMessage> {
        if line.is_empty() {
            if self.data.is_empty() {
                self.event.clear();
                return None;
            }
            let message = SseMessage {
                event: if self.event.is_empty() {
                    "message".into()
                } else {
                    std::mem::take(&mut self.event)
                },
                data: std::mem::take(&mut self.data).join("\n"),
                id: self.id.clone(),
            };
            self.event.clear();
            return Some(message);
        }
        if line.starts_with(':') {
            return None;
        }
        let (field, value) = match line.split_once(':') {
            Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
            None => (line, ""),
        };
        match field {
            "event" => self.event = value.to_owned(),
            "data" => self.data.push(value.to_owned()),
            "id" if !value.contains('\0') => self.id = Some(value.to_owned()),
            "retry" => self.retry_ms = value.parse().ok().or(self.retry_ms),
            _ => {}
        }
        None
    }
}

/// Opens a Server-Sent Events stream.
pub fn connect_sse(
    mut prepared: PreparedRequest,
    settings: WorkspaceSettings,
    jar: Option<CookieJar>,
) -> StreamSession {
    if !prepared
        .headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("accept"))
    {
        prepared
            .headers
            .push(("Accept".into(), "text/event-stream".into()));
    }
    prepared
        .headers
        .push(("Cache-Control".into(), "no-cache".into()));
    let (tx, rx) = channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let out = Emitter {
            tx,
            started: Instant::now(),
        };
        let mut client = Client::new(settings);
        client.use_system_proxy = true;
        out.emit(
            Direction::Info,
            "connect",
            format!("GET {}", prepared.url),
            None,
        );
        let response = match client.open_stream(&prepared, jar.as_ref()) {
            Ok(r) => r,
            Err(e) => {
                out.emit(Direction::Error, "error", e.to_string(), None);
                return;
            }
        };
        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_owned();
        out.emit(
            Direction::Info,
            "open",
            format!(
                "{} {} · {content_type}",
                status.as_u16(),
                status.canonical_reason().unwrap_or("")
            ),
            None,
        );
        if !status.is_success() {
            let mut body = String::new();
            let _ = response.take(64 * 1024).read_to_string(&mut body);
            out.emit(
                Direction::Error,
                "error",
                format!("server answered {status}: {body}"),
                None,
            );
            return;
        }
        if !content_type.contains("text/event-stream") {
            out.emit(
                Direction::Info,
                "warning",
                "the server did not answer with text/event-stream; reading lines anyway",
                None,
            );
        }
        let mut parser = SseParser::default();
        let mut reader = BufReader::new(response);
        let mut line = String::new();
        loop {
            if flag.load(Ordering::Relaxed) {
                out.emit(Direction::Info, "close", "closed by client", None);
                return;
            }
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => {
                    if let Some(m) = parser.feed("") {
                        out.emit(Direction::Incoming, &m.event, m.data, m.id);
                    }
                    out.emit(Direction::Info, "close", "stream ended", None);
                    return;
                }
                Ok(_) => {
                    let trimmed = line.trim_end_matches(['\r', '\n']);
                    if let Some(m) = parser.feed(trimmed)
                        && !out.emit(Direction::Incoming, &m.event, m.data, m.id)
                    {
                        return;
                    }
                }
                Err(e) => {
                    out.emit(
                        Direction::Error,
                        "error",
                        format!("stream read failed: {e}"),
                        None,
                    );
                    return;
                }
            }
        }
    });
    StreamSession {
        rx,
        tx: None,
        cancel,
        accepting: Arc::new(AtomicBool::new(true)),
        finished: false,
    }
}

// ---------------------------------------------------------------------------
// WebSocket

enum Transport {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Transport {
    fn tcp(&self) -> &TcpStream {
        match self {
            Transport::Plain(s) => s,
            Transport::Tls(s) => &s.sock,
        }
    }
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Transport::Plain(s) => s.read(buf),
            Transport::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Transport::Plain(s) => s.write(buf),
            Transport::Tls(s) => s.write(buf),
        }
    }

    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Transport::Plain(s) => s.flush(),
            Transport::Tls(s) => s.flush(),
        }
    }
}

fn tls_config(verify: bool) -> Result<Arc<rustls::ClientConfig>, String> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let builder = rustls::ClientConfig::builder_with_provider(provider.clone())
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?;
    let config = if verify {
        let mut roots = rustls::RootCertStore::empty();
        for cert in rustls_native_certs::load_native_certs().certs {
            let _ = roots.add(cert);
        }
        builder.with_root_certificates(roots).with_no_client_auth()
    } else {
        builder
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(NoVerify(provider)))
            .with_no_client_auth()
    };
    Ok(Arc::new(config))
}

/// Accepts any certificate; used only when the user turned TLS verification off.
#[derive(Debug)]
struct NoVerify(Arc<rustls::crypto::CryptoProvider>);

impl rustls::client::danger::ServerCertVerifier for NoVerify {
    fn verify_server_cert(
        &self,
        _: &rustls::pki_types::CertificateDer<'_>,
        _: &[rustls::pki_types::CertificateDer<'_>],
        _: &rustls::pki_types::ServerName<'_>,
        _: &[u8],
        _: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.0.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

fn open_transport(url: &url::Url, verify: bool, timeout: Duration) -> Result<Transport, String> {
    let host = url.host_str().ok_or("the URL has no host")?.to_owned();
    let port = url.port_or_known_default().unwrap_or(80);
    let addr = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {host}: {e}"))?
        .next()
        .ok_or_else(|| format!("{host} has no address"))?;
    let tcp = TcpStream::connect_timeout(&addr, timeout)
        .map_err(|e| format!("could not connect to {host}:{port}: {e}"))?;
    let _ = tcp.set_nodelay(true);
    if url.scheme() != "wss" {
        return Ok(Transport::Plain(tcp));
    }
    let name = rustls::pki_types::ServerName::try_from(host.clone())
        .map_err(|e| format!("invalid TLS server name {host}: {e}"))?;
    let conn =
        rustls::ClientConnection::new(tls_config(verify)?, name).map_err(|e| e.to_string())?;
    Ok(Transport::Tls(Box::new(rustls::StreamOwned::new(
        conn, tcp,
    ))))
}

/// Headers the WebSocket handshake owns; user values for them are ignored.
const HANDSHAKE_HEADERS: [&str; 6] = [
    "host",
    "connection",
    "upgrade",
    "sec-websocket-key",
    "sec-websocket-version",
    "content-length",
];

/// Opens a WebSocket connection.
pub fn connect_websocket(prepared: PreparedRequest, jar: Option<CookieJar>) -> StreamSession {
    let (tx, rx) = channel();
    let (out_tx, out_rx) = channel::<Outgoing>();
    let cancel = Arc::new(AtomicBool::new(false));
    let flag = cancel.clone();
    std::thread::spawn(move || {
        let out = Emitter {
            tx,
            started: Instant::now(),
        };
        if let Err(e) = run_websocket(&prepared, jar.as_ref(), &out, &out_rx, &flag) {
            out.emit(Direction::Error, "error", e, None);
        }
    });
    StreamSession {
        rx,
        tx: Some(out_tx),
        cancel,
        accepting: Arc::new(AtomicBool::new(true)),
        finished: false,
    }
}

fn run_websocket(
    prepared: &PreparedRequest,
    jar: Option<&CookieJar>,
    out: &Emitter,
    outgoing: &Receiver<Outgoing>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    use tungstenite::client::IntoClientRequest;
    use tungstenite::{Error, Message};

    let url = &prepared.url;
    out.emit(
        Direction::Info,
        "connect",
        format!("Connecting to {url}"),
        None,
    );
    let timeout = Duration::from_millis(prepared.transport.timeout_ms.max(1000));
    let transport = open_transport(url, prepared.transport.verify_tls, timeout)?;
    let _ = transport.tcp().set_read_timeout(Some(timeout));
    let mut request = url
        .as_str()
        .into_client_request()
        .map_err(|e| e.to_string())?;
    for (name, value) in &prepared.headers {
        if HANDSHAKE_HEADERS.contains(&name.to_ascii_lowercase().as_str()) {
            continue;
        }
        if let (Ok(n), Ok(v)) = (
            tungstenite::http::HeaderName::from_bytes(name.as_bytes()),
            tungstenite::http::HeaderValue::from_str(value),
        ) {
            request.headers_mut().append(n, v);
        }
    }
    if prepared.transport.use_cookies
        && let Some(cookie) = jar.and_then(|j| j.header_for(url))
        && let Ok(v) = tungstenite::http::HeaderValue::from_str(&cookie)
    {
        request.headers_mut().append("cookie", v);
    }
    let (mut socket, response) =
        tungstenite::client::client(request, transport).map_err(|e| match e {
            tungstenite::HandshakeError::Failure(Error::Http(r)) => {
                let body = r
                    .body()
                    .as_ref()
                    .map(|b| String::from_utf8_lossy(b).into_owned())
                    .unwrap_or_default();
                format!("handshake rejected with {}: {body}", r.status())
            }
            other => format!("WebSocket handshake failed: {other}"),
        })?;
    let protocol = response
        .headers()
        .get("sec-websocket-protocol")
        .and_then(|v| v.to_str().ok())
        .map(|p| format!(" · protocol {p}"))
        .unwrap_or_default();
    out.emit(
        Direction::Info,
        "open",
        format!(
            "{} Switching Protocols{protocol}",
            response.status().as_u16()
        ),
        None,
    );
    let _ = socket
        .get_ref()
        .tcp()
        .set_read_timeout(Some(Duration::from_millis(40)));

    loop {
        if cancel.load(Ordering::Relaxed) {
            let _ = socket.close(None);
            let _ = socket.flush();
            out.emit(Direction::Info, "close", "closed by client", None);
            return Ok(());
        }
        loop {
            match outgoing.try_recv() {
                Ok(Outgoing::Text(text)) => {
                    socket
                        .send(Message::text(text.clone()))
                        .map_err(|e| format!("send failed: {e}"))?;
                    out.emit(Direction::Outgoing, "message", text, None);
                }
                Ok(Outgoing::Close) | Err(TryRecvError::Disconnected) => {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    out.emit(Direction::Info, "close", "closed by client", None);
                    return Ok(());
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        match socket.read() {
            Ok(Message::Text(t)) => {
                out.emit(Direction::Incoming, "message", t.as_str(), None);
            }
            Ok(Message::Binary(b)) => {
                out.emit(
                    Direction::Incoming,
                    "binary",
                    format!("<{} bytes of binary data>", b.len()),
                    None,
                );
            }
            Ok(Message::Ping(_)) | Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
            Ok(Message::Close(frame)) => {
                let reason = frame
                    .map(|f| format!("{} {}", u16::from(f.code), f.reason))
                    .unwrap_or_else(|| "no reason".into());
                out.emit(
                    Direction::Info,
                    "close",
                    format!("closed by server: {}", reason.trim()),
                    None,
                );
                let _ = socket.flush();
                return Ok(());
            }
            Err(Error::Io(e))
                if matches!(
                    e.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) => {}
            Err(Error::ConnectionClosed) | Err(Error::AlreadyClosed) => {
                out.emit(Direction::Info, "close", "connection closed", None);
                return Ok(());
            }
            Err(e) => return Err(format!("connection error: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_event_stream_per_spec() {
        let mut p = SseParser::default();
        let mut out = Vec::new();
        for line in [
            ": comment",
            "event: greet",
            "data: hello",
            "data:  world",
            "id: 7",
            "",
            "data",
            "",
            "retry: 3000",
            "event: x",
            "",
        ] {
            out.extend(p.feed(line));
        }
        assert_eq!(
            out,
            vec![
                SseMessage {
                    event: "greet".into(),
                    data: "hello\n world".into(),
                    id: Some("7".into())
                },
                SseMessage {
                    event: "message".into(),
                    data: "".into(),
                    id: Some("7".into())
                },
            ]
        );
        assert_eq!(p.retry_ms, Some(3000));
    }

    #[test]
    fn builds_tls_configs() {
        assert!(tls_config(true).is_ok());
        assert!(tls_config(false).is_ok());
    }
}
