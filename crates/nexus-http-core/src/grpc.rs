//! gRPC support: schema from a `.proto` file or server reflection, dynamic
//! JSON ⇄ protobuf conversion, and unary / server-, client- and
//! bidirectional-streaming calls reported as a [`StreamSession`].

use crate::exec::PreparedRequest;
use crate::model::WorkspaceSettings;
use crate::stream::{Direction, Emitter, Outgoing, StreamSession};
use prost::Message as _;
use prost_reflect::{DescriptorPool, DynamicMessage, MessageDescriptor, MethodDescriptor};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};
use tonic::Status;
use tonic::codec::{Codec, DecodeBuf, Decoder, EncodeBuf, Encoder};
use tonic::metadata::{AsciiMetadataKey, AsciiMetadataValue, MetadataMap};
use tonic::transport::{Channel, ClientTlsConfig, Endpoint};

/// Errors that prevent a call from starting.
#[derive(Debug, thiserror::Error)]
pub enum GrpcError {
    #[error("cannot read the .proto schema: {0}")]
    Proto(String),
    #[error("server reflection failed: {0}")]
    Reflection(String),
    #[error("method `{0}` not found; use the form package.Service/Method")]
    UnknownMethod(String),
    #[error("invalid request message: {0}")]
    Message(String),
    #[error("invalid metadata `{name}`: {reason}")]
    Metadata { name: String, reason: String },
    #[error("cannot connect to {target}: {reason}")]
    Connect { target: String, reason: String },
}

/// A callable RPC found in a schema.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MethodInfo {
    /// `package.Service/Method`.
    pub name: String,
    pub input: String,
    pub output: String,
    pub client_streaming: bool,
    pub server_streaming: bool,
    /// JSON skeleton of the request message.
    pub template: String,
}

impl MethodInfo {
    pub fn kind(&self) -> &'static str {
        match (self.client_streaming, self.server_streaming) {
            (false, false) => "unary",
            (false, true) => "server streaming",
            (true, false) => "client streaming",
            (true, true) => "bidirectional streaming",
        }
    }
}

/// Compiles `proto_file` (resolving imports next to it and in `includes`).
pub fn load_proto(proto_file: &Path, includes: &[&Path]) -> Result<DescriptorPool, GrpcError> {
    let dir = proto_file.parent().unwrap_or_else(|| Path::new("."));
    let mut roots: Vec<&Path> = vec![dir];
    roots.extend(includes);
    let file_name = proto_file
        .file_name()
        .ok_or_else(|| GrpcError::Proto(format!("`{}` is not a file", proto_file.display())))?;
    let fds = protox::compile([file_name], roots).map_err(|e| GrpcError::Proto(e.to_string()))?;
    DescriptorPool::from_file_descriptor_set(fds).map_err(|e| GrpcError::Proto(e.to_string()))
}

/// All methods of all services in `pool`, sorted by name.
pub fn list_methods(pool: &DescriptorPool) -> Vec<MethodInfo> {
    let mut out: Vec<MethodInfo> = pool
        .services()
        .flat_map(|s| s.methods().collect::<Vec<_>>())
        .map(|m| method_info(&m))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn method_info(m: &MethodDescriptor) -> MethodInfo {
    MethodInfo {
        name: format!("{}/{}", m.parent_service().full_name(), m.name()),
        input: m.input().full_name().to_owned(),
        output: m.output().full_name().to_owned(),
        client_streaming: m.is_client_streaming(),
        server_streaming: m.is_server_streaming(),
        template: request_template(m),
    }
}

/// Finds `package.Service/Method` (a leading `/` is accepted).
pub fn find_method(pool: &DescriptorPool, name: &str) -> Result<MethodDescriptor, GrpcError> {
    let name = name.trim().trim_start_matches('/');
    let (service, method) = name
        .split_once('/')
        .ok_or_else(|| GrpcError::UnknownMethod(name.to_owned()))?;
    pool.get_service_by_name(service)
        .and_then(|s| s.methods().find(|m| m.name() == method))
        .ok_or_else(|| GrpcError::UnknownMethod(name.to_owned()))
}

/// A JSON skeleton of the request message with every field at its default,
/// used as the starting point of the message editor.
pub fn request_template(method: &MethodDescriptor) -> String {
    let message = DynamicMessage::new(method.input());
    let options = prost_reflect::SerializeOptions::new()
        .skip_default_fields(false)
        .stringify_64_bit_integers(false);
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(
        &mut out,
        serde_json::ser::PrettyFormatter::with_indent(b"  "),
    );
    if message
        .serialize_with_options(&mut serializer, &options)
        .is_err()
    {
        return "{}".into();
    }
    String::from_utf8(out).unwrap_or_else(|_| "{}".into())
}

/// Parses the message editor text: one JSON object, or a JSON array / several
/// concatenated objects for streaming calls.
pub fn parse_messages(
    input: &MessageDescriptor,
    text: &str,
) -> Result<Vec<DynamicMessage>, GrpcError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(Vec::new());
    }
    let mut values = Vec::new();
    for value in serde_json::Deserializer::from_str(text).into_iter::<serde_json::Value>() {
        match value.map_err(|e| GrpcError::Message(e.to_string()))? {
            serde_json::Value::Array(items) => values.extend(items),
            other => values.push(other),
        }
    }
    values
        .into_iter()
        .enumerate()
        .map(|(i, v)| {
            DynamicMessage::deserialize(input.clone(), v)
                .map_err(|e| GrpcError::Message(format!("message {}: {e}", i + 1)))
        })
        .collect()
}

