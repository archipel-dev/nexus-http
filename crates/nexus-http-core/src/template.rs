//! Templating: `{{ variable }}` substitution and `{% tag args %}` template tags.
//!
//! Variables are looked up in a JSON object built from the base environment, the
//! active sub-environment and folder variables (innermost wins). Paths such as
//! `{{ api.host }}`, `{{ users[0].id }}` and the `_.` prefix (`{{ _.token }}`)
//! are supported. Variable values may themselves contain templates; they are
//! rendered recursively up to a fixed depth so cycles produce an error instead of
//! hanging.
//!
//! Tags: `uuid`, `now ['iso'|'millis'|'unix'|'date'|'time']`, `base64 'encode'|'decode', value`,
//! `urlencode value`, `hash 'md5'|'sha256', value`, `random_int min, max`,
//! `env 'NAME'` (process environment), and
//! `response 'body'|'header'|'status'|'url', 'request-id', 'filter'` for chaining.

use crate::model::{Environment, Folder, Workspace};
use base64::Engine as _;
use serde_json::{Map, Value};
use sha2::Digest as _;
use std::fmt;
use std::ops::Range;
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_DEPTH: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TemplateError {
    pub message: String,
    /// Byte range of the offending template expression in its input.
    pub span: Range<usize>,
}

impl fmt::Display for TemplateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for TemplateError {}

/// Which part of another request's response a `response` tag reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseAttribute {
    Body,
    Header,
    Status,
    Url,
}

/// Supplies another request's latest response for `{% response %}` tags.
pub trait ResponseSource {
    /// Returns the selected value or an actionable error message.
    fn response_value(
        &self,
        request_id: &str,
        attribute: ResponseAttribute,
        filter: &str,
    ) -> Result<String, String>;
}

/// A [`ResponseSource`] that has no responses.
pub struct NoResponses;

impl ResponseSource for NoResponses {
    fn response_value(
        &self,
        request_id: &str,
        _: ResponseAttribute,
        _: &str,
    ) -> Result<String, String> {
        Err(format!("no response available for request {request_id}"))
    }
}

/// Variables plus collaborators needed to render templates.
pub struct Context<'a> {
    pub variables: Map<String, Value>,
    pub responses: &'a dyn ResponseSource,
}

impl<'a> Context<'a> {
    pub fn new(variables: Map<String, Value>) -> Self {
        Self {
            variables,
            responses: &NoResponses,
        }
    }

    pub fn with_responses(mut self, responses: &'a dyn ResponseSource) -> Self {
        self.responses = responses;
        self
    }

    /// Variables for a request inside `folders` (outermost first) of `workspace`.
    pub fn for_request(workspace: &Workspace, folders: &[&Folder]) -> Self {
        Self::new(merged_variables(
            &workspace.base_environment,
            workspace.active_environment(),
            folders,
        ))
    }

    /// Renders `input`, substituting variables and evaluating tags.
    pub fn render(&self, input: &str) -> Result<String, TemplateError> {
        self.render_depth(input, 0)
    }

    fn render_depth(&self, input: &str, depth: usize) -> Result<String, TemplateError> {
        if depth > MAX_DEPTH {
            return Err(TemplateError {
                message: "template nesting is too deep (is a variable referring to itself?)".into(),
                span: 0..input.len(),
            });
        }
        let mut out = String::with_capacity(input.len());
        for segment in tokenize(input) {
            match segment.kind {
                SegmentKind::Text => out.push_str(&input[segment.span.clone()]),
                SegmentKind::Variable(path) => {
                    let value = lookup(&self.variables, &path).ok_or_else(|| TemplateError {
                        message: format!("undefined variable `{path}`"),
                        span: segment.span.clone(),
                    })?;
                    let text = value_to_text(value);
                    let rendered =
                        self.render_depth(&text, depth + 1)
                            .map_err(|e| TemplateError {
                                message: format!("in variable `{path}`: {}", e.message),
                                span: segment.span.clone(),
                            })?;
                    out.push_str(&rendered);
                }
                SegmentKind::Tag(body) => {
                    let value = self
                        .eval_tag(&body, depth)
                        .map_err(|message| TemplateError {
                            message,
                            span: segment.span.clone(),
                        })?;
                    out.push_str(&value);
                }
                SegmentKind::Unclosed => {
                    return Err(TemplateError {
                        message: "unclosed template expression".into(),
                        span: segment.span,
                    });
                }
            }
        }
        Ok(out)
    }

