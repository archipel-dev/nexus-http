//! Request execution: rendering a request definition into concrete bytes
//! ([`prepare`]) and sending it ([`Client::send`]) with redirects, cookies,
//! Digest/SigV4 auth, proxies, client certificates, timings and a timeline.

use crate::auth::{self, DigestChallenge, SigV4};
use crate::cookies::CookieJar;
use crate::model::{
    ApiKeyLocation, Auth, Body, Folder, Id, KeyValue, MultipartValue, Request, Workspace,
    WorkspaceSettings,
};
use crate::response::{Response, TimelineEntry, TimelineKind, Timings};
use crate::template::{Context, ResponseSource, TemplateError};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use url::Url;

pub const USER_AGENT: &str = concat!("nexus-http/", env!("CARGO_PKG_VERSION"));
/// Request/response bodies longer than this are elided in the timeline.
const TIMELINE_BODY_LIMIT: usize = 16 * 1024;

#[derive(Debug, Error)]
pub enum ExecError {
    #[error("request {0} not found in workspace")]
    NotFound(Id),
    #[error("{field}: {error}")]
    Template { field: String, error: TemplateError },
    #[error("invalid URL `{url}`: {reason}")]
    InvalidUrl { url: String, reason: String },
    #[error("cannot read {path}: {reason}")]
    File { path: String, reason: String },
    #[error("{0}")]
    Auth(String),
    #[error("invalid client configuration: {0}")]
    Config(String),
    #[error("could not connect to {host}: {reason}")]
    Connect { host: String, reason: String },
    #[error("request timed out after {0} ms")]
    Timeout(u64),
    #[error("stopped after {0} redirects; raise the limit in settings or disable redirects")]
    TooManyRedirects(u32),
    #[error("request cancelled")]
    Cancelled,
    #[error("network error: {0}")]
    Network(String),
}

/// Effective transport options for one request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transport {
    pub follow_redirects: bool,
    pub max_redirects: u32,
    pub verify_tls: bool,
    pub timeout_ms: u64,
    pub use_cookies: bool,
    pub max_response_bytes: u64,
}

/// Authentication that must happen while sending.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeferredAuth {
    None,
    Digest {
        username: String,
        password: String,
    },
    AwsSigV4 {
        access_key_id: String,
        secret_access_key: String,
        session_token: String,
        region: String,
        service: String,
    },
}

/// A fully rendered request ready to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedRequest {
    pub request_id: Option<Id>,
    pub method: String,
    pub url: Url,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub auth: DeferredAuth,
    pub transport: Transport,
}

impl PreparedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    fn set_header(&mut self, name: &str, value: String) {
        self.headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        self.headers.push((name.to_owned(), value));
    }
}

/// Inputs for [`prepare`].
pub struct PrepareOptions<'a> {
    /// Directory that relative file paths (binary bodies, multipart files) resolve against.
    pub base_dir: &'a Path,
    pub responses: &'a dyn ResponseSource,
}

/// Renders the request `id` of `workspace` into a [`PreparedRequest`].
pub fn prepare(
    workspace: &Workspace,
    id: &Id,
    options: &PrepareOptions<'_>,
) -> Result<PreparedRequest, ExecError> {
    let (folders, request) = workspace
        .find_request(id)
        .ok_or_else(|| ExecError::NotFound(id.clone()))?;
    let ctx = Context::for_request(workspace, &folders).with_responses(options.responses);
    prepare_request(
        request,
        &folders,
        &workspace.settings,
        &ctx,
        options.base_dir,
    )
}

/// Renders a request outside a workspace tree (folders outermost first).
pub fn prepare_request(
    request: &Request,
    folders: &[&Folder],
    settings: &WorkspaceSettings,
    ctx: &Context<'_>,
    base_dir: &Path,
) -> Result<PreparedRequest, ExecError> {
    let render = |field: &str, text: &str| {
        ctx.render(text).map_err(|error| ExecError::Template {
            field: field.to_owned(),
            error,
        })
    };

    let raw_url = render("URL", request.url.trim())?;
    let mut url = parse_url_for(&raw_url, request.protocol)?;
    let params = rendered_pairs(&request.params, "query parameter", &render)?;
    if !params.is_empty() {
        let mut query = url.query_pairs_mut();
        for (name, value) in &params {
            query.append_pair(name, value);
        }
    }

    // Folder headers apply outermost first; the request's own headers win.
    let mut headers: Vec<(String, String)> = Vec::new();
    for folder in folders {
        for (name, value) in rendered_pairs(&folder.headers, "folder header", &render)? {
            headers.retain(|(n, _)| !n.eq_ignore_ascii_case(&name));
            headers.push((name, value));
        }
    }
    let own = rendered_pairs(&request.headers, "header", &render)?;
    for (name, _) in &own {
        headers.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
    }
    headers.extend(own);

    let (body, content_type) = encode_body(&request.body, &render, base_dir)?;
    let has_content_type = headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("content-type"));
    if let Some(ct) = content_type
        && !has_content_type
    {
        headers.push(("Content-Type".into(), ct));
    }

    let transport = Transport {
        follow_redirects: request
            .settings
            .follow_redirects
            .unwrap_or(settings.follow_redirects),
        max_redirects: settings.max_redirects,
        verify_tls: request.settings.verify_tls.unwrap_or(settings.verify_tls),
        timeout_ms: request.settings.timeout_ms.unwrap_or(settings.timeout_ms),
        use_cookies: !request.settings.disable_cookies,
        max_response_bytes: settings.max_response_bytes,
    };
    let mut prepared = PreparedRequest {
        request_id: Some(request.id.clone()),
        method: request.method.as_str().to_owned(),
        url,
        headers,
        body,
        auth: DeferredAuth::None,
        transport,
    };
    let auth = effective_auth(request, folders);
    apply_auth(&mut prepared, auth, &render)?;
    Ok(prepared)
}

