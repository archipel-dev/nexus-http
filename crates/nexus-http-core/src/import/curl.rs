//! Parses a curl command line into a request.

use super::{ImportError, split_query};
use crate::model::{Auth, Body, KeyValue, Method, MultipartField, MultipartValue, Request};

/// Parses `curl ...` (as copied from browser dev tools or docs).
pub fn parse(command: &str) -> Result<Request, ImportError> {
    // Join shell line continuations (`\` + newline, and `^` on Windows cmd).
    let joined = command
        .replace("\\\r\n", " ")
        .replace("\\\n", " ")
        .replace("^\r\n", " ")
        .replace("^\n", " ");
    let words = shell_words::split(&joined)
        .map_err(|e| ImportError::invalid("curl command", e.to_string()))?;
    let mut words = words.into_iter().peekable();
    match words.next().as_deref() {
        Some("curl") => {}
        _ => {
            return Err(ImportError::invalid(
                "curl command",
                "must start with `curl`",
            ));
        }
    }

    let mut url: Option<String> = None;
    let mut method: Option<String> = None;
    let mut headers: Vec<KeyValue> = Vec::new();
    let mut data: Vec<String> = Vec::new();
    let mut data_urlencode: Vec<String> = Vec::new();
    let mut form: Vec<MultipartField> = Vec::new();
    let mut user: Option<String> = None;
    let mut digest = false;
    let mut get = false;
    let mut head = false;
    let mut follow = None;
    let mut insecure = false;
    let mut timeout_ms = None;
    let mut binary_file: Option<String> = None;

    while let Some(word) = words.next() {
        // Support `--flag=value` as well as `--flag value`, and `-Xvalue`.
        let (flag, inline) = if word.starts_with("--") {
            match word.split_once('=') {
                Some((f, v)) => (f.to_owned(), Some(v.to_owned())),
                None => (word.clone(), None),
            }
        } else if word.starts_with('-') && word.len() > 2 && takes_value(&word[..2]) {
            (word[..2].to_owned(), Some(word[2..].to_owned()))
        } else {
            (word.clone(), None)
        };
        let mut value = || -> Result<String, ImportError> {
            inline.clone().or_else(|| words.next()).ok_or_else(|| {
                ImportError::invalid("curl command", format!("{flag} needs a value"))
            })
        };
        match flag.as_str() {
            "-X" | "--request" => method = Some(value()?),
            "-H" | "--header" => {
                let raw = value()?;
                if let Some((name, val)) = raw.split_once(':') {
                    headers.push(KeyValue::new(name.trim(), val.trim()));
                }
            }
            "-d" | "--data" | "--data-raw" | "--data-ascii" => data.push(value()?),
            "--data-binary" => {
                let v = value()?;
                match v.strip_prefix('@') {
                    Some(path) => binary_file = Some(path.to_owned()),
                    None => data.push(v),
                }
            }
            "--data-urlencode" => data_urlencode.push(value()?),
            "--json" => {
                data.push(value()?);
                set_default(&mut headers, "Content-Type", "application/json");
                set_default(&mut headers, "Accept", "application/json");
            }
            "-F" | "--form" | "--form-string" => {
                let raw = value()?;
                let (name, val) = raw.split_once('=').unwrap_or((&raw, ""));
                let value = match val.strip_prefix('@').filter(|_| flag != "--form-string") {
                    Some(file) => {
                        let (path, ct) = match file.split_once(";type=") {
                            Some((p, t)) => (p.to_owned(), Some(t.to_owned())),
                            None => (file.to_owned(), None),
                        };
                        MultipartValue::File {
                            path,
                            content_type: ct,
                        }
                    }
                    None => MultipartValue::Text {
                        value: val.to_owned(),
                    },
                };
                form.push(MultipartField {
                    name: name.to_owned(),
                    value,
                    enabled: true,
                });
            }
            "-u" | "--user" => user = Some(value()?),
            "--digest" => digest = true,
            "--basic" => digest = false,
            "-b" | "--cookie" => headers.push(KeyValue::new("Cookie", value()?)),
            "-A" | "--user-agent" => headers.push(KeyValue::new("User-Agent", value()?)),
            "-e" | "--referer" => headers.push(KeyValue::new("Referer", value()?)),
            "--oauth2-bearer" => headers.push(KeyValue::new(
                "Authorization",
                format!("Bearer {}", value()?),
            )),
            "--url" => url = Some(value()?),
            "-G" | "--get" => get = true,
            "-I" | "--head" => head = true,
            "-L" | "--location" => follow = Some(true),
            "-k" | "--insecure" => insecure = true,
            "-m" | "--max-time" => {
                timeout_ms = value()?.parse::<f64>().ok().map(|s| (s * 1000.0) as u64);
            }
            // Flags without effect on the request definition.
            "--compressed" | "-s" | "--silent" | "-v" | "--verbose" | "-i" | "--include" | "-S"
            | "--show-error" | "-f" | "--fail" | "--http1.1" | "--http2" | "-N" | "--no-buffer" => {
            }
            "-o" | "--output" | "-w" | "--write-out" | "--connect-timeout" | "--retry" => {
                value()?;
            }
            other if other.starts_with('-') => {
                // Unknown flags are ignored; curl's own parser would also reject
                // unknown flags, so this only affects exotic options.
            }
            _ => {
                if url.is_none() {
                    url = Some(word);
                }
            }
        }
    }

    let raw_url = url.ok_or_else(|| ImportError::invalid("curl command", "no URL found"))?;
    let (base_url, mut params) = split_query(&raw_url);
    let content_type = headers
        .iter()
        .find(|h| h.name.eq_ignore_ascii_case("content-type"))
        .map(|h| h.value.to_ascii_lowercase());

    let mut all_data = data.clone();
    for item in &data_urlencode {
        let (name, val) = match item.split_once('=') {
            Some((n, v)) => (n.to_owned(), v.to_owned()),
            None => (String::new(), item.clone()),
        };
        let encoded: String = url::form_urlencoded::byte_serialize(val.as_bytes()).collect();
        all_data.push(if name.is_empty() {
            encoded
        } else {
            format!("{name}={encoded}")
        });
    }

    let body = if get {
        for chunk in &all_data {
            let (_, extra) = split_query(&format!("?{chunk}"));
            params.extend(extra);
        }
        Body::None
    } else if !form.is_empty() {
        Body::Multipart { fields: form }
    } else if let Some(path) = binary_file {
        Body::Binary { path }
    } else if !all_data.is_empty() {
        let text = all_data.join("&");
        match content_type.as_deref() {
            Some(ct) if ct.contains("json") => Body::Json { text },
            Some(ct) if ct.contains("xml") => Body::Xml { text },
            None | Some("application/x-www-form-urlencoded") => {
                let (_, fields) = split_query(&format!("?{text}"));
                Body::FormUrlEncoded { fields }
            }
            Some(ct) => Body::Text {
                text,
                content_type: Some(ct.to_owned()),
            },
        }
    } else {
        Body::None
    };

    let method = match (method, head) {
        (Some(m), _) => Method::from(m),
        (None, true) => Method::Head,
        (None, false) if !matches!(body, Body::None) => Method::Post,
        _ => Method::Get,
    };
    let auth = match user {
        Some(u) => {
            let (username, password) = u.split_once(':').unwrap_or((&u, ""));
            if digest {
                Auth::Digest {
                    username: username.into(),
                    password: password.into(),
                }
            } else {
                Auth::Basic {
                    username: username.into(),
                    password: password.into(),
                }
            }
        }
        None => Auth::None,
    };
    // Turn an explicit bearer header into Bearer auth so it is editable as such.
    let mut auth = auth;
    if auth == Auth::None
        && let Some(pos) = headers.iter().position(|h| {
            h.name.eq_ignore_ascii_case("authorization") && h.value.starts_with("Bearer ")
        })
    {
        let header = headers.remove(pos);
        auth = Auth::Bearer {
            token: header.value["Bearer ".len()..].trim().to_owned(),
            prefix: "Bearer".into(),
        };
    }

    let name = request_name(&method, &base_url);
    let mut request = Request::new(name, method, base_url);
    request.params = params;
    request.headers = headers;
    request.body = body;
    request.auth = auth;
    request.settings.follow_redirects = follow;
    if insecure {
        request.settings.verify_tls = Some(false);
    }
    request.settings.timeout_ms = timeout_ms;
    Ok(request)
}