    fn eval_tag(&self, body: &str, depth: usize) -> Result<String, String> {
        let (name, rest) = body
            .trim()
            .split_once(char::is_whitespace)
            .map(|(n, r)| (n, r.trim()))
            .unwrap_or((body.trim(), ""));
        let args = parse_args(rest)?
            .into_iter()
            .map(|arg| match arg {
                Arg::Literal(text) => Ok(text),
                Arg::Variable(path) => lookup(&self.variables, &path)
                    .map(value_to_text)
                    .ok_or_else(|| format!("undefined variable `{path}`")),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let args = args
            .iter()
            .map(|a| self.render_depth(a, depth + 1).map_err(|e| e.message))
            .collect::<Result<Vec<_>, _>>()?;
        let arg = |i: usize| args.get(i).map(String::as_str).unwrap_or("");
        match name {
            "uuid" => Ok(uuid::Uuid::new_v4().to_string()),
            "now" | "timestamp" => Ok(now(arg(0))),
            "base64" => match arg(0) {
                "encode" => Ok(base64::engine::general_purpose::STANDARD.encode(arg(1))),
                "decode" => base64::engine::general_purpose::STANDARD
                    .decode(arg(1).trim())
                    .map_err(|e| format!("base64 decode failed: {e}"))
                    .map(|b| String::from_utf8_lossy(&b).into_owned()),
                other => Err(format!(
                    "base64 expects 'encode' or 'decode', got '{other}'"
                )),
            },
            "urlencode" => Ok(url::form_urlencoded::byte_serialize(arg(0).as_bytes()).collect()),
            "hash" => match arg(0) {
                "md5" => Ok(hex(&md5::Md5::digest(arg(1).as_bytes()))),
                "sha1" => Err("sha1 is not supported; use sha256".into()),
                "sha256" => Ok(hex(&sha2::Sha256::digest(arg(1).as_bytes()))),
                "sha512" => Ok(hex(&sha2::Sha512::digest(arg(1).as_bytes()))),
                other => Err(format!("unknown hash algorithm '{other}'")),
            },
            "random_int" | "randomInt" => {
                let min: i64 = arg(0).parse().unwrap_or(0);
                let max: i64 = arg(1).parse().unwrap_or(100);
                if min > max {
                    return Err(format!("random_int: min {min} is greater than max {max}"));
                }
                use rand::Rng as _;
                Ok(rand::thread_rng().gen_range(min..=max).to_string())
            }
            "env" => std::env::var(arg(0))
                .map_err(|_| format!("process environment variable '{}' is not set", arg(0))),
            "response" => {
                let attribute = match arg(0) {
                    "body" => ResponseAttribute::Body,
                    "header" => ResponseAttribute::Header,
                    "status" => ResponseAttribute::Status,
                    "url" => ResponseAttribute::Url,
                    other => {
                        return Err(format!(
                            "response attribute must be body, header, status or url, got '{other}'"
                        ));
                    }
                };
                if arg(1).is_empty() {
                    return Err("response tag needs a request id".into());
                }
                self.responses.response_value(arg(1), attribute, arg(2))
            }
            "" => Err("empty template tag".into()),
            other => Err(format!("unknown template tag '{other}'")),
        }
    }
}

/// Merges base, sub-environment and folder variables (later wins, objects merge deeply).
pub fn merged_variables(
    base: &Environment,
    active: Option<&Environment>,
    folders: &[&Folder],
) -> Map<String, Value> {
    let mut vars = base.data.clone();
    if let Some(env) = active {
        merge(&mut vars, &env.data);
    }
    for folder in folders {
        merge(&mut vars, &folder.variables);
    }
    vars
}

fn merge(into: &mut Map<String, Value>, from: &Map<String, Value>) {
    for (key, value) in from {
        match (into.get_mut(key), value) {
            (Some(Value::Object(existing)), Value::Object(incoming)) => merge(existing, incoming),
            _ => {
                into.insert(key.clone(), value.clone());
            }
        }
    }
}

/// A lexical piece of a template, used for rendering and for UI highlighting.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    pub span: Range<usize>,
    pub kind: SegmentKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SegmentKind {
    Text,
    /// Variable path without braces and without the `_.` prefix.
    Variable(String),
    /// Tag body without `{%`/`%}`.
    Tag(String),
    /// `{{` or `{%` without a closing delimiter.
    Unclosed,
}

/// Splits a template into text, variable and tag segments.
pub fn tokenize(input: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut text_start = 0;
    let mut i = 0;
    let bytes = input.as_bytes();
    while i + 1 < bytes.len() {
        // Compare bytes: slicing at arbitrary byte offsets would split UTF-8.
        let close = match (bytes[i], bytes[i + 1]) {
            (b'{', b'{') => "}}",
            (b'{', b'%') => "%}",
            _ => {
                i += 1;
                continue;
            }
        };
        if text_start < i {
            out.push(Segment {
                span: text_start..i,
                kind: SegmentKind::Text,
            });
        }
        match input[i + 2..].find(close) {
            Some(rel) => {
                let end = i + 2 + rel + 2;
                let inner = input[i + 2..i + 2 + rel].trim();
                let kind = if close == "}}" {
                    SegmentKind::Variable(inner.strip_prefix("_.").unwrap_or(inner).to_owned())
                } else {
                    SegmentKind::Tag(inner.to_owned())
                };
                out.push(Segment { span: i..end, kind });
                i = end;
            }
            None => {
                out.push(Segment {
                    span: i..input.len(),
                    kind: SegmentKind::Unclosed,
                });
                return out;
            }
        }
        text_start = i;
    }
    if text_start < input.len() {
        out.push(Segment {
            span: text_start..input.len(),
            kind: SegmentKind::Text,
        });
    }
    out
}

/// Whether `input` contains any template expression.
pub fn has_templates(input: &str) -> bool {
    input.contains("{{") || input.contains("{%")
}

/// Flattened variable paths available for autocomplete (`api.host`, `users[0]`...).
pub fn variable_paths(vars: &Map<String, Value>) -> Vec<(String, Value)> {
    fn walk(prefix: &str, value: &Value, out: &mut Vec<(String, Value)>) {
        out.push((prefix.to_owned(), value.clone()));
        if let Value::Object(map) = value {
            for (k, v) in map {
                walk(&format!("{prefix}.{k}"), v, out);
            }
        }
    }
    let mut out = Vec::new();
    for (k, v) in vars {
        walk(k, v, &mut out);
    }
    out
}

/// Resolves `a.b[0].c` against the variables.
pub fn lookup<'v>(vars: &'v Map<String, Value>, path: &str) -> Option<&'v Value> {
    let mut parts = split_path(path)?.into_iter();
    let first = match parts.next()? {
        PathPart::Key(k) => vars.get(&k)?,
        PathPart::Index(_) => return None,
    };
    parts.try_fold(first, |value, part| match part {
        PathPart::Key(k) => value.as_object()?.get(&k),
        PathPart::Index(i) => value.as_array()?.get(i),
    })
}