/// The auth in effect after resolving `Inherit` through the folders.
pub fn effective_auth<'a>(request: &'a Request, folders: &[&'a Folder]) -> &'a Auth {
    if request.auth != Auth::Inherit {
        return &request.auth;
    }
    folders
        .iter()
        .rev()
        .map(|f| &f.auth)
        .find(|a| **a != Auth::Inherit)
        .unwrap_or(&Auth::None)
}

fn apply_auth(
    prepared: &mut PreparedRequest,
    auth: &Auth,
    render: &dyn Fn(&str, &str) -> Result<String, ExecError>,
) -> Result<(), ExecError> {
    let has_authorization = prepared.header("authorization").is_some();
    match auth {
        Auth::None | Auth::Inherit => {}
        Auth::Basic { username, password } => {
            if !has_authorization {
                let header = auth::basic_header(
                    &render("username", username)?,
                    &render("password", password)?,
                );
                prepared.set_header("Authorization", header);
            }
        }
        Auth::Bearer { token, prefix } => {
            if !has_authorization {
                let token = render("token", token)?;
                let prefix = render("prefix", prefix)?;
                let value = if prefix.trim().is_empty() {
                    token
                } else {
                    format!("{} {token}", prefix.trim())
                };
                prepared.set_header("Authorization", value);
            }
        }
        Auth::ApiKey {
            key,
            value,
            location,
        } => {
            let key = render("API key name", key)?;
            let value = render("API key value", value)?;
            if key.is_empty() {
                return Err(ExecError::Auth("API key name is empty".into()));
            }
            match location {
                ApiKeyLocation::Header => prepared.set_header(&key, value),
                ApiKeyLocation::Query => {
                    prepared.url.query_pairs_mut().append_pair(&key, &value);
                }
                ApiKeyLocation::Cookie => {
                    let cookie = match prepared.header("cookie") {
                        Some(existing) => format!("{existing}; {key}={value}"),
                        None => format!("{key}={value}"),
                    };
                    prepared.set_header("Cookie", cookie);
                }
            }
        }
        Auth::Digest { username, password } => {
            prepared.auth = DeferredAuth::Digest {
                username: render("username", username)?,
                password: render("password", password)?,
            };
        }
        Auth::OAuth2(config) => {
            let token = config.token.as_ref().ok_or_else(|| {
                ExecError::Auth("no OAuth 2.0 token yet; use Fetch Token in the Auth tab".into())
            })?;
            if let Some(expires) = token.expires_at
                && expires <= crate::cookies::unix_now()
            {
                return Err(ExecError::Auth(
                    "the OAuth 2.0 token has expired; refresh or fetch a new token".into(),
                ));
            }
            if !has_authorization {
                let prefix = render("header prefix", &config.header_prefix)?;
                let value = if prefix.trim().is_empty() {
                    token.access_token.clone()
                } else {
                    format!("{} {}", prefix.trim(), token.access_token)
                };
                prepared.set_header("Authorization", value);
            }
        }
        Auth::AwsSigV4 {
            access_key_id,
            secret_access_key,
            session_token,
            region,
            service,
        } => {
            prepared.auth = DeferredAuth::AwsSigV4 {
                access_key_id: render("access key id", access_key_id)?,
                secret_access_key: render("secret access key", secret_access_key)?,
                session_token: render("session token", session_token)?,
                region: render("region", region)?,
                service: render("service", service)?,
            };
        }
    }
    Ok(())
}

fn rendered_pairs(
    pairs: &[KeyValue],
    what: &str,
    render: &dyn Fn(&str, &str) -> Result<String, ExecError>,
) -> Result<Vec<(String, String)>, ExecError> {
    pairs
        .iter()
        .filter(|kv| kv.enabled && !kv.name.trim().is_empty())
        .map(|kv| {
            let name = render(&format!("{what} name"), &kv.name)?;
            let value = render(&format!("{what} `{}`", kv.name), &kv.value)?;
            Ok((name, value))
        })
        .collect()
}

/// Parses a URL for `protocol`: WebSocket URLs use `ws`/`wss` (an `http(s)`
/// URL is converted), everything else `http`/`https`.
pub fn parse_url_for(raw: &str, protocol: crate::model::Protocol) -> Result<Url, ExecError> {
    if protocol == crate::model::Protocol::Grpc {
        // `grpc://` and `grpcs://` are shorthands for plain and TLS HTTP/2.
        let candidate = match raw.split_once("://") {
            Some(("grpc", rest)) => format!("http://{rest}"),
            Some(("grpcs", rest)) => format!("https://{rest}"),
            _ => raw.to_owned(),
        };
        return parse_url(&candidate);
    }
    if protocol != crate::model::Protocol::WebSocket {
        return parse_url(raw);
    }
    let candidate = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("ws://{raw}")
    };
    let candidate = match candidate.split_once("://") {
        Some(("http", rest)) => format!("ws://{rest}"),
        Some(("https", rest)) => format!("wss://{rest}"),
        _ => candidate,
    };
    let url = Url::parse(&candidate).map_err(|e| ExecError::InvalidUrl {
        url: raw.to_owned(),
        reason: e.to_string(),
    })?;
    match url.scheme() {
        "ws" | "wss" => Ok(url),
        other => Err(ExecError::InvalidUrl {
            url: raw.to_owned(),
            reason: format!("unsupported scheme `{other}` for WebSocket (use ws or wss)"),
        }),
    }
}