fn takes_value(flag: &str) -> bool {
    matches!(
        flag,
        "-X" | "-H" | "-d" | "-F" | "-u" | "-b" | "-A" | "-e" | "-m" | "-o" | "-w"
    )
}

fn set_default(headers: &mut Vec<KeyValue>, name: &str, value: &str) {
    if !headers.iter().any(|h| h.name.eq_ignore_ascii_case(name)) {
        headers.push(KeyValue::new(name, value));
    }
}

/// `GET /users/{id}`-style name from a URL.
pub(crate) fn request_name(method: &Method, url: &str) -> String {
    let path = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split_once('/')
        .map(|(_, p)| format!("/{p}"))
        .unwrap_or_else(|| "/".into());
    let path = path.split(['?', '#']).next().unwrap_or("/");
    format!("{method} {path}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_browser_style_command() {
        let req = parse(
            "curl 'https://api.test/v1/items?page=2' \\\n  -H 'Accept: application/json' \\\n  -H 'Content-Type: application/json' \\\n  --data-raw '{\"name\":\"Ada\"}' \\\n  --compressed",
        )
        .unwrap();
        assert_eq!(req.method, Method::Post);
        assert_eq!(req.url, "https://api.test/v1/items");
        assert_eq!(req.params, vec![KeyValue::new("page", "2")]);
        assert_eq!(req.headers.len(), 2);
        assert_eq!(
            req.body,
            Body::Json {
                text: "{\"name\":\"Ada\"}".into()
            }
        );
        assert_eq!(req.name, "POST /v1/items");
    }

    #[test]
    fn parses_forms_auth_and_flags() {
        let req = parse("curl -X PUT -u ada:pw --digest -F name=Ada -F avatar=@me.png;type=image/png -k -L -m 2.5 https://x.test/u").unwrap();
        assert_eq!(req.method, Method::Put);
        assert_eq!(
            req.auth,
            Auth::Digest {
                username: "ada".into(),
                password: "pw".into()
            }
        );
        let Body::Multipart { fields } = &req.body else {
            panic!("multipart expected")
        };
        assert_eq!(fields.len(), 2);
        assert_eq!(
            fields[1].value,
            MultipartValue::File {
                path: "me.png".into(),
                content_type: Some("image/png".into())
            }
        );
        assert_eq!(req.settings.verify_tls, Some(false));
        assert_eq!(req.settings.follow_redirects, Some(true));
        assert_eq!(req.settings.timeout_ms, Some(2500));
    }

    #[test]
    fn form_data_get_and_bearer() {
        let req = parse("curl https://x.test -d a=1 -d b=two%20words").unwrap();
        assert_eq!(
            req.body,
            Body::FormUrlEncoded {
                fields: vec![KeyValue::new("a", "1"), KeyValue::new("b", "two words")]
            }
        );
        let get = parse(
            "curl -G https://x.test/s --data-urlencode 'q=a b' -H 'Authorization: Bearer xyz'",
        )
        .unwrap();
        assert_eq!(get.method, Method::Get);
        assert_eq!(get.params, vec![KeyValue::new("q", "a b")]);
        assert_eq!(
            get.auth,
            Auth::Bearer {
                token: "xyz".into(),
                prefix: "Bearer".into()
            }
        );
        assert!(get.headers.is_empty());
        let head = parse("curl -I --url=https://x.test").unwrap();
        assert_eq!(head.method, Method::Head);
        let json = parse("curl --json '{\"a\":1}' https://x.test").unwrap();
        assert!(matches!(json.body, Body::Json { .. }));
    }

    #[test]
    fn rejects_bad_commands() {
        assert!(parse("wget x").is_err());
        assert!(parse("curl -H").is_err());
        assert!(parse("curl -v").is_err());
        assert!(parse("curl 'unterminated").is_err());
    }
}
