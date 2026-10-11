//! Autocomplete for `{{ variables }}` and `{% tags %}` in template fields.

use crate::template;
use serde_json::{Map, Value};

/// The template expression being typed at the cursor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Context {
    /// Char index of the opening `{{` / `{%`.
    pub start: usize,
    /// What was typed after the opening braces (without `_.`).
    pub prefix: String,
    pub tag: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    /// Replacement for the whole expression, from `{{`/`{%` up to the cursor.
    pub insert: String,
    pub detail: String,
}

const TAGS: [(&str, &str, &str); 8] = [
    ("uuid", "{% uuid %}", "random UUID v4"),
    (
        "now",
        "{% now 'iso' %}",
        "current time: iso, millis, unix, date, time",
    ),
    (
        "base64",
        "{% base64 'encode', '' %}",
        "encode or decode Base64",
    ),
    ("urlencode", "{% urlencode '' %}", "percent-encode a value"),
    (
        "hash",
        "{% hash 'sha256', '' %}",
        "md5, sha256 or sha512 digest",
    ),
    (
        "random_int",
        "{% random_int 0, 100 %}",
        "random integer in a range",
    ),
    ("env", "{% env 'NAME' %}", "process environment variable"),
    (
        "response",
        "{% response 'body', 'req_id', '$.token' %}",
        "value from another request's response",
    ),
];

/// Finds an unclosed `{{`/`{%` before the cursor (chars, not bytes).
pub fn context_at(text: &str, cursor: usize) -> Option<Context> {
    let before: String = text.chars().take(cursor).collect();
    let open_var = before.rfind("{{");
    let open_tag = before.rfind("{%");
    let (open, tag) = match (open_var, open_tag) {
        (Some(v), Some(t)) if t > v => (t, true),
        (Some(v), _) => (v, false),
        (None, Some(t)) => (t, true),
        (None, None) => return None,
    };
    let inner = &before[open + 2..];
    if inner.contains("}}") || inner.contains("%}") {
        return None;
    }
    let prefix = inner.trim_start();
    // Only complete the leading identifier (tags take arguments after a space).
    if prefix.contains(char::is_whitespace) {
        return None;
    }
    Some(Context {
        start: before[..open].chars().count(),
        prefix: prefix.strip_prefix("_.").unwrap_or(prefix).to_owned(),
        tag,
    })
}

/// Completion candidates ranked prefix matches first, then substring matches.
pub fn items(ctx: &Context, vars: &Map<String, Value>) -> Vec<Item> {
    let needle = ctx.prefix.to_lowercase();
    let mut scored: Vec<(u8, Item)> = if ctx.tag {
        TAGS.iter()
            .map(|(name, insert, detail)| Item {
                label: (*name).to_owned(),
                insert: (*insert).to_owned(),
                detail: (*detail).to_owned(),
            })
            .filter_map(|i| rank(&i.label, &needle).map(|r| (r, i)))
            .collect()
    } else {
        template::variable_paths(vars)
            .into_iter()
            .map(|(path, value)| Item {
                insert: format!("{{{{ {path} }}}}"),
                detail: preview(&value),
                label: path,
            })
            .filter_map(|i| rank(&i.label, &needle).map(|r| (r, i)))
            .collect()
    };
    scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.cmp(&b.1.label)));
    scored.into_iter().map(|(_, i)| i).take(12).collect()
}

fn rank(label: &str, needle: &str) -> Option<u8> {
    let l = label.to_lowercase();
    if needle.is_empty() || l.starts_with(needle) {
        Some(0)
    } else if l.contains(needle) {
        Some(1)
    } else {
        None
    }
}

fn preview(value: &Value) -> String {
    let text = match value {
        Value::Object(m) => format!("{{…}} {} keys", m.len()),
        Value::Array(a) => format!("[…] {} items", a.len()),
        other => template::value_to_text(other),
    };
    if text.chars().count() > 40 {
        format!("{}…", text.chars().take(40).collect::<String>())
    } else {
        text
    }
}

