//! Editing rules of the request model: what happens when the user types a query into the
//! URL, switches the body type or the auth scheme, picks a gRPC method or beautifies JSON.
//! The views call these; they hold no rules of their own.

use crate::assertions::{Operator, Source};
use crate::model::{
    ApiKeyLocation, Auth, Body, KeyValue, Method, MultipartField, MultipartValue, OAuth2Config,
    OAuth2Grant, Protocol, Request,
};
use serde_json::Value;
use std::fmt;

/// What a request talks to: an HTTP method, or a streaming or RPC protocol. The editor's
/// first control picks one.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    Http(Method),
    WebSocket,
    Sse,
    Grpc,
}

impl Target {
    /// Every target in menu order: the common methods, then the protocols.
    pub fn all() -> Vec<Target> {
        Method::COMMON
            .into_iter()
            .map(Target::Http)
            .chain([Target::WebSocket, Target::Sse, Target::Grpc])
            .collect()
    }

    /// Index of the first protocol in [`all`](Self::all), where a menu separates them.
    pub fn protocols_start() -> usize {
        Method::COMMON.len()
    }

    pub fn of(request: &Request) -> Self {
        match request.protocol {
            Protocol::Http => Target::Http(request.method.clone()),
            Protocol::WebSocket => Target::WebSocket,
            Protocol::Sse => Target::Sse,
            Protocol::Grpc => Target::Grpc,
        }
    }

    /// Short label for tags and the picker: the method, `WS`, `SSE` or `gRPC`.
    pub fn short(&self) -> String {
        match self {
            Target::Http(m) => m.to_string(),
            Target::WebSocket => "WS".into(),
            Target::Sse => "SSE".into(),
            Target::Grpc => "gRPC".into(),
        }
    }
}

impl fmt::Display for Target {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.short())
    }
}

/// Points `request` at `target`; returns whether it changed. A protocol request keeps GET
/// as its method.
pub fn set_target(request: &mut Request, target: Target) -> bool {
    if Target::of(request) == target {
        return false;
    }
    match target {
        Target::Http(method) => {
            request.method = method;
            request.protocol = Protocol::Http;
        }
        Target::WebSocket | Target::Sse | Target::Grpc => {
            request.method = Method::Get;
            request.protocol = match target {
                Target::WebSocket => Protocol::WebSocket,
                Target::Sse => Protocol::Sse,
                _ => Protocol::Grpc,
            };
        }
    }
    true
}

/// The hint of an empty URL for each protocol.
pub fn url_hint(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Http => "Enter a URL, paste a curl command, or use {{ base_url }}",
        Protocol::WebSocket => "ws://host/socket or wss://…",
        Protocol::Sse => "https://host/events",
        Protocol::Grpc => "grpc://host:50051 or grpcs://host:443",
    }
}

/// The kinds of request body, in the order the editor offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BodyKind {
    Json,
    Xml,
    Text,
    Form,
    Multipart,
    GraphQl,
    Binary,
    None,
}

impl BodyKind {
    /// Every kind, in menu order.
    pub const ALL: [BodyKind; 8] = [
        BodyKind::Json,
        BodyKind::Xml,
        BodyKind::Text,
        BodyKind::Form,
        BodyKind::Multipart,
        BodyKind::GraphQl,
        BodyKind::Binary,
        BodyKind::None,
    ];

    /// The kinds a WebSocket or gRPC message can have.
    pub const MESSAGE: [BodyKind; 2] = [BodyKind::Json, BodyKind::Text];