fn message_json(message: &DynamicMessage) -> String {
    let options = prost_reflect::SerializeOptions::new().skip_default_fields(false);
    let mut out = Vec::new();
    let mut serializer = serde_json::Serializer::with_formatter(
        &mut out,
        serde_json::ser::PrettyFormatter::with_indent(b"  "),
    );
    match message.serialize_with_options(&mut serializer, &options) {
        Ok(()) => String::from_utf8(out).unwrap_or_default(),
        Err(e) => format!("<cannot render message: {e}>"),
    }
}

// ---------------------------------------------------------------------------
// Transport

/// Builds the call for `prepared` (URL authority and headers become target and
/// metadata) with the schema named by `proto_file` (reflection when empty).
pub fn call_for(
    prepared: &PreparedRequest,
    settings: &WorkspaceSettings,
    proto_file: &str,
    base_dir: &Path,
    method: &str,
    body: &str,
) -> Call {
    Call {
        target: Target::from_prepared(prepared, settings),
        schema: schema_for(proto_file, base_dir),
        method: method.to_owned(),
        body: body.to_owned(),
    }
}

/// `.proto` path (relative to `base_dir`) or reflection when empty.
pub fn schema_for(proto_file: &str, base_dir: &Path) -> Schema {
    let proto_file = proto_file.trim();
    if proto_file.is_empty() {
        Schema::Reflection
    } else {
        Schema::ProtoFile(base_dir.join(proto_file))
    }
}

/// Loads the schema and lists its methods; the blocking call behind the
/// "Load schema" button.
pub fn load_methods(schema: &Schema, target: &Target) -> Result<Vec<MethodInfo>, GrpcError> {
    let methods = list_methods(&schema.load(target)?);
    if methods.is_empty() {
        return Err(GrpcError::Proto("the schema defines no services".into()));
    }
    Ok(methods)
}

/// Where and how to connect.
#[derive(Debug, Clone)]
pub struct Target {
    /// `http://host:port` or `https://host:port`.
    pub uri: String,
    pub metadata: Vec<(String, String)>,
    pub timeout: Option<Duration>,
    pub verify_tls: bool,
    pub ca_pem: Option<String>,
}

impl Target {
    /// Builds the target from a prepared request: the URL authority is used,
    /// headers become metadata (HTTP bookkeeping headers are dropped).
    pub fn from_prepared(p: &PreparedRequest, settings: &WorkspaceSettings) -> Self {
        let url = &p.url;
        let mut uri = format!("{}://{}", url.scheme(), url.host_str().unwrap_or(""));
        if let Some(port) = url.port() {
            uri.push_str(&format!(":{port}"));
        }
        let metadata = p
            .headers
            .iter()
            .filter(|(n, _)| {
                !["content-type", "content-length", "host", "te", "user-agent"]
                    .contains(&n.to_ascii_lowercase().as_str())
            })
            .cloned()
            .collect();
        Self {
            uri,
            metadata,
            timeout: (p.transport.timeout_ms > 0)
                .then(|| Duration::from_millis(p.transport.timeout_ms)),
            verify_tls: p.transport.verify_tls,
            ca_pem: settings.ca_certificate.clone(),
        }
    }
}

fn metadata_map(pairs: &[(String, String)]) -> Result<MetadataMap, GrpcError> {
    let mut map = MetadataMap::new();
    for (name, value) in pairs {
        let bad = |reason: String| GrpcError::Metadata {
            name: name.clone(),
            reason,
        };
        let key = AsciiMetadataKey::from_bytes(name.to_ascii_lowercase().as_bytes())
            .map_err(|e| bad(e.to_string()))?;
        let value: AsciiMetadataValue = value.parse().map_err(|_| {
            bad("values must be printable ASCII (use a `-bin` key for binary data)".into())
        })?;
        map.append(key, value);
    }
    Ok(map)
}

