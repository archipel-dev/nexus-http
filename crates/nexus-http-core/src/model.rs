//! Data model: workspaces with a tree of folders and requests, environments,
//! request settings and auth. Everything is serde-serializable with stable field
//! order so the on-disk format diffs cleanly in git.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fmt;

/// Stable identifier of a workspace item (folder, request, environment).
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Id(pub String);

impl Id {
    /// A fresh random identifier, prefixed by the item kind (`req_`, `fld_`, ...).
    pub fn new(prefix: &str) -> Self {
        let simple = uuid::Uuid::new_v4().simple().to_string();
        Self(format!("{prefix}_{}", &simple[..16]))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the id is safe to use as a file name.
    pub fn is_valid(&self) -> bool {
        !self.0.is_empty()
            && self.0.len() <= 96
            && self
                .0
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
            && !self.0.starts_with('.')
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for Id {
    fn from(value: &str) -> Self {
        Self(value.to_owned())
    }
}

/// HTTP method. Custom methods (`PROPFIND`, ...) are preserved verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(from = "String", into = "String")]
pub enum Method {
    Get,
    Post,
    Put,
    Patch,
    Delete,
    Head,
    Options,
    Custom(String),
}

impl Method {
    pub const COMMON: [Method; 7] = [
        Method::Get,
        Method::Post,
        Method::Put,
        Method::Patch,
        Method::Delete,
        Method::Head,
        Method::Options,
    ];

    pub fn as_str(&self) -> &str {
        match self {
            Method::Get => "GET",
            Method::Post => "POST",
            Method::Put => "PUT",
            Method::Patch => "PATCH",
            Method::Delete => "DELETE",
            Method::Head => "HEAD",
            Method::Options => "OPTIONS",
            Method::Custom(m) => m,
        }
    }
}

impl From<String> for Method {
    fn from(value: String) -> Self {
        match value.to_ascii_uppercase().as_str() {
            "GET" => Method::Get,
            "POST" => Method::Post,
            "PUT" => Method::Put,
            "PATCH" => Method::Patch,
            "DELETE" => Method::Delete,
            "HEAD" => Method::Head,
            "OPTIONS" => Method::Options,
            _ => Method::Custom(value.to_ascii_uppercase()),
        }
    }
}

impl From<Method> for String {
    fn from(value: Method) -> Self {
        value.as_str().to_owned()
    }
}

impl fmt::Display for Method {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A toggleable name/value pair (query parameter, header, form field).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeyValue {
    pub name: String,
    pub value: String,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
}

impl Default for KeyValue {
    /// An empty, enabled row.
    fn default() -> Self {
        Self::new("", "")
    }
}

impl KeyValue {
    pub fn new(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            enabled: true,
            description: String::new(),
        }
    }
}

/// A multipart form field: text or file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MultipartField {
    pub name: String,
    #[serde(flatten)]
    pub value: MultipartValue,
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MultipartValue {
    Text {
        value: String,
    },
    File {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
}

/// Request body.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Body {
    #[default]
    None,
    Json {
        text: String,
    },
    Xml {
        text: String,
    },
    Text {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        content_type: Option<String>,
    },
    FormUrlEncoded {
        fields: Vec<KeyValue>,
    },
    Multipart {
        fields: Vec<MultipartField>,
    },
    /// Raw bytes read from a file at send time.
    Binary {
        path: String,
    },
    GraphQl {
        query: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        variables: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        operation_name: Option<String>,
    },
}

impl Body {
    /// The `Content-Type` sent when the user did not set one explicitly.
    /// Multipart's boundary is added by the executor.
    pub fn default_content_type(&self) -> Option<&str> {
        match self {
            Body::None | Body::Binary { .. } | Body::Multipart { .. } => None,
            Body::Json { .. } | Body::GraphQl { .. } => Some("application/json"),
            Body::Xml { .. } => Some("application/xml"),
            Body::Text { content_type, .. } => {
                Some(content_type.as_deref().unwrap_or("text/plain"))
            }
            Body::FormUrlEncoded { .. } => Some("application/x-www-form-urlencoded"),
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Body::None => "No Body",
            Body::Json { .. } => "JSON",
            Body::Xml { .. } => "XML",
            Body::Text { .. } => "Plain Text",
            Body::FormUrlEncoded { .. } => "Form URL Encoded",
            Body::Multipart { .. } => "Multipart Form",
            Body::Binary { .. } => "Binary File",
            Body::GraphQl { .. } => "GraphQL",
        }
    }
}

/// Where an API key is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApiKeyLocation {
    #[default]
    Header,
    Query,
    Cookie,
}

/// OAuth 2.0 grant types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuth2Grant {
    #[default]
    AuthorizationCode,
    ClientCredentials,
    Password,
    Implicit,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OAuth2Config {
    pub grant: OAuth2Grant,
    #[serde(default)]
    pub authorization_url: String,
    #[serde(default)]
    pub access_token_url: String,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default)]
    pub redirect_url: String,
    #[serde(default)]
    pub scope: String,
    #[serde(default)]
    pub audience: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    #[serde(default = "yes")]
    pub use_pkce: bool,
    /// Send client credentials as HTTP Basic instead of in the body.
    #[serde(default)]
    pub credentials_in_header: bool,
    /// Token prefix in the `Authorization` header.
    #[serde(default = "bearer_prefix")]
    pub header_prefix: String,
    /// The last token obtained, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token: Option<OAuth2Token>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OAuth2Token {
    pub access_token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_token: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub token_type: Option<String>,
    /// Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<u64>,
}