/// Parses a user-typed URL, defaulting to `http://` when no scheme is given.
pub fn parse_url(raw: &str) -> Result<Url, ExecError> {
    if raw.is_empty() {
        return Err(ExecError::InvalidUrl {
            url: String::new(),
            reason: "enter a URL".into(),
        });
    }
    let candidate = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("http://{raw}")
    };
    let url = Url::parse(&candidate).map_err(|e| ExecError::InvalidUrl {
        url: raw.to_owned(),
        reason: e.to_string(),
    })?;
    match url.scheme() {
        "http" | "https" => Ok(url),
        other => Err(ExecError::InvalidUrl {
            url: raw.to_owned(),
            reason: format!("unsupported scheme `{other}` (use http or https)"),
        }),
    }
}

fn encode_body(
    body: &Body,
    render: &dyn Fn(&str, &str) -> Result<String, ExecError>,
    base_dir: &Path,
) -> Result<(Vec<u8>, Option<String>), ExecError> {
    let default = body.default_content_type().map(str::to_owned);
    Ok(match body {
        Body::None => (Vec::new(), None),
        Body::Json { text } | Body::Xml { text } | Body::Text { text, .. } => {
            (render("body", text)?.into_bytes(), default)
        }
        Body::FormUrlEncoded { fields } => {
            let pairs = rendered_pairs(fields, "form field", render)?;
            let encoded = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(pairs)
                .finish();
            (encoded.into_bytes(), default)
        }
        Body::Binary { path } => {
            let path = resolve(base_dir, &render("file path", path)?);
            (read_file(&path)?, None)
        }
        Body::GraphQl {
            query,
            variables,
            operation_name,
        } => {
            let query = render("GraphQL query", query)?;
            let variables = render("GraphQL variables", variables)?;
            let mut payload = serde_json::Map::new();
            payload.insert("query".into(), query.into());
            if !variables.trim().is_empty() {
                let value: serde_json::Value =
                    serde_json::from_str(&variables).map_err(|e| ExecError::Template {
                        field: "GraphQL variables".into(),
                        error: TemplateError {
                            message: format!("variables are not valid JSON: {e}"),
                            span: 0..variables.len(),
                        },
                    })?;
                payload.insert("variables".into(), value);
            }
            if let Some(op) = operation_name.as_ref().filter(|o| !o.is_empty()) {
                payload.insert("operationName".into(), op.clone().into());
            }
            (
                serde_json::Value::Object(payload).to_string().into_bytes(),
                default,
            )
        }
        Body::Multipart { fields } => {
            let boundary = format!(
                "----nexus{}",
                &uuid::Uuid::new_v4().simple().to_string()[..20]
            );
            let mut out = Vec::new();
            for field in fields.iter().filter(|f| f.enabled && !f.name.is_empty()) {
                let name = render("multipart field name", &field.name)?.replace('"', "%22");
                out.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
                match &field.value {
                    MultipartValue::Text { value } => {
                        out.extend_from_slice(
                            format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n")
                                .as_bytes(),
                        );
                        out.extend_from_slice(
                            render(&format!("field `{name}`"), value)?.as_bytes(),
                        );
                    }
                    MultipartValue::File { path, content_type } => {
                        let path = resolve(base_dir, &render("file path", path)?);
                        let file_name = path
                            .file_name()
                            .map(|n| n.to_string_lossy().replace('"', "%22"))
                            .unwrap_or_else(|| "file".into());
                        let ct = content_type
                            .clone()
                            .unwrap_or_else(|| guess_mime(&path).to_owned());
                        out.extend_from_slice(
                            format!(
                                "Content-Disposition: form-data; name=\"{name}\"; filename=\"{file_name}\"\r\nContent-Type: {ct}\r\n\r\n"
                            )
                            .as_bytes(),
                        );
                        out.extend_from_slice(&read_file(&path)?);
                    }
                }
                out.extend_from_slice(b"\r\n");
            }
            out.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
            (
                out,
                Some(format!("multipart/form-data; boundary={boundary}")),
            )
        }
    })
}

