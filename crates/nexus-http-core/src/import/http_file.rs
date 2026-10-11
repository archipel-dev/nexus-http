//! Parses `.http` / `.rest` files (the plain-text request format used by editor REST
//! clients): requests separated by `###` lines, `# name` comments, a request line,
//! header lines, a blank line and then the body.

use super::{Format, ImportError, Imported, split_query};
use crate::model::{Body, Item, KeyValue, Method, Request, Workspace};

pub fn import(text: &str, name: &str) -> Result<Imported, ImportError> {
    let mut workspace = Workspace::new(name);
    let mut warnings = Vec::new();
    for (index, (title, block)) in split_blocks(text).into_iter().enumerate() {
        match parse_block(&block, title, index + 1) {
            Ok(Some(request)) => workspace.items.push(Item::Request(request)),
            Ok(None) => {}
            Err(reason) => warnings.push(format!("request {}: {reason}", index + 1)),
        }
    }
    if workspace.items.is_empty() {
        return Err(ImportError::invalid(
            ".http file",
            "no request lines found (expected `METHOD url`)",
        ));
    }
    Ok(Imported {
        format: Format::HttpFile,
        workspace,
        warnings,
    })
}

/// Blocks of lines; the text after a `###` separator names the following request.
fn split_blocks(text: &str) -> Vec<(Option<String>, Vec<&str>)> {
    let mut blocks = vec![(None, Vec::new())];
    for line in text.lines() {
        if let Some(title) = line.trim_start().strip_prefix("###") {
            let title = title.trim_start_matches('#').trim();
            blocks.push(((!title.is_empty()).then(|| title.to_owned()), Vec::new()));
        } else if let Some((_, lines)) = blocks.last_mut() {
            lines.push(line);
        }
    }
    blocks
}

fn parse_block(
    lines: &[&str],
    mut name: Option<String>,
    number: usize,
) -> Result<Option<Request>, String> {
    let mut iter = lines.iter().copied().peekable();
    let mut request_line = None;
    for line in iter.by_ref() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(comment) = trimmed
            .strip_prefix('#')
            .or_else(|| trimmed.strip_prefix("//"))
        {
            let comment = comment.trim();
            let title = comment.strip_prefix("@name").unwrap_or(comment).trim();
            if name.is_none() && !title.is_empty() {
                name = Some(title.to_owned());
            }
            continue;
        }
        request_line = Some(trimmed);
        break;
    }
    let Some(line) = request_line else {
        return Ok(None);
    };
    let mut parts = line.split_whitespace();
    let first = parts.next().unwrap_or_default();
    let (method, url) =
        if first.contains("://") || first.starts_with('/') || first.starts_with("{{") {
            (Method::Get, first.to_owned())
        } else {
            let url = parts
                .next()
                .ok_or_else(|| format!("`{line}` has no URL"))?
                .to_owned();
            (Method::from(first.to_ascii_uppercase()), url)
        };
    let (url, params) = split_query(&url);
    let mut request = Request::new(
        name.unwrap_or_else(|| format!("{} {url}", method.as_str())),
        method,
        url,
    );
    request.params = params;
    let mut body_lines = Vec::new();
    let mut in_body = false;
    for line in iter {
        if in_body {
            body_lines.push(line);
        } else if line.trim().is_empty() {
            in_body = true;
        } else if let Some((key, value)) = line.split_once(':') {
            request
                .headers
                .push(KeyValue::new(key.trim(), value.trim()));
        } else {
            return Err(format!("line {number}: `{line}` is not a header"));
        }
    }
    let body = body_lines.join("\n");
    let body = body.trim();
    if !body.is_empty() {
        let json = request
            .headers
            .iter()
            .any(|h| h.name.eq_ignore_ascii_case("content-type") && h.value.contains("json"));
        request.body = if json || body.starts_with('{') || body.starts_with('[') {
            Body::Json {
                text: body.to_owned(),
            }
        } else {
            Body::Text {
                text: body.to_owned(),
                content_type: None,
            }
        };
    }
    Ok(Some(request))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_requests_headers_and_bodies() {
        let text = "### List users\nGET https://api.test/users?limit=2\nAccept: application/json\n\n### Create\nPOST https://api.test/users\nContent-Type: application/json\n\n{\"name\": \"a\"}\n";
        let imported = import(text, "api.http").unwrap();
        let requests = imported.workspace.requests();
        assert_eq!(requests.len(), 2);
        let (_, first) = &requests[0];
        assert_eq!(first.name, "List users");
        assert_eq!(first.url, "https://api.test/users");
        assert_eq!(first.params[0].name, "limit");
        assert_eq!(first.headers[0].name, "Accept");
        let (_, second) = &requests[1];
        assert_eq!(second.method, Method::Post);
        assert!(matches!(&second.body, Body::Json { text } if text == "{\"name\": \"a\"}"));
    }

    #[test]
    fn bare_urls_default_to_get_and_garbage_is_an_error() {
        let imported = import("https://example.test/ping\n", "x").unwrap();
        assert_eq!(imported.workspace.requests()[0].1.method, Method::Get);
        assert!(import("   \n# only a comment\n", "x").is_err());
    }
}