/// Request authentication.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Auth {
    #[default]
    None,
    /// Use the parent folder's auth.
    Inherit,
    Basic {
        username: String,
        password: String,
    },
    Bearer {
        token: String,
        #[serde(default = "bearer_prefix")]
        prefix: String,
    },
    ApiKey {
        key: String,
        value: String,
        #[serde(default)]
        location: ApiKeyLocation,
    },
    Digest {
        username: String,
        password: String,
    },
    OAuth2(Box<OAuth2Config>),
    AwsSigV4 {
        access_key_id: String,
        secret_access_key: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        session_token: String,
        region: String,
        service: String,
    },
}

impl Auth {
    pub fn label(&self) -> &'static str {
        match self {
            Auth::None => "No Auth",
            Auth::Inherit => "Inherit from parent",
            Auth::Basic { .. } => "Basic Auth",
            Auth::Bearer { .. } => "Bearer Token",
            Auth::ApiKey { .. } => "API Key",
            Auth::Digest { .. } => "Digest Auth",
            Auth::OAuth2(_) => "OAuth 2.0",
            Auth::AwsSigV4 { .. } => "AWS Signature v4",
        }
    }
}

/// Per-request transport settings; `None` falls back to workspace settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct RequestSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub follow_redirects: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub verify_tls: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// Do not send or store cookies for this request.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub disable_cookies: bool,
    /// Do not percent-encode the URL.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub raw_url: bool,
}

/// How a request talks to the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// One request, one response.
    #[default]
    Http,
    /// A WebSocket connection (`ws://`/`wss://`); the body is the message composer.
    WebSocket,
    /// A Server-Sent Events stream (`text/event-stream`).
    Sse,
    /// A gRPC call; the URL is the target, `grpc` selects the method, headers are
    /// metadata and the body is the JSON request message(s).
    Grpc,
}

/// gRPC specific request settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct GrpcConfig {
    /// `.proto` file to load the schema from; empty uses server reflection.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub proto_file: String,
    /// Method as `package.Service/Method`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub method: String,
}

impl GrpcConfig {
    pub fn is_default(&self) -> bool {
        *self == GrpcConfig::default()
    }
}

impl Protocol {
    pub fn label(self) -> &'static str {
        match self {
            Protocol::Http => "HTTP",
            Protocol::WebSocket => "WebSocket",
            Protocol::Sse => "Server-Sent Events",
            Protocol::Grpc => "gRPC",
        }
    }

    pub fn is_http(&self) -> bool {
        *self == Protocol::Http
    }
}

/// An HTTP request definition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Request {
    pub id: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "Protocol::is_http")]
    pub protocol: Protocol,
    #[serde(default, skip_serializing_if = "GrpcConfig::is_default")]
    pub grpc: GrpcConfig,
    pub method: Method,
    pub url: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub params: Vec<KeyValue>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub body: Body,
    #[serde(default)]
    pub auth: Auth,
    #[serde(default)]
    pub settings: RequestSettings,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    /// Assertions checked after each response (see `assertions`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assertions: Vec<crate::assertions::Assertion>,
}