fn resolve(base: &Path, path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_owned()
    } else {
        base.join(p)
    }
}

fn read_file(path: &Path) -> Result<Vec<u8>, ExecError> {
    std::fs::read(path).map_err(|e| ExecError::File {
        path: path.display().to_string(),
        reason: e.to_string(),
    })
}

/// A MIME type guessed from a file extension.
pub fn guess_mime(path: &Path) -> &'static str {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    match ext.as_str() {
        "json" => "application/json",
        "xml" => "application/xml",
        "txt" | "log" => "text/plain",
        "csv" => "text/csv",
        "html" | "htm" => "text/html",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        _ => "application/octet-stream",
    }
}

// ---------------------------------------------------------------------------
// Sending

/// Sends prepared requests. Holds connection pools per TLS/certificate setup.
pub struct Client {
    settings: WorkspaceSettings,
    /// Honor `HTTP(S)_PROXY`/`NO_PROXY` from the process environment when no
    /// explicit proxy is configured.
    pub use_system_proxy: bool,
    pools: Mutex<HashMap<(bool, Option<usize>), reqwest::blocking::Client>>,
}

impl Client {
    pub fn new(settings: WorkspaceSettings) -> Self {
        Self {
            settings,
            use_system_proxy: true,
            pools: Mutex::new(HashMap::new()),
        }
    }

    pub fn settings(&self) -> &WorkspaceSettings {
        &self.settings
    }

    fn pool(&self, url: &Url, verify_tls: bool) -> Result<reqwest::blocking::Client, ExecError> {
        let cert = self.client_certificate_for(url);
        let key = (verify_tls, cert);
        let mut pools = self.pools.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(client) = pools.get(&key) {
            return Ok(client.clone());
        }
        let mut builder = reqwest::blocking::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .danger_accept_invalid_certs(!verify_tls)
            .timeout(None::<Duration>)
            .connect_timeout(Duration::from_secs(30));
        let proxy = &self.settings.proxy;
        if proxy.enabled {
            let no_proxy = reqwest::NoProxy::from_string(&proxy.no_proxy);
            if !proxy.http.trim().is_empty() {
                builder = builder.proxy(
                    reqwest::Proxy::http(proxy.http.trim())
                        .map_err(|e| ExecError::Config(format!("HTTP proxy: {e}")))?
                        .no_proxy(no_proxy.clone()),
                );
            }
            if !proxy.https.trim().is_empty() {
                builder = builder.proxy(
                    reqwest::Proxy::https(proxy.https.trim())
                        .map_err(|e| ExecError::Config(format!("HTTPS proxy: {e}")))?
                        .no_proxy(no_proxy),
                );
            }
        } else if !self.use_system_proxy {
            builder = builder.no_proxy();
        }
        if let Some(ca) = &self.settings.ca_certificate {
            let pem = std::fs::read(ca).map_err(|e| ExecError::File {
                path: ca.clone(),
                reason: e.to_string(),
            })?;
            for cert in reqwest::Certificate::from_pem_bundle(&pem)
                .map_err(|e| ExecError::Config(format!("CA certificate {ca}: {e}")))?
            {
                builder = builder.add_root_certificate(cert);
            }
        }
        if let Some(index) = cert {
            let c = &self.settings.client_certificates[index];
            let mut pem = std::fs::read(&c.cert_path).map_err(|e| ExecError::File {
                path: c.cert_path.clone(),
                reason: e.to_string(),
            })?;
            if c.key_path != c.cert_path {
                pem.push(b'\n');
                pem.extend(std::fs::read(&c.key_path).map_err(|e| ExecError::File {
                    path: c.key_path.clone(),
                    reason: e.to_string(),
                })?);
            }
            let identity = reqwest::Identity::from_pem(&pem).map_err(|e| {
                ExecError::Config(format!("client certificate for {}: {e}", c.host))
            })?;
            builder = builder.identity(identity);
        }
        let client = builder
            .build()
            .map_err(|e| ExecError::Config(error_chain(&e)))?;
        pools.insert(key, client.clone());
        Ok(client)
    }

    fn client_certificate_for(&self, url: &Url) -> Option<usize> {
        let host = url.host_str()?;
        let with_port = url.port().map(|p| format!("{host}:{p}"));
        self.settings.client_certificates.iter().position(|c| {
            c.enabled
                && (c.host.eq_ignore_ascii_case(host)
                    || with_port
                        .as_deref()
                        .is_some_and(|hp| c.host.eq_ignore_ascii_case(hp)))
        })
    }