async fn connect(target: &Target) -> Result<Channel, GrpcError> {
    let fail = |reason: String| GrpcError::Connect {
        target: target.uri.clone(),
        reason,
    };
    let mut endpoint =
        Endpoint::from_shared(target.uri.clone()).map_err(|e| fail(e.to_string()))?;
    endpoint = endpoint.connect_timeout(target.timeout.unwrap_or(Duration::from_secs(30)));
    if target.uri.starts_with("https://") {
        let mut tls = ClientTlsConfig::new().with_native_roots();
        if let Some(pem) = &target.ca_pem {
            tls = tls.ca_certificate(tonic::transport::Certificate::from_pem(pem));
        }
        endpoint = endpoint.tls_config(tls).map_err(|e| fail(e.to_string()))?;
    }
    endpoint.connect().await.map_err(|e| {
        let mut reason = e.to_string();
        let mut source = std::error::Error::source(&e);
        while let Some(s) = source {
            reason = format!("{reason}: {s}");
            source = s.source();
        }
        fail(reason)
    })
}

fn runtime() -> Result<tokio::runtime::Runtime, GrpcError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| GrpcError::Connect {
            target: String::new(),
            reason: e.to_string(),
        })
}

// ---------------------------------------------------------------------------
// Reflection

/// Loads the schema from the server's reflection service (v1, falling back to
/// v1alpha) by listing every service and fetching its file descriptors.
pub fn fetch_reflection(target: &Target) -> Result<DescriptorPool, GrpcError> {
    runtime()?.block_on(async {
        let channel = connect(target).await?;
        match reflect(channel.clone(), target, true).await {
            Ok(pool) => Ok(pool),
            Err(_) => reflect(channel, target, false).await,
        }
    })
}

async fn reflect(channel: Channel, target: &Target, v1: bool) -> Result<DescriptorPool, GrpcError> {
    use tokio_stream::StreamExt as _;
    use tonic_reflection::pb::{v1 as r1, v1alpha as ra};
    let fail = |s: Status| GrpcError::Reflection(format!("{}: {}", s.code(), s.message()));
    let metadata = metadata_map(&target.metadata)?;

    // The two protocol versions carry identical payloads in different types, so a
    // small macro runs the same exchange against either client.
    macro_rules! exchange {
        ($m:ident, $client:ident) => {{
            use $m::server_reflection_request::MessageRequest;
            use $m::server_reflection_response::MessageResponse;
            let mut client = $m::server_reflection_client::ServerReflectionClient::new(channel);
            let mk = |req: MessageRequest| $m::ServerReflectionRequest {
                host: String::new(),
                message_request: Some(req),
            };
            let call = |reqs: Vec<MessageRequest>| {
                let mut request = tonic::Request::new(tokio_stream::iter(
                    reqs.into_iter().map(mk).collect::<Vec<_>>(),
                ));
                *request.metadata_mut() = metadata.clone();
                request
            };
            let mut list = client
                .server_reflection_info(call(vec![MessageRequest::ListServices(String::new())]))
                .await
                .map_err(fail)?
                .into_inner();
            let mut services = Vec::new();
            while let Some(r) = list.next().await {
                if let Some(MessageResponse::ListServicesResponse(l)) =
                    r.map_err(fail)?.message_response
                {
                    services.extend(l.service.into_iter().map(|s| s.name));
                }
            }
            let mut files = Vec::new();
            let mut seen = std::collections::HashSet::new();
            for service in services
                .into_iter()
                .filter(|s| !s.starts_with("grpc.reflection."))
            {
                let mut stream = client
                    .server_reflection_info(call(vec![MessageRequest::FileContainingSymbol(
                        service,
                    )]))
                    .await
                    .map_err(fail)?
                    .into_inner();
                while let Some(r) = stream.next().await {
                    match r.map_err(fail)?.message_response {
                        Some(MessageResponse::FileDescriptorResponse(f)) => {
                            for bytes in f.file_descriptor_proto {
                                let file = prost_types_file(&bytes)?;
                                if seen.insert(file.name().to_owned()) {
                                    files.push(file);
                                }
                            }
                        }
                        Some(MessageResponse::ErrorResponse(e)) => {
                            return Err(GrpcError::Reflection(e.error_message));
                        }
                        _ => {}
                    }
                }
            }
            let mut pool = DescriptorPool::new();
            pool.add_file_descriptor_protos(files)
                .map_err(|e| GrpcError::Reflection(e.to_string()))?;
            pool
        }};
    }
    let pool = if v1 {
        exchange!(r1, client)
    } else {
        exchange!(ra, client)
    };
    Ok(pool)
}

fn prost_types_file(
    bytes: &[u8],
) -> Result<prost_reflect::prost_types::FileDescriptorProto, GrpcError> {
    prost_reflect::prost_types::FileDescriptorProto::decode(bytes)
        .map_err(|e| GrpcError::Reflection(e.to_string()))
}

// ---------------------------------------------------------------------------
// Dynamic codec

#[derive(Clone)]
struct DynamicCodec {
    /// Type of the messages this side receives.
    decode: MessageDescriptor,
}