/// What completes at `cursor` (chars) in `text`: the chars `start..end` to replace (the
/// open expression, and a closing `}}`/`%}` already typed after the cursor) and the
/// ranked candidates. `None` when the cursor is not in an open expression.
pub fn complete(
    text: &str,
    cursor: usize,
    vars: &Map<String, Value>,
) -> Option<(usize, usize, Vec<Item>)> {
    let ctx = context_at(text, cursor)?;
    let items = items(&ctx, vars);
    Some((ctx.start, replaced_end(text, &ctx, cursor), items))
}

/// The end of the replaced part: the cursor, or after a closing `}}`/`%}` right behind it.
fn replaced_end(text: &str, ctx: &Context, cursor: usize) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let rest: String = chars[cursor..].iter().take(3).collect();
    let rest_trim = rest.trim_start();
    let close = if ctx.tag { "%}" } else { "}}" };
    if rest_trim.starts_with(close) {
        cursor + rest.chars().count() - rest_trim.chars().count() + 2
    } else {
        cursor
    }
}

/// Applies `item` and returns the new text and cursor (chars).
pub fn apply(text: &str, ctx: &Context, cursor: usize, item: &Item) -> (String, usize) {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    // Also swallow an auto-typed closing `}}`/`%}` right after the cursor.
    let mut end = cursor;
    let rest: String = chars[cursor..].iter().take(3).collect();
    let rest_trim = rest.trim_start();
    let close = if ctx.tag { "%}" } else { "}}" };
    if rest_trim.starts_with(close) {
        end += rest.len() - rest_trim.len() + 2;
    }
    let mut out: String = chars[..ctx.start].iter().collect();
    out.push_str(&item.insert);
    let new_cursor = out.chars().count();
    out.extend(chars[end.min(chars.len())..].iter());
    (out, new_cursor)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn detects_open_expressions() {
        assert_eq!(
            context_at("http://{{ ho", 12),
            Some(Context {
                start: 7,
                prefix: "ho".into(),
                tag: false
            })
        );
        assert_eq!(context_at("{{_.api.v", 9).unwrap().prefix, "api.v");
        assert!(context_at("{{ host }}/x", 12).is_none());
        assert!(context_at("plain", 5).is_none());
        assert!(context_at("{% base64 'enc", 14).is_none());
        assert_eq!(
            context_at("ü{% uu", 6),
            Some(Context {
                start: 1,
                prefix: "uu".into(),
                tag: true
            })
        );
    }

    #[test]
    fn ranks_variables_and_tags() {
        let vars = json!({"host": "x", "api": {"host": "y"}, "token": "t"})
            .as_object()
            .unwrap()
            .clone();
        let found = items(
            &Context {
                start: 0,
                prefix: "ho".into(),
                tag: false,
            },
            &vars,
        );
        assert_eq!(found[0].label, "host");
        assert!(found.iter().any(|i| i.label == "api.host"));
        assert!(!found.iter().any(|i| i.label == "token"));
        let tags = items(
            &Context {
                start: 0,
                prefix: "re".into(),
                tag: true,
            },
            &vars,
        );
        assert_eq!(tags[0].label, "response");
    }

    #[test]
    fn applies_and_swallows_closing_braces() {
        let ctx = context_at("a {{ ho}}/b", 7).unwrap();
        let item = Item {
            label: "host".into(),
            insert: "{{ host }}".into(),
            detail: String::new(),
        };
        assert_eq!(
            apply("a {{ ho}}/b", &ctx, 7, &item),
            ("a {{ host }}/b".into(), 12)
        );
        let ctx = context_at("{{ ho", 5).unwrap();
        assert_eq!(apply("{{ ho", &ctx, 5, &item), ("{{ host }}".into(), 10));
    }
}

#[cfg(test)]
mod complete_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn complete_replaces_a_closing_brace_behind_the_cursor() {
        let vars = json!({"host": "x"})
            .as_object()
            .cloned()
            .unwrap_or_default();
        let (start, end, items) = complete("a{{ ho }}", 6, &vars).expect("open expression");
        assert_eq!((start, end), (1, 9));
        assert_eq!(items[0].insert, "{{ host }}");
        let (_, end, _) = complete("{{ ho", 5, &vars).expect("open expression");
        assert_eq!(end, 5);
        assert!(complete("plain", 5, &vars).is_none());
    }
}