    /// Sends a prepared request, following redirects and answering Digest
    /// challenges. Cookies are read from and stored into `jar` when enabled.
    pub fn send(
        &self,
        prepared: &PreparedRequest,
        mut jar: Option<&mut CookieJar>,
        cancel: &AtomicBool,
    ) -> Result<Response, ExecError> {
        let started = Instant::now();
        let started_at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let transport = &prepared.transport;
        let mut timeline = vec![TimelineEntry::new(
            TimelineKind::Info,
            format!("Preparing request to {}", prepared.url),
        )];
        let mut method = prepared.method.clone();
        let mut url = prepared.url.clone();
        let mut headers = prepared.headers.clone();
        let mut body = prepared.body.clone();
        let mut redirects = 0u32;
        let mut digest_attempted = false;
        let mut digest_nc = 0u32;
        let mut wait_ms = 0.0;
        let prepare_ms = 0.0;

        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err(ExecError::Cancelled);
            }
            let mut send_headers = headers.clone();
            if !send_headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("user-agent"))
            {
                send_headers.push(("User-Agent".into(), USER_AGENT.into()));
            }
            if !send_headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("accept"))
            {
                send_headers.push(("Accept".into(), "*/*".into()));
            }
            if transport.use_cookies
                && let Some(jar) = jar.as_deref()
                && let Some(stored) = jar.header_for(&url)
            {
                match send_headers
                    .iter_mut()
                    .find(|(n, _)| n.eq_ignore_ascii_case("cookie"))
                {
                    Some((_, v)) => *v = format!("{v}; {stored}"),
                    None => send_headers.push(("Cookie".into(), stored)),
                }
            }
            if let DeferredAuth::AwsSigV4 {
                access_key_id,
                secret_access_key,
                session_token,
                region,
                service,
            } = &prepared.auth
            {
                SigV4 {
                    access_key_id,
                    secret_access_key,
                    session_token,
                    region,
                    service,
                }
                .sign(
                    &method,
                    &url,
                    &mut send_headers,
                    &body,
                    &auth::amz_date_now(),
                )
                .map_err(ExecError::Auth)?;
            }

            push_request_timeline(&mut timeline, &method, &url, &send_headers, &body);
            let client = self.pool(&url, transport.verify_tls)?;
            let reqwest_method = reqwest::Method::from_bytes(method.as_bytes())
                .map_err(|_| ExecError::Config(format!("invalid HTTP method {method}")))?;
            let mut builder = client.request(reqwest_method, url.clone());
            for (name, value) in &send_headers {
                builder = builder.header(name.as_str(), value.as_str());
            }
            if transport.timeout_ms > 0 {
                let elapsed = started.elapsed().as_millis() as u64;
                let remaining = transport.timeout_ms.saturating_sub(elapsed).max(1);
                builder = builder.timeout(Duration::from_millis(remaining));
            }
            if !body.is_empty() {
                builder = builder.body(body.clone());
            }
            let sent = Instant::now();
            let mut response = builder.send().map_err(|e| {
                let error = classify(&e, &url, transport.timeout_ms);
                timeline.push(TimelineEntry::new(TimelineKind::Error, error.to_string()));
                error
            })?;
            wait_ms += sent.elapsed().as_secs_f64() * 1000.0;

            let status = response.status();
            let version = format!("{:?}", response.version());
            let response_headers: Vec<(String, String)> = response
                .headers()
                .iter()
                .map(|(n, v)| {
                    (
                        n.to_string(),
                        String::from_utf8_lossy(v.as_bytes()).into_owned(),
                    )
                })
                .collect();
            timeline.push(TimelineEntry::new(
                TimelineKind::ResponseHeader,
                format!(
                    "{version} {} {}",
                    status.as_u16(),
                    status.canonical_reason().unwrap_or("")
                ),
            ));
            for (name, value) in &response_headers {
                timeline.push(TimelineEntry::new(
                    TimelineKind::ResponseHeader,
                    format!("{name}: {value}"),
                ));
            }
            if transport.use_cookies
                && let Some(jar) = jar.as_deref_mut()
            {
                for (_, value) in response_headers
                    .iter()
                    .filter(|(n, _)| n.eq_ignore_ascii_case("set-cookie"))
                {
                    jar.store(&url, value);
                }
            }

            // Digest challenge: answer once.
            if status.as_u16() == 401
                && !digest_attempted
                && let DeferredAuth::Digest { username, password } = &prepared.auth
            {
                let challenge = response_headers
                    .iter()
                    .filter(|(n, _)| n.eq_ignore_ascii_case("www-authenticate"))
                    .find_map(|(_, v)| DigestChallenge::parse(v));
                if let Some(challenge) = challenge {
                    digest_attempted = true;
                    digest_nc += 1;
                    let uri = match url.query() {
                        Some(q) => format!("{}?{q}", url.path()),
                        None => url.path().to_owned(),
                    };
                    let cnonce = &uuid::Uuid::new_v4().simple().to_string()[..16];
                    let header = challenge
                        .respond(&method, &uri, username, password, cnonce, digest_nc)
                        .map_err(ExecError::Auth)?;
                    timeline.push(TimelineEntry::new(
                        TimelineKind::Info,
                        "Answering Digest challenge",
                    ));
                    headers.retain(|(n, _)| !n.eq_ignore_ascii_case("authorization"));
                    headers.push(("Authorization".into(), header));
                    let _ = drain(&mut response, 64 * 1024, cancel);
                    continue;
                }
            }

            let location = response_headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case("location"))
                .map(|(_, v)| v.clone());
            if status.is_redirection()
                && transport.follow_redirects
                && let Some(location) = location
            {
                if redirects >= transport.max_redirects {
                    return Err(ExecError::TooManyRedirects(redirects));
                }
                let next = url.join(&location).map_err(|e| ExecError::InvalidUrl {
                    url: location.clone(),
                    reason: e.to_string(),
                })?;
                redirects += 1;
                let code = status.as_u16();
                let drop_body = code == 303
                    || ((code == 301 || code == 302) && method != "GET" && method != "HEAD");
                if drop_body {
                    if method != "HEAD" {
                        method = "GET".into();
                    }
                    body.clear();
                    headers.retain(|(n, _)| {
                        !n.eq_ignore_ascii_case("content-type")
                            && !n.eq_ignore_ascii_case("content-length")
                    });
                }
                if next.origin() != url.origin() {
                    headers.retain(|(n, _)| {
                        !n.eq_ignore_ascii_case("authorization")
                            && !n.eq_ignore_ascii_case("cookie")
                    });
                }
                timeline.push(TimelineEntry::new(
                    TimelineKind::Info,
                    format!("Following redirect to {next}"),
                ));
                let _ = drain(&mut response, 64 * 1024, cancel);
                url = next;
                continue;
            }

            let download = Instant::now();
            let (bytes, truncated) = drain(&mut response, transport.max_response_bytes, cancel)?;
            let download_ms = download.elapsed().as_secs_f64() * 1000.0;
            push_body_timeline(&mut timeline, TimelineKind::ResponseBody, &bytes);
            if truncated {
                timeline.push(TimelineEntry::new(
                    TimelineKind::Info,
                    format!(
                        "Response body truncated at {}",
                        crate::response::format_size(transport.max_response_bytes)
                    ),
                ));
            }
            let total_ms = started.elapsed().as_secs_f64() * 1000.0;
            timeline.push(TimelineEntry::new(
                TimelineKind::Info,
                format!(
                    "Received {} in {}",
                    crate::response::format_size(bytes.len() as u64),
                    crate::response::format_duration(total_ms)
                ),
            ));
            return Ok(Response {
                request_id: prepared.request_id.clone(),
                method,
                url: url.to_string(),
                status: status.as_u16(),
                reason: status.canonical_reason().unwrap_or("").to_owned(),
                http_version: version,
                headers: response_headers,
                body: bytes,
                truncated,
                timings: Timings {
                    prepare_ms,
                    wait_ms,
                    download_ms,
                    total_ms,
                },
                timeline,
                redirects,
                started_at,
            });
        }
    }
}