struct DynEncoder;
struct DynDecoder(MessageDescriptor);

impl Codec for DynamicCodec {
    type Encode = DynamicMessage;
    type Decode = DynamicMessage;
    type Encoder = DynEncoder;
    type Decoder = DynDecoder;
    fn encoder(&mut self) -> DynEncoder {
        DynEncoder
    }
    fn decoder(&mut self) -> DynDecoder {
        DynDecoder(self.decode.clone())
    }
}

impl Encoder for DynEncoder {
    type Item = DynamicMessage;
    type Error = Status;
    fn encode(&mut self, item: DynamicMessage, dst: &mut EncodeBuf<'_>) -> Result<(), Status> {
        item.encode(dst)
            .map_err(|e| Status::internal(format!("encode: {e}")))
    }
}

impl Decoder for DynDecoder {
    type Item = DynamicMessage;
    type Error = Status;
    fn decode(&mut self, src: &mut DecodeBuf<'_>) -> Result<Option<DynamicMessage>, Status> {
        DynamicMessage::decode(self.0.clone(), src)
            .map(Some)
            .map_err(|e| Status::internal(format!("decode: {e}")))
    }
}

// ---------------------------------------------------------------------------
// Calls

/// Where the schema comes from.
#[derive(Clone)]
pub enum Schema {
    /// An already loaded schema.
    Pool(DescriptorPool),
    /// A `.proto` file on disk.
    ProtoFile(std::path::PathBuf),
    /// The server's reflection service.
    Reflection,
}

impl Schema {
    /// Loads the schema; reflection uses `target` for the connection.
    pub fn load(&self, target: &Target) -> Result<DescriptorPool, GrpcError> {
        match self {
            Schema::Pool(pool) => Ok(pool.clone()),
            Schema::ProtoFile(path) => load_proto(path, &[]),
            Schema::Reflection => fetch_reflection(target),
        }
    }
}

/// Everything needed to run one call.
pub struct Call {
    pub target: Target,
    pub schema: Schema,
    /// `package.Service/Method`.
    pub method: String,
    /// The message editor text: one JSON object, or several for streaming calls.
    pub body: String,
}

/// Starts `call` on a background thread: resolves the schema, then runs the RPC.
/// For client-streaming and bidirectional methods further messages are sent with
/// [`StreamSession::send_text`] (JSON) and [`StreamSession::close_requests`]
/// half-closes the request stream.
pub fn start(call: Call) -> StreamSession {
    let (tx, rx) = channel();
    let (out_tx, out_rx) = channel::<Outgoing>();
    let cancel = Arc::new(AtomicBool::new(false));
    let accepting = Arc::new(AtomicBool::new(true));
    let flag = cancel.clone();
    let gate = accepting.clone();
    std::thread::spawn(move || {
        let out = Emitter {
            tx,
            started: Instant::now(),
        };
        if let Err(e) = resolve_and_run(call, &out, out_rx, &flag, &gate) {
            out.emit(Direction::Error, "error", e.to_string(), None);
        }
        gate.store(false, Ordering::Relaxed);
    });
    StreamSession::from_parts(rx, Some(out_tx), cancel, accepting)
}

fn resolve_and_run(
    call: Call,
    out: &Emitter,
    out_rx: std::sync::mpsc::Receiver<Outgoing>,
    cancel: &Arc<AtomicBool>,
    accepting: &AtomicBool,
) -> Result<(), GrpcError> {
    let pool = call.schema.load(&call.target)?;
    let method = find_method(&pool, &call.method)?;
    accepting.store(method.is_client_streaming(), Ordering::Relaxed);
    let messages = parse_messages(&method.input(), &call.body)?;
    run(
        RunSpec {
            target: call.target,
            method,
            messages,
        },
        out,
        out_rx,
        cancel,
    )
}

struct RunSpec {
    target: Target,
    method: MethodDescriptor,
    messages: Vec<DynamicMessage>,
}

