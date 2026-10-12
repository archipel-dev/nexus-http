//! Executed request/response records: status, headers, body, timings and the
//! timeline (the raw exchange as sent and received).

use crate::model::Id;
use serde::{Deserialize, Serialize};

/// Phase durations of one exchange in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
pub struct Timings {
    /// Template rendering, auth and body encoding.
    pub prepare_ms: f64,
    /// Request sent until response headers arrived (includes DNS, connect, TLS).
    pub wait_ms: f64,
    /// Response headers until the last body byte.
    pub download_ms: f64,
    pub total_ms: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimelineKind {
    Info,
    RequestHeader,
    RequestBody,
    ResponseHeader,
    ResponseBody,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TimelineEntry {
    pub kind: TimelineKind,
    pub text: String,
}

impl TimelineEntry {
    pub fn new(kind: TimelineKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

/// A received response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Response {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<Id>,
    pub method: String,
    /// Final URL after redirects.
    pub url: String,
    pub status: u16,
    pub reason: String,
    pub http_version: String,
    pub headers: Vec<(String, String)>,
    #[serde(with = "body_serde")]
    pub body: Vec<u8>,
    /// The body exceeded the configured limit and was cut.
    #[serde(default)]
    pub truncated: bool,
    pub timings: Timings,
    #[serde(default)]
    pub timeline: Vec<TimelineEntry>,
    /// Number of redirects followed.
    #[serde(default)]
    pub redirects: u32,
    /// Unix milliseconds when the request started.
    pub started_at: u64,
}

impl Response {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn headers_named<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        self.headers
            .iter()
            .filter(move |(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    pub fn body_text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    /// The file name to save this body under for the request `request_name`: the name with
    /// everything but letters and digits as `-`, then `.response.json`, `.bin` or `.txt`.
    pub fn file_name(&self, request_name: &str) -> String {
        let stem: String = request_name
            .chars()
            .map(|c| if c.is_alphanumeric() { c } else { '-' })
            .collect();
        let stem = if stem.is_empty() {
            "response".to_owned()
        } else {
            stem
        };
        let ext = match self.body_kind() {
            BodyKind::Json => "json",
            BodyKind::Image | BodyKind::Binary => "bin",
            _ => "txt",
        };
        format!("{stem}.response.{ext}")
    }

    pub fn body_kind(&self) -> BodyKind {
        BodyKind::detect(self.content_type(), &self.body)
    }

    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// Body pretty-printed when it is JSON; otherwise the text as is.
    pub fn pretty_body(&self) -> String {
        match self.body_kind() {
            BodyKind::Json => serde_json::from_slice::<serde_json::Value>(&self.body)
                .ok()
                .and_then(|v| serde_json::to_string_pretty(&v).ok())
                .unwrap_or_else(|| self.body_text()),
            _ => self.body_text(),
        }
    }
}

/// Coarse content classification for choosing a viewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyKind {
    Json,
    Xml,
    Html,
    Text,
    Image,
    Binary,
    Empty,
}

impl BodyKind {
    pub fn detect(content_type: Option<&str>, body: &[u8]) -> Self {
        if body.is_empty() {
            return BodyKind::Empty;
        }
        let ct = content_type.unwrap_or("").to_ascii_lowercase();
        if ct.contains("json") {
            return BodyKind::Json;
        }
        if ct.contains("html") {
            return BodyKind::Html;
        }
        if ct.contains("xml") {
            return BodyKind::Xml;
        }
        if ct.starts_with("image/") {
            return BodyKind::Image;
        }
        if ct.starts_with("text/") || ct.contains("javascript") || ct.contains("x-www-form") {
            return BodyKind::Text;
        }
        let Ok(text) = std::str::from_utf8(body) else {
            return BodyKind::Binary;
        };
        let trimmed = text.trim_start();
        if (trimmed.starts_with('{') || trimmed.starts_with('['))
            && serde_json::from_str::<serde_json::Value>(text).is_ok()
        {
            BodyKind::Json
        } else if trimmed.starts_with("<!DOCTYPE html") || trimmed.starts_with("<html") {
            BodyKind::Html
        } else if trimmed.starts_with('<') {
            BodyKind::Xml
        } else {
            BodyKind::Text
        }
    }
}

/// Human-readable byte size (`512 B`, `1.4 KB`, `3.2 MB`).
pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

/// Human-readable duration (`87 ms`, `1.24 s`).
pub fn format_duration(ms: f64) -> String {
    if ms < 1000.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{:.2} s", ms / 1000.0)
    }
}

/// Bodies are stored as UTF-8 text when possible, otherwise base64.
mod body_serde {
    use base64::Engine as _;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    #[derive(Serialize, Deserialize)]
    #[serde(tag = "encoding", content = "data", rename_all = "snake_case")]
    enum Stored {
        Utf8(String),
        Base64(String),
    }

    pub fn serialize<S: Serializer>(body: &[u8], s: S) -> Result<S::Ok, S::Error> {
        match std::str::from_utf8(body) {
            Ok(text) => Stored::Utf8(text.to_owned()),
            Err(_) => Stored::Base64(base64::engine::general_purpose::STANDARD.encode(body)),
        }
        .serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        match Stored::deserialize(d)? {
            Stored::Utf8(text) => Ok(text.into_bytes()),
            Stored::Base64(data) => base64::engine::general_purpose::STANDARD
                .decode(data)
                .map_err(serde::de::Error::custom),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    pub(crate) fn sample(body: &[u8], content_type: &str) -> Response {
        Response {
            request_id: None,
            method: "GET".into(),
            url: "http://x/".into(),
            status: 200,
            reason: "OK".into(),
            http_version: "HTTP/1.1".into(),
            headers: vec![("Content-Type".into(), content_type.into())],
            body: body.to_vec(),
            truncated: false,
            timings: Timings::default(),
            timeline: Vec::new(),
            redirects: 0,
            started_at: 0,
        }
    }

    #[test]
    fn file_names_follow_the_request_and_the_body() {
        assert_eq!(
            sample(b"{}", "application/json").file_name("List products"),
            "List-products.response.json"
        );
        assert_eq!(
            sample(&[0xff, 0xfe, 0], "").file_name("img/1"),
            "img-1.response.bin"
        );
        assert_eq!(
            sample(b"ok", "text/plain").file_name(""),
            "response.response.txt"
        );
    }

    #[test]
    fn detects_body_kinds() {
        assert_eq!(sample(b"{\"a\":1}", "").body_kind(), BodyKind::Json);
        assert_eq!(
            sample(b"<a/>", "application/xml").body_kind(),
            BodyKind::Xml
        );
        assert_eq!(sample(b"<html></html>", "").body_kind(), BodyKind::Html);
        assert_eq!(sample(&[0xff, 0xfe, 0], "").body_kind(), BodyKind::Binary);
        assert_eq!(sample(b"", "text/plain").body_kind(), BodyKind::Empty);
    }

    #[test]
    fn body_round_trips_binary_and_text() {
        for body in [b"hello".to_vec(), vec![0xff, 0x00, 0x10]] {
            let r = sample(&body, "x/y");
            let json = serde_json::to_string(&r).unwrap();
            let back: Response = serde_json::from_str(&json).unwrap();
            assert_eq!(back.body, body);
        }
    }

    #[test]
    fn pretty_prints_json_and_formats_sizes() {
        let r = sample(b"{\"a\":[1]}", "application/json");
        assert_eq!(r.pretty_body(), "{\n  \"a\": [\n    1\n  ]\n}");
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_duration(87.2), "87 ms");
        assert_eq!(format_duration(1240.0), "1.24 s");
    }
}