impl Client {
    /// Sends a request and returns the live response for streaming (Server-Sent
    /// Events). Redirects follow reqwest's default policy; cookies are sent but
    /// the body is not read.
    pub fn open_stream(
        &self,
        prepared: &PreparedRequest,
        jar: Option<&CookieJar>,
    ) -> Result<reqwest::blocking::Response, ExecError> {
        let url = &prepared.url;
        let client = self.pool(url, prepared.transport.verify_tls)?;
        let method = reqwest::Method::from_bytes(prepared.method.as_bytes())
            .map_err(|_| ExecError::Config(format!("invalid HTTP method {}", prepared.method)))?;
        let mut builder = client.request(method, url.clone());
        for (name, value) in &prepared.headers {
            builder = builder.header(name.as_str(), value.as_str());
        }
        if !prepared
            .headers
            .iter()
            .any(|(n, _)| n.eq_ignore_ascii_case("user-agent"))
        {
            builder = builder.header("User-Agent", USER_AGENT);
        }
        if prepared.transport.use_cookies
            && let Some(cookie) = jar.and_then(|j| j.header_for(url))
        {
            builder = builder.header("Cookie", cookie);
        }
        if !prepared.body.is_empty() {
            builder = builder.body(prepared.body.clone());
        }
        builder
            .send()
            .map_err(|e| classify(&e, url, prepared.transport.timeout_ms))
    }