fn run(
    call: RunSpec,
    out: &Emitter,
    out_rx: std::sync::mpsc::Receiver<Outgoing>,
    cancel: &Arc<AtomicBool>,
) -> Result<(), GrpcError> {
    let method = call.method.clone();
    let path = format!("/{}/{}", method.parent_service().full_name(), method.name());
    let client_streaming = method.is_client_streaming();
    if !client_streaming && call.messages.len() != 1 {
        return Err(GrpcError::Message(format!(
            "{path} is {}; the message editor must contain exactly one JSON object (found {})",
            method_info(&method).kind(),
            call.messages.len()
        )));
    }
    let metadata = metadata_map(&call.target.metadata)?;
    out.emit(
        Direction::Info,
        "connect",
        format!("{} · {}", call.target.uri, path),
        None,
    );
    let rt = runtime()?;
    let input = method.input();
    let timeout = call.target.timeout;
    let cancel_flag = cancel.clone();

    rt.block_on(async {
        let channel = tokio::select! {
            channel = connect(&call.target) => channel?,
            _ = async {
                while !cancel_flag.load(Ordering::Relaxed) {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
            } => {
                out.emit(Direction::Info, "close", "cancelled by client", None);
                return Ok(());
            }
        };
        out.emit(
            Direction::Info,
            "open",
            format!("connected ({})", method_info(&method).kind()),
            None,
        );
        let mut grpc = tonic::client::Grpc::new(channel);
        let path_and_query: tonic::codegen::http::uri::PathAndQuery =
            path.parse()
                .map_err(|e: tonic::codegen::http::uri::InvalidUri| {
                    GrpcError::UnknownMethod(e.to_string())
                })?;
        let codec = DynamicCodec {
            decode: method.output(),
        };

        // Request side: initial messages first, then whatever the user sends.
        let (msg_tx, msg_rx) = tokio::sync::mpsc::channel::<DynamicMessage>(64);
        for m in &call.messages {
            out.emit(Direction::Outgoing, "message", message_json(m), None);
            let _ = msg_tx.try_send(m.clone());
        }
        if client_streaming {
            let input = input.clone();
            let events = out.tx.clone();
            let started = out.started;
            std::thread::spawn(move || {
                while let Ok(item) = out_rx.recv() {
                    let text = match item {
                        Outgoing::Text(t) => t,
                        Outgoing::Close => break,
                    };
                    let report = |direction, label: &str, data: String| {
                        let _ = events.send(crate::stream::StreamEvent {
                            at_ms: started.elapsed().as_secs_f64() * 1000.0,
                            direction,
                            label: label.into(),
                            data,
                            id: None,
                        });
                    };
                    match parse_messages(&input, &text) {
                        Ok(messages) => {
                            for m in messages {
                                report(Direction::Outgoing, "message", message_json(&m));
                                if msg_tx.blocking_send(m).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(e) => report(Direction::Error, "error", e.to_string()),
                    }
                }
                // Dropping the sender half-closes the request stream.
            });
        } else {
            drop(msg_tx);
        }

        let mut request: tonic::Request<_> =
            tonic::Request::new(tokio_stream::wrappers::ReceiverStream::new(msg_rx));
        *request.metadata_mut() = metadata;
        if let Some(t) = timeout {
            request.set_timeout(t);
        }

        let work = async {
            grpc.ready()
                .await
                .map_err(|e| Status::unavailable(e.to_string()))?;
            if method.is_server_streaming() {
                let response = grpc.streaming(request, path_and_query, codec).await?;
                report_headers(out, response.metadata());
                let mut stream = response.into_inner();
                while let Some(message) = stream.message().await? {
                    out.emit(Direction::Incoming, "message", message_json(&message), None);
                }
                report_trailers(out, stream.trailers().await?);
            } else {
                let response = grpc
                    .client_streaming(request, path_and_query, codec)
                    .await?;
                report_headers(out, response.metadata());
                out.emit(
                    Direction::Incoming,
                    "message",
                    message_json(response.get_ref()),
                    None,
                );
            }
            Ok::<(), Status>(())
        };
        let watch = async {
            while !cancel_flag.load(Ordering::Relaxed) {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        tokio::select! {
            result = work => match result {
                Ok(()) => { out.emit(Direction::Info, "close", "status OK", None); }
                Err(status) => { out.emit(Direction::Error, "error", status_text(&status), None); }
            },
            _ = watch => { out.emit(Direction::Info, "close", "cancelled by client", None); }
        }
        Ok(())
    })
}

fn status_text(status: &Status) -> String {
    let mut text = format!("{:?}: {}", status.code(), status.message());
    if text.ends_with(": ") {
        text.truncate(text.len() - 2);
    }
    text
}

fn metadata_text(map: &MetadataMap) -> String {
    use tonic::metadata::KeyAndValueRef;
    map.iter()
        .map(|kv| match kv {
            KeyAndValueRef::Ascii(k, v) => {
                format!("{}: {}", k.as_str(), v.to_str().unwrap_or("<invalid>"))
            }
            KeyAndValueRef::Binary(k, v) => {
                format!("{}: <{} bytes>", k.as_str(), v.as_encoded_bytes().len())
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn report_headers(out: &Emitter, map: &MetadataMap) {
    let text = metadata_text(map);
    if !text.is_empty() {
        out.emit(Direction::Info, "headers", text, None);
    }
}

fn report_trailers(out: &Emitter, trailers: Option<MetadataMap>) {
    if let Some(map) = trailers {
        let text = metadata_text(&map);
        if !text.is_empty() {
            out.emit(Direction::Info, "trailers", text, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::convert::Infallible;
    use std::task::{Context, Poll};
    use tonic::codegen::{BoxFuture, http};
    use tonic::server::NamedService;
    use tower_service::Service;

    pub const PROTO: &str = r#"
syntax = "proto3";
package test;
service Echo {
  rpc Say(SayRequest) returns (SayReply);
  rpc Count(CountRequest) returns (stream SayReply);
  rpc Join(stream SayRequest) returns (SayReply);
}
message SayRequest { string name = 1; int64 times = 2; }
message CountRequest { int32 to = 1; }
message SayReply { string message = 1; }
"#;

    pub fn pool() -> DescriptorPool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("echo.proto");
        std::fs::write(&path, PROTO).unwrap();
        load_proto(&path, &[]).unwrap()
    }

    fn reply(pool: &DescriptorPool, text: String) -> DynamicMessage {
        let mut m = DynamicMessage::new(pool.get_message_by_name("test.SayReply").unwrap());
        m.set_field_by_name("message", prost_reflect::Value::String(text));
        m
    }

    fn name_of(m: &DynamicMessage) -> String {
        m.get_field_by_name("name")
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default()
    }

    #[derive(Clone)]
    struct Echo(DescriptorPool);

    impl NamedService for Echo {
        const NAME: &'static str = "test.Echo";
    }

    struct Unary(DescriptorPool);
    impl Service<tonic::Request<DynamicMessage>> for Unary {
        type Response = tonic::Response<DynamicMessage>;
        type Error = Status;
        type Future = std::future::Ready<Result<Self::Response, Status>>;
        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Status>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: tonic::Request<DynamicMessage>) -> Self::Future {
            let m = req.get_ref();
            let name = name_of(m);
            if name == "boom" {
                return std::future::ready(Err(Status::not_found("no such user")));
            }
            let auth = req
                .metadata()
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("none")
                .to_owned();
            std::future::ready(Ok(tonic::Response::new(reply(
                &self.0,
                format!("hello {name} ({auth})"),
            ))))
        }
    }

    struct Count(DescriptorPool);
    impl Service<tonic::Request<DynamicMessage>> for Count {
        type Response =
            tonic::Response<tokio_stream::Iter<std::vec::IntoIter<Result<DynamicMessage, Status>>>>;
        type Error = Status;
        type Future = std::future::Ready<Result<Self::Response, Status>>;
        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Status>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: tonic::Request<DynamicMessage>) -> Self::Future {
            let to = req
                .get_ref()
                .get_field_by_name("to")
                .and_then(|v| v.as_i32())
                .unwrap_or(0);
            let items: Vec<_> = (1..=to)
                .map(|i| Ok(reply(&self.0, format!("n{i}"))))
                .collect();
            std::future::ready(Ok(tonic::Response::new(tokio_stream::iter(items))))
        }
    }

    struct Join(DescriptorPool);
    impl Service<tonic::Request<tonic::Streaming<DynamicMessage>>> for Join {
        type Response = tonic::Response<DynamicMessage>;
        type Error = Status;
        type Future = BoxFuture<Self::Response, Status>;
        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Status>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: tonic::Request<tonic::Streaming<DynamicMessage>>) -> Self::Future {
            let pool = self.0.clone();
            Box::pin(async move {
                let mut stream = req.into_inner();
                let mut names = Vec::new();
                while let Some(m) = stream.message().await? {
                    names.push(name_of(&m));
                }
                Ok(tonic::Response::new(reply(&pool, names.join(","))))
            })
        }
    }

    impl Service<http::Request<tonic::body::Body>> for Echo {
        type Response = http::Response<tonic::body::Body>;
        type Error = Infallible;
        type Future = BoxFuture<Self::Response, Infallible>;
        fn poll_ready(&mut self, _: &mut Context<'_>) -> Poll<Result<(), Infallible>> {
            Poll::Ready(Ok(()))
        }
        fn call(&mut self, req: http::Request<tonic::body::Body>) -> Self::Future {
            let pool = self.0.clone();
            let path = req.uri().path().to_owned();
            Box::pin(async move {
                let desc = |m: &str| {
                    pool.get_service_by_name("test.Echo")
                        .unwrap()
                        .methods()
                        .find(|x| x.name() == m)
                        .unwrap()
                };
                let mut grpc;
                Ok(match path.as_str() {
                    "/test.Echo/Say" => {
                        grpc = tonic::server::Grpc::new(DynamicCodec {
                            decode: desc("Say").input(),
                        });
                        grpc.unary(Unary(pool.clone()), req).await
                    }
                    "/test.Echo/Count" => {
                        grpc = tonic::server::Grpc::new(DynamicCodec {
                            decode: desc("Count").input(),
                        });
                        grpc.server_streaming(Count(pool.clone()), req).await
                    }
                    "/test.Echo/Join" => {
                        grpc = tonic::server::Grpc::new(DynamicCodec {
                            decode: desc("Join").input(),
                        });
                        grpc.client_streaming(Join(pool.clone()), req).await
                    }
                    _ => Status::unimplemented("unknown method").into_http(),
                })
            })
        }
    }

    /// Serves Echo (plus reflection) on a random port; returns `http://127.0.0.1:port`.
    pub fn serve() -> String {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let pool = pool();
        let fds = pool_to_set(&pool);
        std::thread::spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(1)
                .build()
                .unwrap();
            rt.block_on(async move {
                let incoming = tokio_stream::wrappers::TcpListenerStream::new(
                    tokio::net::TcpListener::from_std(listener).unwrap(),
                );
                let reflection = tonic_reflection::server::Builder::configure()
                    .register_encoded_file_descriptor_set(&fds)
                    .build_v1()
                    .unwrap();
                let _ = tonic::transport::Server::builder()
                    .add_service(Echo(pool))
                    .add_service(reflection)
                    .serve_with_incoming(incoming)
                    .await;
            });
        });
        format!("http://127.0.0.1:{port}")
    }

    fn pool_to_set(pool: &DescriptorPool) -> Vec<u8> {
        let set = prost_reflect::prost_types::FileDescriptorSet {
            file: pool.file_descriptor_protos().cloned().collect(),
        };
        set.encode_to_vec()
    }

    fn target(uri: String) -> Target {
        Target {
            uri,
            metadata: vec![("Authorization".into(), "Bearer t".into())],
            timeout: Some(Duration::from_secs(10)),
            verify_tls: true,
            ca_pem: None,
        }
    }

    fn collect(mut session: StreamSession) -> Vec<crate::stream::StreamEvent> {
        let mut events = Vec::new();
        while let Some(e) = session.recv_timeout(Duration::from_secs(10)) {
            let done = e.label == "close" || e.direction == Direction::Error;
            events.push(e);
            if done {
                break;
            }
        }
        events
    }

    fn call(uri: &str, pool: &DescriptorPool, method: &str, body: &str) -> Call {
        Call {
            target: target(uri.to_owned()),
            schema: Schema::Pool(pool.clone()),
            method: method.to_owned(),
            body: body.to_owned(),
        }
    }

    #[test]
    fn lists_methods_and_builds_templates() {
        let pool = pool();
        let methods = list_methods(&pool);
        let names: Vec<_> = methods
            .iter()
            .map(|m| (m.name.as_str(), m.kind()))
            .collect();
        assert_eq!(
            names,
            [
                ("test.Echo/Count", "server streaming"),
                ("test.Echo/Join", "client streaming"),
                ("test.Echo/Say", "unary"),
            ]
        );
        let template = request_template(&find_method(&pool, "/test.Echo/Say").unwrap());
        let v: serde_json::Value = serde_json::from_str(&template).unwrap();
        assert_eq!(v, serde_json::json!({"name": "", "times": 0}));
        assert!(matches!(
            find_method(&pool, "test.Echo/Nope"),
            Err(GrpcError::UnknownMethod(_))
        ));
    }

    #[test]
    fn message_parsing_accepts_arrays_and_reports_errors() {
        let pool = pool();
        let input = find_method(&pool, "test.Echo/Join").unwrap().input();
        assert_eq!(
            parse_messages(&input, r#"[{"name":"a"},{"name":"b"}]"#)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            parse_messages(&input, r#"{"name":"a"} {"name":"b"}"#)
                .unwrap()
                .len(),
            2
        );
        assert!(parse_messages(&input, "  ").unwrap().is_empty());
        let err = parse_messages(&input, r#"{"nam":"a"}"#)
            .unwrap_err()
            .to_string();
        assert!(err.contains("message 1"), "{err}");
        assert!(parse_messages(&input, "{").is_err());
    }

    #[test]
    fn loads_broken_proto_with_an_actionable_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.proto");
        std::fs::write(&path, "syntax = \"proto3\"; message {").unwrap();
        let err = load_proto(&path, &[]).unwrap_err().to_string();
        assert!(err.starts_with("cannot read the .proto schema"), "{err}");
    }

    #[test]
    fn unary_call_sends_metadata_and_decodes_the_reply() {
        let uri = serve();
        let pool = pool();
        let events = collect(start(call(
            &uri,
            &pool,
            "test.Echo/Say",
            r#"{"name":"Ada"}"#,
        )));
        let incoming: Vec<_> = events
            .iter()
            .filter(|e| e.direction == Direction::Incoming)
            .collect();
        assert_eq!(incoming.len(), 1, "{events:?}");
        let v: serde_json::Value = serde_json::from_str(&incoming[0].data).unwrap();
        assert_eq!(v["message"], "hello Ada (Bearer t)");
        assert_eq!(events.last().unwrap().data, "status OK");
    }

    #[test]
    fn unary_call_reports_the_grpc_status() {
        let uri = serve();
        let pool = pool();
        let events = collect(start(call(
            &uri,
            &pool,
            "test.Echo/Say",
            r#"{"name":"boom"}"#,
        )));
        let last = events.last().unwrap();
        assert_eq!(last.direction, Direction::Error);
        assert_eq!(last.data, "NotFound: no such user");
    }

    #[test]
    fn server_streaming_delivers_every_message() {
        let uri = serve();
        let pool = pool();
        let events = collect(start(call(&uri, &pool, "test.Echo/Count", r#"{"to":3}"#)));
        let got: Vec<String> = events
            .iter()
            .filter(|e| e.direction == Direction::Incoming && e.label == "message")
            .map(|e| {
                serde_json::from_str::<serde_json::Value>(&e.data).unwrap()["message"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(got, ["n1", "n2", "n3"]);
        assert_eq!(events.last().unwrap().data, "status OK");
    }

    #[test]
    fn client_streaming_sends_initial_and_interactive_messages() {
        let uri = serve();
        let pool = pool();
        let mut session = start(call(
            &uri,
            &pool,
            "test.Echo/Join",
            r#"[{"name":"a"},{"name":"b"}]"#,
        ));
        assert!(session.can_send());
        assert!(session.send_text(r#"{"name":"c"}"#));
        // Wait until the interactive message was queued, then half-close.
        let mut events = Vec::new();
        while let Some(e) = session.recv_timeout(Duration::from_secs(10)) {
            let queued = e.direction == Direction::Outgoing && e.data.contains("\"c\"");
            events.push(e);
            if queued {
                break;
            }
        }
        session.close_requests();
        while let Some(e) = session.recv_timeout(Duration::from_secs(10)) {
            let done = e.label == "close" || e.direction == Direction::Error;
            events.push(e);
            if done {
                break;
            }
        }
        let reply = events
            .iter()
            .find(|e| e.direction == Direction::Incoming)
            .expect("reply");
        let v: serde_json::Value = serde_json::from_str(&reply.data).unwrap();
        assert_eq!(v["message"], "a,b,c");
    }

    #[test]
    fn unary_needs_exactly_one_message_and_unreachable_servers_fail_clearly() {
        let pool = pool();
        let events = collect(start(call(
            "http://127.0.0.1:9",
            &pool,
            "test.Echo/Say",
            "",
        )));
        assert!(events[0].data.contains("exactly one"), "{events:?}");
        let events = collect(start(call(
            "http://127.0.0.1:9",
            &pool,
            "test.Echo/Say",
            r#"{"name":"x"}"#,
        )));
        let last = events.last().unwrap();
        assert_eq!(last.direction, Direction::Error);
        assert!(
            last.data.contains("cannot connect to http://127.0.0.1:9"),
            "{}",
            last.data
        );
    }

    #[test]
    fn schema_can_be_fetched_by_reflection() {
        let uri = serve();
        let pool = fetch_reflection(&target(uri.clone())).unwrap();
        let names: Vec<_> = list_methods(&pool).into_iter().map(|m| m.name).collect();
        assert_eq!(
            names,
            ["test.Echo/Count", "test.Echo/Join", "test.Echo/Say"]
        );
        let events = collect(start(call(&uri, &pool, "test.Echo/Say", r#"{"name":"R"}"#)));
        assert!(events.iter().any(|e| e.data.contains("hello R")));
    }

    #[test]
    fn cancelling_stops_a_call() {
        let pool = pool();
        let session = start(call(
            "http://10.255.255.1:50051",
            &pool,
            "test.Echo/Say",
            r#"{"name":"x"}"#,
        ));
        session.close();
        let events = collect(session);
        assert!(!events.is_empty());
    }

    #[test]
    fn grpc_requests_serialize_and_resolve_schemes() {
        use crate::model::{Method, Protocol, Request};
        let mut r = Request::new("Say", Method::Post, "grpcs://api.example.com:443");
        r.protocol = Protocol::Grpc;
        r.grpc.proto_file = "api/echo.proto".into();
        r.grpc.method = "test.Echo/Say".into();
        let json = serde_json::to_value(&r).unwrap();
        assert_eq!(json["protocol"], "grpc");
        assert_eq!(json["grpc"]["method"], "test.Echo/Say");
        let back: Request = serde_json::from_value(json).unwrap();
        assert_eq!(back, r);
        // Plain HTTP requests do not grow a `grpc` key.
        let plain = serde_json::to_value(Request::new("x", Method::Get, "http://h")).unwrap();
        assert!(plain.get("grpc").is_none());

        let url =
            crate::exec::parse_url_for("grpcs://api.example.com:443", Protocol::Grpc).unwrap();
        assert_eq!(url.scheme(), "https");
        let url = crate::exec::parse_url_for("grpc://localhost:50051", Protocol::Grpc).unwrap();
        assert_eq!((url.scheme(), url.port()), ("http", Some(50051)));
    }
}