impl Request {
    pub fn new(name: impl Into<String>, method: Method, url: impl Into<String>) -> Self {
        Self {
            id: Id::new("req"),
            name: name.into(),
            protocol: Protocol::Http,
            grpc: GrpcConfig::default(),
            method,
            url: url.into(),
            params: Vec::new(),
            headers: Vec::new(),
            body: Body::None,
            auth: Auth::None,
            settings: RequestSettings::default(),
            description: String::new(),
            assertions: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push(KeyValue::new(name, value));
        self
    }

    pub fn param(mut self, name: &str, value: &str) -> Self {
        self.params.push(KeyValue::new(name, value));
        self
    }

    pub fn with_body(mut self, body: Body) -> Self {
        self.body = body;
        self
    }

    pub fn with_auth(mut self, auth: Auth) -> Self {
        self.auth = auth;
        self
    }
}

/// A folder groups requests and can carry shared headers, auth and variables.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Folder {
    pub id: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub headers: Vec<KeyValue>,
    #[serde(default)]
    pub auth: Auth,
    /// Folder-level variables, overriding environment values for children.
    #[serde(default, skip_serializing_if = "Map::is_empty")]
    pub variables: Map<String, Value>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<Item>,
}

impl Folder {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Id::new("fld"),
            name: name.into(),
            headers: Vec::new(),
            auth: Auth::Inherit,
            variables: Map::new(),
            description: String::new(),
            children: Vec::new(),
        }
    }
}

/// A node of the collection tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Item {
    Folder(Folder),
    Request(Request),
}

impl Item {
    pub fn id(&self) -> &Id {
        match self {
            Item::Folder(f) => &f.id,
            Item::Request(r) => &r.id,
        }
    }

    pub fn name(&self) -> &str {
        match self {
            Item::Folder(f) => &f.name,
            Item::Request(r) => &r.name,
        }
    }
}

/// A named set of variables. Sub-environments override the base environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Environment {
    pub id: Id,
    pub name: String,
    #[serde(default)]
    pub data: Map<String, Value>,
    /// Keys whose values are secret: masked in the UI and stored outside git.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub secrets: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

impl Environment {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Id::new("env"),
            name: name.into(),
            data: Map::new(),
            secrets: Vec::new(),
            color: None,
        }
    }

    pub fn with(mut self, key: &str, value: impl Into<Value>) -> Self {
        self.data.insert(key.to_owned(), value.into());
        self
    }
}

/// Proxy configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProxySettings {
    #[serde(default)]
    pub enabled: bool,
    /// e.g. `http://proxy:3128` or `socks5://127.0.0.1:1080`.
    #[serde(default)]
    pub http: String,
    #[serde(default)]
    pub https: String,
    /// Comma-separated host suffixes that bypass the proxy.
    #[serde(default)]
    pub no_proxy: String,
}

/// A TLS client certificate for a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClientCertificate {
    /// Host (optionally with port) this certificate is presented to.
    pub host: String,
    /// PEM file containing the certificate chain.
    pub cert_path: String,
    /// PEM file containing the private key (may be the same file).
    pub key_path: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

/// Workspace-wide transport settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceSettings {
    #[serde(default = "yes")]
    pub follow_redirects: bool,
    #[serde(default = "default_max_redirects")]
    pub max_redirects: u32,
    #[serde(default = "yes")]
    pub verify_tls: bool,
    /// Zero disables the timeout.
    #[serde(default = "default_timeout")]
    pub timeout_ms: u64,
    #[serde(default)]
    pub proxy: ProxySettings,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub client_certificates: Vec<ClientCertificate>,
    /// Extra PEM CA bundle trusted in addition to the system roots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ca_certificate: Option<String>,
    /// Maximum response body bytes kept in memory.
    #[serde(default = "default_max_body")]
    pub max_response_bytes: u64,
}

impl Default for WorkspaceSettings {
    fn default() -> Self {
        Self {
            follow_redirects: true,
            max_redirects: default_max_redirects(),
            verify_tls: true,
            timeout_ms: default_timeout(),
            proxy: ProxySettings::default(),
            client_certificates: Vec::new(),
            ca_certificate: None,
            max_response_bytes: default_max_body(),
        }
    }
}

/// A workspace: the collection tree, environments and settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: Id,
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default)]
    pub items: Vec<Item>,
    /// The base environment, always applied.
    pub base_environment: Environment,
    /// Selectable sub-environments.
    #[serde(default)]
    pub environments: Vec<Environment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_environment: Option<Id>,
    #[serde(default)]
    pub settings: WorkspaceSettings,
}