    /// Runs an OAuth 2.0 token request and parses the token.
    pub fn fetch_oauth_token(
        &self,
        request: &auth::TokenRequest,
        cancel: &AtomicBool,
    ) -> Result<crate::model::OAuth2Token, ExecError> {
        let url = parse_url(&request.url)?;
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(&request.form)
            .finish()
            .into_bytes();
        let mut headers = vec![
            (
                "Content-Type".to_owned(),
                "application/x-www-form-urlencoded".to_owned(),
            ),
            ("Accept".to_owned(), "application/json".to_owned()),
        ];
        if let Some(basic) = &request.basic_auth {
            headers.push(("Authorization".into(), basic.clone()));
        }
        let prepared = PreparedRequest {
            request_id: None,
            method: "POST".into(),
            url,
            headers,
            body,
            auth: DeferredAuth::None,
            transport: Transport {
                follow_redirects: true,
                max_redirects: self.settings.max_redirects,
                verify_tls: self.settings.verify_tls,
                timeout_ms: self.settings.timeout_ms,
                use_cookies: false,
                max_response_bytes: 1024 * 1024,
            },
        };
        let response = self.send(&prepared, None, cancel)?;
        auth::parse_token_response(&response.body, crate::cookies::unix_now()).map_err(|e| {
            ExecError::Auth(format!("token request failed ({}): {e}", response.status))
        })
    }
}

fn drain(
    response: &mut reqwest::blocking::Response,
    limit: u64,
    cancel: &AtomicBool,
) -> Result<(Vec<u8>, bool), ExecError> {
    let mut out = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(ExecError::Cancelled);
        }
        let n = response
            .read(&mut chunk)
            .map_err(|e| ExecError::Network(format!("reading response body: {e}")))?;
        if n == 0 {
            return Ok((out, false));
        }
        let room = (limit as usize).saturating_sub(out.len());
        if n > room {
            out.extend_from_slice(&chunk[..room]);
            return Ok((out, true));
        }
        out.extend_from_slice(&chunk[..n]);
    }
}

fn push_request_timeline(
    timeline: &mut Vec<TimelineEntry>,
    method: &str,
    url: &Url,
    headers: &[(String, String)],
    body: &[u8],
) {
    let target = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_owned(),
    };
    timeline.push(TimelineEntry::new(
        TimelineKind::RequestHeader,
        format!("{method} {target} HTTP/1.1"),
    ));
    if let Some(host) = url.host_str() {
        let host = match url.port() {
            Some(p) => format!("{host}:{p}"),
            None => host.to_owned(),
        };
        timeline.push(TimelineEntry::new(
            TimelineKind::RequestHeader,
            format!("Host: {host}"),
        ));
    }
    for (name, value) in headers {
        timeline.push(TimelineEntry::new(
            TimelineKind::RequestHeader,
            format!("{name}: {value}"),
        ));
    }
    push_body_timeline(timeline, TimelineKind::RequestBody, body);
}

fn push_body_timeline(timeline: &mut Vec<TimelineEntry>, kind: TimelineKind, body: &[u8]) {
    if body.is_empty() {
        return;
    }
    let text = match std::str::from_utf8(body) {
        Ok(text) if text.len() <= TIMELINE_BODY_LIMIT => text.to_owned(),
        Ok(text) => {
            let mut end = TIMELINE_BODY_LIMIT;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            format!("{}\n… ({} more bytes)", &text[..end], text.len() - end)
        }
        Err(_) => format!("<{} bytes of binary data>", body.len()),
    };
    timeline.push(TimelineEntry::new(kind, text));
}

fn classify(error: &reqwest::Error, url: &Url, timeout_ms: u64) -> ExecError {
    if error.is_timeout() {
        return ExecError::Timeout(timeout_ms);
    }
    let reason = error_chain(error);
    if error.is_connect() {
        return ExecError::Connect {
            host: url.host_str().unwrap_or("").to_owned(),
            reason,
        };
    }
    ExecError::Network(reason)
}

