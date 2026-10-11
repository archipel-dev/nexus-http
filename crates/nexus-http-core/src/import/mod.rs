//! Importers: curl commands, Insomnia v4 (JSON) and v5 (YAML) exports, Postman
//! v2.1 collections, OpenAPI 3 documents and nexus-http's own export format.
//!
//! [`import`] detects the format. Every importer works on untrusted input: it never
//! panics, skips what it cannot map and reports it in [`Imported::warnings`].

pub mod curl;
pub mod http_file;
pub mod insomnia;
pub mod openapi;
pub mod postman;

use crate::model::{Item, KeyValue, Workspace};
use serde_json::Value;
use thiserror::Error;

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ImportError {
    #[error(
        "unrecognized format: expected a curl command, Insomnia, Postman, OpenAPI or nexus-http export"
    )]
    UnknownFormat,
    #[error("invalid {format}: {reason}")]
    Invalid {
        format: &'static str,
        reason: String,
    },
}

impl ImportError {
    pub(crate) fn invalid(format: &'static str, reason: impl Into<String>) -> Self {
        Self::Invalid {
            format,
            reason: reason.into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Curl,
    HttpFile,
    InsomniaV4,
    InsomniaV5,
    PostmanV21,
    OpenApi3,
    Nexus,
}

impl Format {
    pub fn label(self) -> &'static str {
        match self {
            Format::Curl => "curl",
            Format::HttpFile => ".http file",
            Format::InsomniaV4 => "Insomnia v4",
            Format::InsomniaV5 => "Insomnia v5",
            Format::PostmanV21 => "Postman v2.1",
            Format::OpenApi3 => "OpenAPI 3",
            Format::Nexus => "nexus-http",
        }
    }
}

/// The result of an import: a new workspace plus anything that was skipped.
#[derive(Debug, Clone, PartialEq)]
pub struct Imported {
    pub format: Format,
    pub workspace: Workspace,
    pub warnings: Vec<String>,
}

/// Single-file export of a workspace (`nexus-http` format).
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct NexusExport {
    nexus_export: u32,
    workspace: Workspace,
}

/// Serializes a workspace to the single-file nexus-http export format.
/// Secret values are blanked.
pub fn export(workspace: &Workspace) -> String {
    let mut ws = workspace.clone();
    for env in std::iter::once(&mut ws.base_environment).chain(ws.environments.iter_mut()) {
        for key in env.secrets.clone() {
            if let Some(v) = env.data.get_mut(&key) {
                *v = Value::String(String::new());
            }
        }
    }
    let mut text = serde_json::to_string_pretty(&NexusExport {
        nexus_export: 1,
        workspace: ws,
    })
    .unwrap_or_default();
    text.push('\n');
    text
}

/// Detects the format of `text` and imports it.
pub fn import(text: &str) -> Result<Imported, ImportError> {
    let trimmed = text.trim_start();
    if trimmed.starts_with("curl ") || trimmed.starts_with("curl\t") || trimmed == "curl" {
        let request = curl::parse(text)?;
        let mut workspace = Workspace::new("Imported from curl");
        workspace.items.push(Item::Request(request));
        return Ok(Imported {
            format: Format::Curl,
            workspace,
            warnings: Vec::new(),
        });
    }
    let value = parse_document(text)?;
    if value.get("nexus_export").is_some() {
        let export: NexusExport = serde_json::from_value(value)
            .map_err(|e| ImportError::invalid("nexus-http export", e.to_string()))?;
        return Ok(Imported {
            format: Format::Nexus,
            workspace: export.workspace,
            warnings: Vec::new(),
        });
    }
    if value.get("_type").and_then(Value::as_str) == Some("export") {
        return insomnia::import_v4(&value);
    }
    if value
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|t| t.contains("insomnia.rest/5"))
    {
        return insomnia::import_v5(&value);
    }
    if value
        .pointer("/info/schema")
        .and_then(Value::as_str)
        .is_some_and(|s| s.contains("getpostman.com"))
        || (value.get("info").is_some() && value.get("item").is_some())
    {
        return postman::import(&value);
    }
    if value
        .get("openapi")
        .and_then(Value::as_str)
        .is_some_and(|v| v.starts_with('3'))
    {
        return openapi::import(&value);
    }
    if value.get("swagger").is_some() {
        return Err(ImportError::invalid(
            "OpenAPI",
            "Swagger 2.0 is not supported; convert it to OpenAPI 3 first",
        ));
    }
    Err(ImportError::UnknownFormat)
}

/// Parses JSON, falling back to YAML.
pub(crate) fn parse_document(text: &str) -> Result<Value, ImportError> {
    match serde_json::from_str::<Value>(text) {
        Ok(v) => Ok(v),
        Err(json_err) => {
            let trimmed = text.trim_start();
            if trimmed.starts_with('{') || trimmed.starts_with('[') {
                return Err(ImportError::invalid("JSON", json_err.to_string()));
            }
            yaml_serde::from_str::<Value>(text)
                .map_err(|e| ImportError::invalid("YAML", e.to_string()))
        }
    }
}

pub(crate) fn str_of<'a>(value: &'a Value, key: &str) -> &'a str {
    value.get(key).and_then(Value::as_str).unwrap_or("")
}

/// Splits a URL's query string into parameters, returning the URL without it.
/// Template expressions are left intact.
pub(crate) fn split_query(url: &str) -> (String, Vec<KeyValue>) {
    let Some((base, query)) = url.split_once('?') else {
        return (url.to_owned(), Vec::new());
    };
    let (query, fragment) = match query.split_once('#') {
        Some((q, f)) => (q, Some(f)),
        None => (query, None),
    };
    let params = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            KeyValue::new(decode_component(name), decode_component(value))
        })
        .collect();
    let base = match fragment {
        Some(f) => format!("{base}#{f}"),
        None => base.to_owned(),
    };
    (base, params)
}

fn decode_component(s: &str) -> String {
    if s.contains("{{") || s.contains("{%") {
        return s.to_owned();
    }
    url::form_urlencoded::parse(format!("x={s}").as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| s.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Method, Request};

    #[test]
    fn splits_query_keeping_templates() {
        let (base, params) = split_query("https://x/a?q=a%20b&t={{ _.tok }}&flag#frag");
        assert_eq!(base, "https://x/a#frag");
        assert_eq!(params[0], KeyValue::new("q", "a b"));
        assert_eq!(params[1], KeyValue::new("t", "{{ _.tok }}"));
        assert_eq!(params[2], KeyValue::new("flag", ""));
    }

    #[test]
    fn export_round_trips_and_blanks_secrets() {
        let mut ws = Workspace::new("W");
        ws.base_environment = ws.base_environment.clone().with("key", "secret");
        ws.base_environment.secrets.push("key".into());
        ws.items
            .push(Item::Request(Request::new("R", Method::Get, "http://x")));
        let text = export(&ws);
        assert!(!text.contains("secret\""));
        let imported = import(&text).unwrap();
        assert_eq!(imported.format, Format::Nexus);
        assert_eq!(imported.workspace.items, ws.items);
    }

    #[test]
    fn rejects_unknown_and_swagger() {
        assert_eq!(import("{\"hello\": 1}"), Err(ImportError::UnknownFormat));
        assert!(matches!(
            import("swagger: '2.0'"),
            Err(ImportError::Invalid { .. })
        ));
        assert!(matches!(
            import("{ broken"),
            Err(ImportError::Invalid { .. })
        ));
    }
}