    pub fn of(body: &Body) -> Self {
        match body {
            Body::None => BodyKind::None,
            Body::Json { .. } => BodyKind::Json,
            Body::Xml { .. } => BodyKind::Xml,
            Body::Text { .. } => BodyKind::Text,
            Body::FormUrlEncoded { .. } => BodyKind::Form,
            Body::Multipart { .. } => BodyKind::Multipart,
            Body::Binary { .. } => BodyKind::Binary,
            Body::GraphQl { .. } => BodyKind::GraphQl,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            BodyKind::None => "No Body",
            BodyKind::Json => "JSON",
            BodyKind::Xml => "XML",
            BodyKind::Text => "Plain Text",
            BodyKind::Form => "Form URL Encoded",
            BodyKind::Multipart => "Multipart Form",
            BodyKind::Binary => "Binary File",
            BodyKind::GraphQl => "GraphQL",
        }
    }

    /// `body` as this kind, carrying text and fields over where they fit.
    pub fn convert(self, body: &Body) -> Body {
        let text = match body {
            Body::Json { text } | Body::Xml { text } | Body::Text { text, .. } => text.clone(),
            _ => String::new(),
        };
        let fields = match body {
            Body::FormUrlEncoded { fields } => fields.clone(),
            Body::Multipart { fields } => fields
                .iter()
                .filter_map(|f| match &f.value {
                    MultipartValue::Text { value } => Some(KeyValue {
                        enabled: f.enabled,
                        ..KeyValue::new(&f.name, value)
                    }),
                    MultipartValue::File { .. } => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        match self {
            BodyKind::None => Body::None,
            BodyKind::Json => Body::Json { text },
            BodyKind::Xml => Body::Xml { text },
            BodyKind::Text => Body::Text {
                text,
                content_type: None,
            },
            BodyKind::Form => Body::FormUrlEncoded { fields },
            BodyKind::Multipart => Body::Multipart {
                fields: fields
                    .into_iter()
                    .map(|kv| MultipartField {
                        name: kv.name,
                        value: MultipartValue::Text { value: kv.value },
                        enabled: kv.enabled,
                    })
                    .collect(),
            },
            BodyKind::Binary => Body::Binary {
                path: String::new(),
            },
            BodyKind::GraphQl => Body::GraphQl {
                query: if text.is_empty() {
                    "query {\n  \n}".into()
                } else {
                    text
                },
                variables: String::new(),
                operation_name: None,
            },
        }
    }
}

impl fmt::Display for BodyKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Switches `body` to `kind`; returns whether it changed.
pub fn set_body_kind(body: &mut Body, kind: BodyKind) -> bool {
    if BodyKind::of(body) == kind {
        return false;
    }
    *body = kind.convert(body);
    true
}

/// A WebSocket or gRPC message is JSON or text; anything else becomes an empty JSON
/// message. Returns whether it changed.
pub fn ensure_message_body(body: &mut Body) -> bool {
    if BodyKind::MESSAGE.contains(&BodyKind::of(body)) {
        return false;
    }
    *body = Body::Json {
        text: String::new(),
    };
    true
}

/// The auth schemes, in the order the editor offers them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AuthKind {
    None,
    Inherit,
    Basic,
    Bearer,
    ApiKey,
    Digest,
    OAuth2,
    AwsSigV4,
}

impl AuthKind {
    pub const ALL: [AuthKind; 8] = [
        AuthKind::None,
        AuthKind::Inherit,
        AuthKind::Basic,
        AuthKind::Bearer,
        AuthKind::ApiKey,
        AuthKind::Digest,
        AuthKind::OAuth2,
        AuthKind::AwsSigV4,
    ];

    pub fn of(auth: &Auth) -> Self {
        match auth {
            Auth::None => AuthKind::None,
            Auth::Inherit => AuthKind::Inherit,
            Auth::Basic { .. } => AuthKind::Basic,
            Auth::Bearer { .. } => AuthKind::Bearer,
            Auth::ApiKey { .. } => AuthKind::ApiKey,
            Auth::Digest { .. } => AuthKind::Digest,
            Auth::OAuth2(_) => AuthKind::OAuth2,
            Auth::AwsSigV4 { .. } => AuthKind::AwsSigV4,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            AuthKind::None => "No Auth",
            AuthKind::Inherit => "Inherit",
            AuthKind::Basic => "Basic",
            AuthKind::Bearer => "Bearer",
            AuthKind::ApiKey => "API Key",
            AuthKind::Digest => "Digest",
            AuthKind::OAuth2 => "OAuth 2.0",
            AuthKind::AwsSigV4 => "AWS SigV4",
        }
    }

    /// A new configuration of this scheme, with the usual defaults filled in.
    pub fn default_auth(self) -> Auth {
        match self {
            AuthKind::None => Auth::None,
            AuthKind::Inherit => Auth::Inherit,
            AuthKind::Basic => Auth::Basic {
                username: String::new(),
                password: String::new(),
            },
            AuthKind::Bearer => Auth::Bearer {
                token: String::new(),
                prefix: "Bearer".into(),
            },
            AuthKind::ApiKey => Auth::ApiKey {
                key: "X-API-Key".into(),
                value: String::new(),
                location: ApiKeyLocation::Header,
            },
            AuthKind::Digest => Auth::Digest {
                username: String::new(),
                password: String::new(),
            },
            AuthKind::OAuth2 => Auth::OAuth2(Box::new(OAuth2Config {
                header_prefix: "Bearer".into(),
                use_pkce: true,
                redirect_url: "http://127.0.0.1:8765/callback".into(),
                ..Default::default()
            })),
            AuthKind::AwsSigV4 => Auth::AwsSigV4 {
                access_key_id: String::new(),
                secret_access_key: String::new(),
                session_token: String::new(),
                region: "us-east-1".into(),
                service: String::new(),
            },
        }
    }
}

impl fmt::Display for AuthKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// The OAuth 2.0 grants, in menu order, with their labels.
pub const OAUTH2_GRANTS: [(OAuth2Grant, &str); 4] = [
    (OAuth2Grant::AuthorizationCode, "Authorization Code"),
    (OAuth2Grant::ClientCredentials, "Client Credentials"),
    (OAuth2Grant::Password, "Password"),
    (OAuth2Grant::Implicit, "Implicit"),
];

/// What an assertion checks, without its argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SourceKind {
    Status,
    Header,
    JsonPath,
    Body,
    Time,
    Size,
}

impl SourceKind {
    pub const ALL: [SourceKind; 6] = [
        SourceKind::Status,
        SourceKind::Header,
        SourceKind::JsonPath,
        SourceKind::Body,
        SourceKind::Time,
        SourceKind::Size,
    ];