enum PathPart {
    Key(String),
    Index(usize),
}

fn split_path(path: &str) -> Option<Vec<PathPart>> {
    let mut parts = Vec::new();
    for dotted in path.trim().split('.') {
        let (key, mut rest) = match dotted.find('[') {
            Some(at) => (&dotted[..at], &dotted[at..]),
            None => (dotted, ""),
        };
        if !key.is_empty() {
            parts.push(PathPart::Key(key.to_owned()));
        } else if rest.is_empty() {
            return None;
        }
        while let Some(stripped) = rest.strip_prefix('[') {
            let end = stripped.find(']')?;
            let inner = stripped[..end].trim_matches(|c| c == '\'' || c == '"');
            match inner.parse::<usize>() {
                Ok(i) => parts.push(PathPart::Index(i)),
                Err(_) => parts.push(PathPart::Key(inner.to_owned())),
            }
            rest = &stripped[end + 1..];
        }
        if !rest.is_empty() {
            return None;
        }
    }
    (!parts.is_empty()).then_some(parts)
}

/// String form of a JSON value as inserted into a template.
pub fn value_to_text(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => other.to_string(),
    }
}

enum Arg {
    Literal(String),
    Variable(String),
}

fn parse_args(input: &str) -> Result<Vec<Arg>, String> {
    let mut args = Vec::new();
    let mut chars = input.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        let Some(&c) = chars.peek() else { break };
        if c == '\'' || c == '"' {
            chars.next();
            let mut text = String::new();
            loop {
                match chars.next() {
                    Some('\\') => match chars.next() {
                        Some(escaped) => text.push(escaped),
                        None => return Err("unterminated string in tag".into()),
                    },
                    Some(q) if q == c => break,
                    Some(other) => text.push(other),
                    None => return Err("unterminated string in tag".into()),
                }
            }
            args.push(Arg::Literal(text));
        } else {
            let mut word = String::new();
            while let Some(&c) = chars.peek() {
                if c.is_whitespace() || c == ',' {
                    break;
                }
                word.push(c);
                chars.next();
            }
            let is_number = word.parse::<f64>().is_ok();
            let is_keyword = matches!(word.as_str(), "true" | "false" | "null");
            if is_number || is_keyword {
                args.push(Arg::Literal(word));
            } else {
                let path = word.strip_prefix("_.").unwrap_or(&word).to_owned();
                args.push(Arg::Variable(path));
            }
        }
    }
    Ok(args)
}