/// The error and its sources, joined, so TLS/DNS causes are visible.
fn error_chain(error: &dyn std::error::Error) -> String {
    let mut parts = vec![error.to_string()];
    let mut source = error.source();
    while let Some(s) = source {
        let text = s.to_string();
        if !parts.iter().any(|p| p.contains(&text)) {
            parts.push(text);
        }
        source = s.source();
    }
    parts.join(": ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Method, MultipartField};
    use crate::template::NoResponses;
    use serde_json::json;

    fn options(dir: &Path) -> PrepareOptions<'_> {
        PrepareOptions {
            base_dir: dir,
            responses: &NoResponses,
        }
    }

    #[test]
    fn prepares_url_params_headers_and_folder_inheritance() {
        let mut ws = Workspace::new("W");
        ws.base_environment = ws
            .base_environment
            .clone()
            .with("host", "api.test")
            .with("tok", "s3cr3t");
        let mut folder = Folder::new("F");
        folder.headers.push(KeyValue::new("X-Team", "core"));
        folder.headers.push(KeyValue::new("Accept", "text/plain"));
        folder.auth = Auth::Bearer {
            token: "{{tok}}".into(),
            prefix: "Bearer".into(),
        };
        let mut req = Request::new("R", Method::Get, "{{host}}/items?fixed=1")
            .param("q", "a b")
            .header("Accept", "application/json");
        req.params.push(KeyValue {
            enabled: false,
            ..KeyValue::new("off", "1")
        });
        req.auth = Auth::Inherit;
        let id = req.id.clone();
        folder.children.push(crate::model::Item::Request(req));
        ws.items.push(crate::model::Item::Folder(folder));

        let p = prepare(&ws, &id, &options(Path::new("."))).unwrap();
        assert_eq!(p.url.as_str(), "http://api.test/items?fixed=1&q=a+b");
        assert_eq!(p.header("accept"), Some("application/json"));
        assert_eq!(p.header("x-team"), Some("core"));
        assert_eq!(p.header("authorization"), Some("Bearer s3cr3t"));
    }

    #[test]
    fn reports_template_errors_with_field() {
        let mut ws = Workspace::new("W");
        let req = Request::new("R", Method::Get, "http://x/{{missing}}");
        let id = req.id.clone();
        ws.items.push(crate::model::Item::Request(req));
        let err = prepare(&ws, &id, &options(Path::new("."))).unwrap_err();
        assert!(
            err.to_string()
                .starts_with("URL: undefined variable `missing`"),
            "{err}"
        );
        assert!(matches!(
            parse_url("ftp://x"),
            Err(ExecError::InvalidUrl { .. })
        ));
        assert!(matches!(parse_url(""), Err(ExecError::InvalidUrl { .. })));
    }

    fn prepare_one(req: Request, dir: &Path) -> PreparedRequest {
        let ctx = Context::new(json!({"v": "1"}).as_object().unwrap().clone());
        prepare_request(&req, &[], &WorkspaceSettings::default(), &ctx, dir).unwrap()
    }

    #[test]
    fn encodes_bodies() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.png"), [1u8, 2, 3]).unwrap();

        let form = prepare_one(
            Request::new("f", Method::Post, "http://x").with_body(Body::FormUrlEncoded {
                fields: vec![KeyValue::new("a", "{{v}}"), KeyValue::new("b c", "&")],
            }),
            dir.path(),
        );
        assert_eq!(form.body, b"a=1&b+c=%26");
        assert_eq!(
            form.header("content-type"),
            Some("application/x-www-form-urlencoded")
        );

        let gql = prepare_one(
            Request::new("g", Method::Post, "http://x").with_body(Body::GraphQl {
                query: "{ me { id } }".into(),
                variables: "{\"n\": {{v}}}".into(),
                operation_name: None,
            }),
            dir.path(),
        );
        let v: serde_json::Value = serde_json::from_slice(&gql.body).unwrap();
        assert_eq!(v, json!({"query": "{ me { id } }", "variables": {"n": 1}}));

        let multipart = prepare_one(
            Request::new("m", Method::Post, "http://x").with_body(Body::Multipart {
                fields: vec![
                    MultipartField {
                        name: "t".into(),
                        value: MultipartValue::Text { value: "hi".into() },
                        enabled: true,
                    },
                    MultipartField {
                        name: "f".into(),
                        value: MultipartValue::File {
                            path: "a.png".into(),
                            content_type: None,
                        },
                        enabled: true,
                    },
                ],
            }),
            dir.path(),
        );
        let ct = multipart.header("content-type").unwrap();
        let boundary = ct.split("boundary=").nth(1).unwrap();
        let text = String::from_utf8_lossy(&multipart.body);
        assert!(text.contains("name=\"t\"\r\n\r\nhi\r\n"));
        assert!(text.contains("filename=\"a.png\"\r\nContent-Type: image/png"));
        assert!(text.ends_with(&format!("--{boundary}--\r\n")));

        let missing = prepare_request(
            &Request::new("b", Method::Post, "http://x").with_body(Body::Binary {
                path: "nope.bin".into(),
            }),
            &[],
            &WorkspaceSettings::default(),
            &Context::new(Default::default()),
            dir.path(),
        );
        assert!(matches!(missing, Err(ExecError::File { .. })));
    }

    #[test]
    fn applies_api_key_locations() {
        for (location, check) in [
            (ApiKeyLocation::Header, "header"),
            (ApiKeyLocation::Query, "query"),
            (ApiKeyLocation::Cookie, "cookie"),
        ] {
            let p = prepare_one(
                Request::new("k", Method::Get, "http://x/").with_auth(Auth::ApiKey {
                    key: "k".into(),
                    value: "{{v}}".into(),
                    location,
                }),
                Path::new("."),
            );
            match check {
                "header" => assert_eq!(p.header("k"), Some("1")),
                "query" => assert_eq!(p.url.query(), Some("k=1")),
                _ => assert_eq!(p.header("cookie"), Some("k=1")),
            }
        }
    }

    #[test]
    fn oauth2_without_token_is_actionable() {
        let ctx = Context::new(Default::default());
        let req =
            Request::new("o", Method::Get, "http://x/").with_auth(Auth::OAuth2(Box::default()));
        let err = prepare_request(
            &req,
            &[],
            &WorkspaceSettings::default(),
            &ctx,
            Path::new("."),
        )
        .unwrap_err();
        assert!(err.to_string().contains("Fetch Token"));
    }
}