impl Workspace {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: Id::new("wrk"),
            name: name.into(),
            description: String::new(),
            items: Vec::new(),
            base_environment: Environment {
                id: Id::new("env"),
                name: "Base Environment".into(),
                data: Map::new(),
                secrets: Vec::new(),
                color: None,
            },
            environments: Vec::new(),
            active_environment: None,
            settings: WorkspaceSettings::default(),
        }
    }

    /// Depth-first iterator over every request with its folder ancestry
    /// (outermost first).
    pub fn requests(&self) -> Vec<(Vec<&Folder>, &Request)> {
        fn walk<'a>(
            items: &'a [Item],
            path: &mut Vec<&'a Folder>,
            out: &mut Vec<(Vec<&'a Folder>, &'a Request)>,
        ) {
            for item in items {
                match item {
                    Item::Request(r) => out.push((path.clone(), r)),
                    Item::Folder(f) => {
                        path.push(f);
                        walk(&f.children, path, out);
                        path.pop();
                    }
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.items, &mut Vec::new(), &mut out);
        out
    }

    pub fn find_request(&self, id: &Id) -> Option<(Vec<&Folder>, &Request)> {
        self.requests().into_iter().find(|(_, r)| &r.id == id)
    }

    pub fn find_item(&self, id: &Id) -> Option<&Item> {
        fn walk<'a>(items: &'a [Item], id: &Id) -> Option<&'a Item> {
            items.iter().find_map(|item| {
                if item.id() == id {
                    return Some(item);
                }
                match item {
                    Item::Folder(f) => walk(&f.children, id),
                    Item::Request(_) => None,
                }
            })
        }
        walk(&self.items, id)
    }

    pub fn find_item_mut(&mut self, id: &Id) -> Option<&mut Item> {
        fn walk<'a>(items: &'a mut [Item], id: &Id) -> Option<&'a mut Item> {
            for item in items.iter_mut() {
                if item.id() == id {
                    return Some(item);
                }
                if let Item::Folder(f) = item
                    && let Some(found) = walk(&mut f.children, id)
                {
                    return Some(found);
                }
            }
            None
        }
        walk(&mut self.items, id)
    }

    pub fn active_environment(&self) -> Option<&Environment> {
        let id = self.active_environment.as_ref()?;
        self.environments.iter().find(|e| &e.id == id)
    }
}

fn yes() -> bool {
    true
}

fn is_true(value: &bool) -> bool {
    *value
}

fn bearer_prefix() -> String {
    "Bearer".into()
}

fn default_max_redirects() -> u32 {
    10
}

fn default_timeout() -> u64 {
    30_000
}

fn default_max_body() -> u64 {
    64 * 1024 * 1024
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_round_trips_and_preserves_custom() {
        assert_eq!(Method::from("post".to_owned()), Method::Post);
        let custom = Method::from("propfind".to_owned());
        assert_eq!(custom.as_str(), "PROPFIND");
        let json = serde_json::to_string(&Method::Patch).unwrap();
        assert_eq!(json, "\"PATCH\"");
    }

    #[test]
    fn ids_are_file_name_safe() {
        assert!(Id::new("req").is_valid());
        assert!(!Id::from("../etc").is_valid());
        assert!(!Id::from("").is_valid());
        assert!(!Id::from(".hidden").is_valid());
    }

    #[test]
    fn finds_nested_requests_with_ancestry() {
        let mut ws = Workspace::new("Demo");
        let mut folder = Folder::new("Users");
        let req = Request::new("List", Method::Get, "https://example.com/users");
        let id = req.id.clone();
        folder.children.push(Item::Request(req));
        ws.items.push(Item::Folder(folder));
        let (path, found) = ws.find_request(&id).unwrap();
        assert_eq!(path.len(), 1);
        assert_eq!(path[0].name, "Users");
        assert_eq!(found.name, "List");
        if let Some(Item::Request(r)) = ws.find_item_mut(&id) {
            r.name = "All users".into();
        }
        assert_eq!(ws.find_request(&id).unwrap().1.name, "All users");
    }

    #[test]
    fn body_serializes_with_kind_tag() {
        let body = Body::Json { text: "{}".into() };
        let json = serde_json::to_value(&body).unwrap();
        assert_eq!(json["kind"], "json");
        assert_eq!(body.default_content_type(), Some("application/json"));
    }
}