fn now(format: &str) -> String {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    match format {
        "millis" | "ms" => since.as_millis().to_string(),
        "unix" | "seconds" => since.as_secs().to_string(),
        "date" => iso8601(since.as_secs(), 0)[..10].to_owned(),
        "time" => iso8601(since.as_secs(), 0)[11..19].to_owned(),
        _ => iso8601(since.as_secs(), since.subsec_millis()),
    }
}

/// `YYYY-MM-DDTHH:MM:SS.mmmZ` for a Unix timestamp (UTC, proleptic Gregorian).
pub fn iso8601(secs: u64, millis: u32) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let (y, m, d) = civil_from_days(days);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Howard Hinnant's days-to-civil algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ctx(vars: Value) -> Context<'static> {
        Context::new(vars.as_object().unwrap().clone())
    }

    #[test]
    fn substitutes_variables_and_paths() {
        let c = ctx(json!({"host": "api.test", "api": {"v": 2}, "ids": [7, 8]}));
        assert_eq!(
            c.render("https://{{ host }}/v{{api.v}}/{{ _.ids[1] }}")
                .unwrap(),
            "https://api.test/v2/8"
        );
    }

    #[test]
    fn renders_nested_variables_and_detects_cycles() {
        let c =
            ctx(json!({"base": "https://{{host}}", "host": "x.dev", "a": "{{b}}", "b": "{{a}}"}));
        assert_eq!(c.render("{{base}}/p").unwrap(), "https://x.dev/p");
        let err = c.render("{{a}}").unwrap_err();
        assert!(err.message.contains("too deep"), "{err}");
    }

    #[test]
    fn reports_undefined_and_unclosed_with_spans() {
        let c = ctx(json!({}));
        let err = c.render("a {{ missing }} b").unwrap_err();
        assert_eq!(err.span, 2..15);
        assert!(err.message.contains("missing"));
        assert!(c.render("a {{ b").unwrap_err().message.contains("unclosed"));
    }

    #[test]
    fn evaluates_builtin_tags() {
        let c = ctx(json!({"user": "ada"}));
        assert_eq!(c.render("{% base64 'encode', 'hi' %}").unwrap(), "aGk=");
        assert_eq!(c.render("{% base64 'decode', 'aGk=' %}").unwrap(), "hi");
        assert_eq!(c.render("{% base64 'encode', user %}").unwrap(), "YWRh");
        assert_eq!(
            c.render("{% hash 'md5', 'abc' %}").unwrap(),
            "900150983cd24fb0d6963f7d28e17f72"
        );
        assert_eq!(c.render("{% urlencode 'a b&c' %}").unwrap(), "a+b%26c");
        assert_eq!(c.render("{% uuid %}").unwrap().len(), 36);
        let n: i64 = c.render("{% random_int 3, 3 %}").unwrap().parse().unwrap();
        assert_eq!(n, 3);
        assert!(
            c.render("{% nope %}")
                .unwrap_err()
                .message
                .contains("unknown")
        );
    }

    #[test]
    fn formats_dates() {
        assert_eq!(iso8601(0, 0), "1970-01-01T00:00:00.000Z");
        assert_eq!(iso8601(951_782_400, 5), "2000-02-29T00:00:00.005Z");
        assert_eq!(iso8601(1_790_000_000, 0), "2026-09-21T14:13:20.000Z");
    }

    #[test]
    fn response_tag_uses_source() {
        struct Fixed;
        impl ResponseSource for Fixed {
            fn response_value(
                &self,
                id: &str,
                attribute: ResponseAttribute,
                filter: &str,
            ) -> Result<String, String> {
                Ok(format!("{id}:{attribute:?}:{filter}"))
            }
        }
        let c = ctx(json!({})).with_responses(&Fixed);
        assert_eq!(
            c.render("Bearer {% response 'body', 'req_1', '$.token' %}")
                .unwrap(),
            "Bearer req_1:Body:$.token"
        );
        assert!(
            ctx(json!({}))
                .render("{% response 'body', 'req_1', '' %}")
                .is_err()
        );
    }

    #[test]
    fn merges_environments_and_folders() {
        let base = Environment::new("Base")
            .with("host", "prod")
            .with("cfg", json!({"a": 1, "b": 2}));
        let dev = Environment::new("Dev")
            .with("host", "dev")
            .with("cfg", json!({"b": 3}));
        let mut folder = Folder::new("F");
        folder.variables.insert("token".into(), json!("t"));
        let vars = merged_variables(&base, Some(&dev), &[&folder]);
        assert_eq!(vars["host"], "dev");
        assert_eq!(vars["cfg"], json!({"a": 1, "b": 3}));
        assert_eq!(vars["token"], "t");
        let paths: Vec<_> = variable_paths(&vars).into_iter().map(|(p, _)| p).collect();
        assert!(paths.contains(&"cfg.a".to_owned()));
    }

    #[test]
    fn handles_multibyte_text() {
        let c = ctx(json!({"name": "Zoë"}));
        assert_eq!(c.render("ünï {{name}} ✓ {").unwrap(), "ünï Zoë ✓ {");
        assert_eq!(tokenize("Zoë ✓").len(), 1);
    }

    #[test]
    fn tokenizes_for_highlighting() {
        let segments = tokenize("x{{a}}y{% uuid %}");
        let kinds: Vec<_> = segments.iter().map(|s| s.kind.clone()).collect();
        assert_eq!(
            kinds,
            vec![
                SegmentKind::Text,
                SegmentKind::Variable("a".into()),
                SegmentKind::Text,
                SegmentKind::Tag("uuid".into()),
            ]
        );
    }
}