    pub fn of(source: &Source) -> Self {
        match source {
            Source::Status => SourceKind::Status,
            Source::Header(_) => SourceKind::Header,
            Source::JsonPath(_) => SourceKind::JsonPath,
            Source::Body => SourceKind::Body,
            Source::ResponseTimeMs => SourceKind::Time,
            Source::BodySize => SourceKind::Size,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SourceKind::Status => "Status",
            SourceKind::Header => "Header",
            SourceKind::JsonPath => "JSONPath",
            SourceKind::Body => "Body",
            SourceKind::Time => "Time (ms)",
            SourceKind::Size => "Size (bytes)",
        }
    }

    /// A new source of this kind, with a typical argument.
    pub fn source(self) -> Source {
        match self {
            SourceKind::Status => Source::Status,
            SourceKind::Header => Source::Header("content-type".into()),
            SourceKind::JsonPath => Source::JsonPath("$.".into()),
            SourceKind::Body => Source::Body,
            SourceKind::Time => Source::ResponseTimeMs,
            SourceKind::Size => Source::BodySize,
        }
    }
}

impl fmt::Display for SourceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// Switches an assertion's source to `kind`; returns whether it changed.
pub fn set_source_kind(source: &mut Source, kind: SourceKind) -> bool {
    if SourceKind::of(source) == kind {
        return false;
    }
    *source = kind.source();
    true
}

impl fmt::Display for Operator {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

/// The new assertion the editor adds: the status is 200.
pub fn new_assertion() -> crate::assertions::Assertion {
    crate::assertions::Assertion::new(Source::Status, Operator::Equals, "200")
}

/// A new multipart field, text or file.
pub fn new_multipart_field(file: bool) -> MultipartField {
    MultipartField {
        name: String::new(),
        value: if file {
            MultipartValue::File {
                path: String::new(),
                content_type: None,
            }
        } else {
            MultipartValue::Text {
                value: String::new(),
            }
        },
        enabled: true,
    }
}

/// Switches a multipart field between text and file; the name stays.
pub fn set_multipart_file(field: &mut MultipartField, file: bool) -> bool {
    let is_file = matches!(field.value, MultipartValue::File { .. });
    if is_file == file {
        return false;
    }
    field.value = new_multipart_field(file).value;
    true
}

/// Switches `auth` to `kind` with its defaults; returns whether it changed.
pub fn set_auth_kind(auth: &mut Auth, kind: AuthKind) -> bool {
    if AuthKind::of(auth) == kind {
        return false;
    }
    *auth = kind.default_auth();
    true
}

/// Moves a `?a=b&c` typed into the URL into the params table. Waits while a `{{` in the
/// query is still open. Returns whether anything moved.
pub fn take_query_into_params(request: &mut Request) -> bool {
    let Some((base, query)) = request.url.split_once('?') else {
        return false;
    };
    if query.contains("{{") && !query.contains("}}") {
        return false;
    }
    let base = base.to_owned();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        request.params.push(KeyValue::new(name, value));
    }
    request.url = base;
    true
}

/// Picks gRPC method `name`. An empty message body is filled with the method's request
/// `template`; a message the user wrote is kept.
pub fn pick_grpc_method(request: &mut Request, name: &str, template: &str) {
    request.grpc.method = name.to_owned();
    let empty = match &request.body {
        Body::None => true,
        Body::Json { text } => text.trim().is_empty(),
        _ => false,
    };
    if empty {
        request.body = Body::Json {
            text: template.to_owned(),
        };
    }
}

/// `text` re-indented, if it is valid JSON.
pub fn beautify_json(text: &str) -> Option<String> {
    let value: Value = serde_json::from_str(text).ok()?;
    serde_json::to_string_pretty(&value).ok()
}

/// Why a JSON body won't parse, if it doesn't. Bodies with templates are not checked: they
/// are only JSON once rendered.
pub fn json_problem(text: &str) -> Option<String> {
    if text.trim().is_empty() || crate::template::has_templates(text) {
        return None;
    }
    serde_json::from_str::<Value>(text)
        .err()
        .map(|e| format!("Invalid JSON: {e}"))
}

/// The timeout typed into a field: a number of milliseconds, or the workspace default when
/// empty or not a number.
pub fn parse_timeout_ms(text: &str) -> Option<u64> {
    text.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_typed_query_into_params() {
        let mut r = Request::new("r", Method::Get, "https://x/a?b=1&c");
        assert!(take_query_into_params(&mut r));
        assert_eq!(r.url, "https://x/a");
        assert_eq!(
            r.params,
            vec![KeyValue::new("b", "1"), KeyValue::new("c", "")]
        );
        let mut typing = Request::new("r", Method::Get, "https://x/a?b={{ va");
        assert!(!take_query_into_params(&mut typing));
        assert!(typing.params.is_empty());
    }

    #[test]
    fn converting_body_kinds_keeps_content() {
        let json = Body::Json { text: "{}".into() };
        assert_eq!(
            BodyKind::Text.convert(&json),
            Body::Text {
                text: "{}".into(),
                content_type: None
            }
        );
        let form = Body::FormUrlEncoded {
            fields: vec![KeyValue::new("a", "1")],
        };
        let Body::Multipart { fields } = BodyKind::Multipart.convert(&form) else {
            panic!("multipart")
        };
        assert_eq!(fields[0].name, "a");
        assert_eq!(
            BodyKind::of(&BodyKind::GraphQl.convert(&Body::None)),
            BodyKind::GraphQl
        );
        let mut body = Body::None;
        assert!(set_body_kind(&mut body, BodyKind::Json));
        assert!(!set_body_kind(&mut body, BodyKind::Json));
    }

    #[test]
    fn messages_are_json_or_text() {
        let mut body = Body::FormUrlEncoded { fields: vec![] };
        assert!(ensure_message_body(&mut body));
        assert_eq!(BodyKind::of(&body), BodyKind::Json);
        assert!(!ensure_message_body(&mut body));
    }

    #[test]
    fn auth_kinds_round_trip_with_defaults() {
        for kind in AuthKind::ALL {
            assert_eq!(AuthKind::of(&kind.default_auth()), kind);
        }
        let mut auth = Auth::None;
        assert!(set_auth_kind(&mut auth, AuthKind::Bearer));
        assert!(matches!(&auth, Auth::Bearer { prefix, .. } if prefix == "Bearer"));
        assert!(!set_auth_kind(&mut auth, AuthKind::Bearer));
    }

    #[test]
    fn picking_a_grpc_method_fills_only_an_empty_message() {
        let mut r = Request::new("r", Method::Post, "grpc://x");
        pick_grpc_method(&mut r, "pkg.Echo/Say", "{\"text\": \"\"}");
        assert_eq!(r.grpc.method, "pkg.Echo/Say");
        assert_eq!(
            r.body,
            Body::Json {
                text: "{\"text\": \"\"}".into()
            }
        );
        r.body = Body::Json {
            text: "{\"text\": \"mine\"}".into(),
        };
        pick_grpc_method(&mut r, "pkg.Echo/Other", "{}");
        assert_eq!(
            r.body,
            Body::Json {
                text: "{\"text\": \"mine\"}".into()
            }
        );
    }

    #[test]
    fn targets_switch_method_and_protocol() {
        let mut r = Request::new("r", Method::Get, "https://x");
        assert!(set_target(&mut r, Target::Http(Method::Post)));
        assert_eq!(
            (r.method.clone(), r.protocol),
            (Method::Post, Protocol::Http)
        );
        assert!(set_target(&mut r, Target::WebSocket));
        assert_eq!(
            (r.method.clone(), r.protocol),
            (Method::Get, Protocol::WebSocket)
        );
        assert!(!set_target(&mut r, Target::WebSocket));
        assert_eq!(Target::of(&r).short(), "WS");
        assert_eq!(Target::all()[Target::protocols_start()], Target::WebSocket);
    }

    #[test]
    fn assertion_sources_and_multipart_fields() {
        let mut source = Source::Status;
        assert!(set_source_kind(&mut source, SourceKind::JsonPath));
        assert_eq!(source, Source::JsonPath("$.".into()));
        for kind in SourceKind::ALL {
            assert_eq!(SourceKind::of(&kind.source()), kind);
        }
        let mut field = new_multipart_field(false);
        field.name = "avatar".into();
        assert!(set_multipart_file(&mut field, true));
        assert!(matches!(field.value, MultipartValue::File { .. }));
        assert_eq!(field.name, "avatar");
        assert!(!set_multipart_file(&mut field, true));
    }

    #[test]
    fn json_helpers() {
        assert_eq!(
            beautify_json("{\"a\":1}").as_deref(),
            Some("{\n  \"a\": 1\n}")
        );
        assert_eq!(beautify_json("{"), None);
        assert!(json_problem("{").is_some());
        assert!(json_problem("{\"a\": {{ x }}}").is_none());
        assert!(json_problem("").is_none());
        assert_eq!(parse_timeout_ms(" 250 "), Some(250));
        assert_eq!(parse_timeout_ms(""), None);
    }
}
