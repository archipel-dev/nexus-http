//! Postman Collection v2.1 importer (public collection schema).

use super::{Format, ImportError, Imported, split_query, str_of};
use crate::model::{
    ApiKeyLocation, Auth, Body, Folder, Item, KeyValue, Method, MultipartField, MultipartValue,
    OAuth2Config, OAuth2Grant, Request, Workspace,
};
use serde_json::Value;

pub fn import(doc: &Value) -> Result<Imported, ImportError> {
    let info = doc
        .get("info")
        .ok_or_else(|| ImportError::invalid("Postman collection", "missing info"))?;
    let schema = str_of(info, "schema");
    let mut warnings = Vec::new();
    if schema.contains("v2.0.0") {
        warnings.push("Postman v2.0 collection imported with v2.1 rules".into());
    } else if schema.contains("v1") {
        return Err(ImportError::invalid(
            "Postman collection",
            "collection format v1 is not supported; re-export as v2.1",
        ));
    }
    let name = match str_of(info, "name") {
        "" => "Postman import",
        n => n,
    };
    let mut workspace = Workspace::new(name);
    workspace.description = description(info.get("description"));
    let items = convert_items(doc.get("item"), &mut warnings);
    match doc.get("auth").map(convert_auth) {
        Some(auth) if auth != Auth::None && auth != Auth::Inherit => {
            // Collection-level auth lives on a root folder so requests can inherit it.
            let mut root = Folder::new(name);
            root.auth = auth;
            root.children = items;
            workspace.items.push(Item::Folder(root));
        }
        _ => workspace.items = items,
    }
    for var in doc
        .get("variable")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let key = str_of(var, "key");
        if !key.is_empty() {
            let value = match var.get("value") {
                Some(Value::String(s)) => Value::String(convert_template(s)),
                Some(other) => other.clone(),
                None => Value::String(String::new()),
            };
            workspace
                .base_environment
                .data
                .insert(key.to_owned(), value);
        }
    }
    Ok(Imported {
        format: Format::PostmanV21,
        workspace,
        warnings,
    })
}

fn convert_items(list: Option<&Value>, warnings: &mut Vec<String>) -> Vec<Item> {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            if entry.get("item").is_some() {
                let mut folder = Folder::new(match str_of(entry, "name") {
                    "" => "Folder",
                    n => n,
                });
                folder.description = description(entry.get("description"));
                folder.auth = entry.get("auth").map(convert_auth).unwrap_or(Auth::Inherit);
                folder.children = convert_items(entry.get("item"), warnings);
                Some(Item::Folder(folder))
            } else if let Some(request) = entry.get("request") {
                Some(Item::Request(convert_request(
                    str_of(entry, "name"),
                    request,
                    warnings,
                )))
            } else {
                warnings.push(format!(
                    "skipped item \"{}\" without request",
                    str_of(entry, "name")
                ));
                None
            }
        })
        .collect()
}

fn convert_request(name: &str, r: &Value, warnings: &mut Vec<String>) -> Request {
    // A request may be a bare URL string.
    if let Value::String(url) = r {
        let (url, params) = split_query(&convert_template(url));
        let mut req = Request::new(name, Method::Get, url);
        req.params = params;
        req.auth = Auth::Inherit;
        return req;
    }
    let method = Method::from(match str_of(r, "method") {
        "" => "GET".to_owned(),
        m => m.to_owned(),
    });
    let (url, params) = match r.get("url") {
        Some(Value::String(raw)) => split_query(&convert_template(raw)),
        Some(obj @ Value::Object(_)) => {
            let raw = convert_template(str_of(obj, "raw"));
            let (base, from_raw) = split_query(&raw);
            let query: Vec<KeyValue> = obj
                .get("query")
                .and_then(Value::as_array)
                .map(|q| {
                    q.iter()
                        .map(|p| KeyValue {
                            name: convert_template(str_of(p, "key")),
                            value: convert_template(str_of(p, "value")),
                            enabled: !p.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                            description: description(p.get("description")),
                        })
                        .collect()
                })
                .unwrap_or(from_raw);
            (base, query)
        }
        _ => (String::new(), Vec::new()),
    };
    let mut req = Request::new(if name.is_empty() { "Request" } else { name }, method, url);
    req.params = params;
    req.headers = r
        .get("header")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|h| !str_of(h, "key").is_empty())
        .map(|h| KeyValue {
            name: str_of(h, "key").to_owned(),
            value: convert_template(str_of(h, "value")),
            enabled: !h.get("disabled").and_then(Value::as_bool).unwrap_or(false),
            description: description(h.get("description")),
        })
        .collect();
    req.description = description(r.get("description"));
    req.auth = r.get("auth").map(convert_auth).unwrap_or(Auth::Inherit);
    if let Some(body) = r.get("body") {
        req.body = convert_body(body, &req.name, warnings);
    }
    req
}

fn convert_body(body: &Value, name: &str, warnings: &mut Vec<String>) -> Body {
    if body.get("disabled").and_then(Value::as_bool) == Some(true) {
        return Body::None;
    }
    let fields = |key: &str| -> Vec<KeyValue> {
        body.get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .map(|p| KeyValue {
                name: str_of(p, "key").to_owned(),
                value: convert_template(str_of(p, "value")),
                enabled: !p.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                description: String::new(),
            })
            .collect()
    };
    match str_of(body, "mode") {
        "raw" => {
            let text = convert_template(str_of(body, "raw"));
            match body
                .pointer("/options/raw/language")
                .and_then(Value::as_str)
            {
                Some("json") => Body::Json { text },
                Some("xml") => Body::Xml { text },
                Some("html") => Body::Text {
                    text,
                    content_type: Some("text/html".into()),
                },
                Some("javascript") => Body::Text {
                    text,
                    content_type: Some("application/javascript".into()),
                },
                _ => Body::Text {
                    text,
                    content_type: None,
                },
            }
        }
        "urlencoded" => Body::FormUrlEncoded {
            fields: fields("urlencoded"),
        },
        "formdata" => Body::Multipart {
            fields: body
                .get("formdata")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|p| MultipartField {
                    name: str_of(p, "key").to_owned(),
                    value: if str_of(p, "type") == "file" {
                        let src = match p.get("src") {
                            Some(Value::Array(a)) => {
                                a.first().and_then(Value::as_str).unwrap_or("")
                            }
                            Some(Value::String(s)) => s,
                            _ => "",
                        };
                        MultipartValue::File {
                            path: src.to_owned(),
                            content_type: p
                                .get("contentType")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        }
                    } else {
                        MultipartValue::Text {
                            value: convert_template(str_of(p, "value")),
                        }
                    },
                    enabled: !p.get("disabled").and_then(Value::as_bool).unwrap_or(false),
                })
                .collect(),
        },
        "file" => Body::Binary {
            path: body
                .pointer("/file/src")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
        },
        "graphql" => Body::GraphQl {
            query: body
                .pointer("/graphql/query")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            variables: body
                .pointer("/graphql/variables")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_owned(),
            operation_name: None,
        },
        "" => Body::None,
        other => {
            warnings.push(format!("{name}: unsupported body mode {other}"));
            Body::None
        }
    }
}

/// Postman stores auth parameters as `[{key, value}]` lists per type.
fn convert_auth(a: &Value) -> Auth {
    let kind = str_of(a, "type");
    let get = |key: &str| -> String {
        a.get(kind)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|p| str_of(p, "key") == key)
            .and_then(|p| p.get("value"))
            .map(|v| match v {
                Value::String(s) => convert_template(s),
                other => other.to_string(),
            })
            .unwrap_or_default()
    };
    match kind {
        "noauth" => Auth::None,
        "basic" => Auth::Basic {
            username: get("username"),
            password: get("password"),
        },
        "bearer" => Auth::Bearer {
            token: get("token"),
            prefix: "Bearer".into(),
        },
        "digest" => Auth::Digest {
            username: get("username"),
            password: get("password"),
        },
        "apikey" => Auth::ApiKey {
            key: get("key"),
            value: get("value"),
            location: if get("in") == "query" {
                ApiKeyLocation::Query
            } else {
                ApiKeyLocation::Header
            },
        },
        "awsv4" => Auth::AwsSigV4 {
            access_key_id: get("accessKey"),
            secret_access_key: get("secretKey"),
            session_token: get("sessionToken"),
            region: get("region"),
            service: get("service"),
        },
        "oauth2" => Auth::OAuth2(Box::new(OAuth2Config {
            grant: match get("grant_type").as_str() {
                "client_credentials" => OAuth2Grant::ClientCredentials,
                "password_credentials" => OAuth2Grant::Password,
                "implicit" => OAuth2Grant::Implicit,
                _ => OAuth2Grant::AuthorizationCode,
            },
            authorization_url: get("authUrl"),
            access_token_url: get("accessTokenUrl"),
            client_id: get("clientId"),
            client_secret: get("clientSecret"),
            redirect_url: get("redirect_uri"),
            scope: get("scope"),
            username: get("username"),
            password: get("password"),
            use_pkce: get("grant_type") == "authorization_code_with_pkce",
            header_prefix: "Bearer".into(),
            ..Default::default()
        })),
        _ => Auth::Inherit,
    }
}

fn description(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(obj) => str_of(obj, "content").to_owned(),
        None => String::new(),
    }
}

/// Maps Postman dynamic variables to template tags.
fn convert_template(text: &str) -> String {
    if !text.contains("{{$") {
        return text.to_owned();
    }
    text.replace("{{$guid}}", "{% uuid %}")
        .replace("{{$randomUUID}}", "{% uuid %}")
        .replace("{{$timestamp}}", "{% now 'unix' %}")
        .replace("{{$isoTimestamp}}", "{% now 'iso' %}")
        .replace("{{$randomInt}}", "{% random_int 0, 1000 %}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::import;
    use serde_json::json;

    #[test]
    fn imports_collection() {
        let doc = json!({
            "info": {"name": "Demo", "schema": "https://schema.getpostman.com/json/collection/v2.1.0/collection.json"},
            "auth": {"type": "bearer", "bearer": [{"key": "token", "value": "{{token}}"}]},
            "variable": [{"key": "base", "value": "https://api.test"}],
            "item": [
                {"name": "Users", "item": [
                    {"name": "Create user", "request": {
                        "method": "POST",
                        "header": [{"key": "X-Id", "value": "{{$guid}}"}],
                        "url": {"raw": "{{base}}/users?x=1", "query": [{"key": "x", "value": "1"}, {"key": "y", "value": "2", "disabled": true}]},
                        "body": {"mode": "raw", "raw": "{\"a\":1}", "options": {"raw": {"language": "json"}}}
                    }},
                    {"name": "Upload", "request": {
                        "method": "POST", "url": "{{base}}/upload",
                        "auth": {"type": "basic", "basic": [{"key": "username", "value": "u"}, {"key": "password", "value": "p"}]},
                        "body": {"mode": "formdata", "formdata": [{"key": "f", "type": "file", "src": "/tmp/a.txt"}, {"key": "t", "value": "v", "type": "text"}]}
                    }}
                ]},
                {"name": "Ping", "request": "{{base}}/ping"}
            ]
        });
        let imported = import(&doc.to_string()).unwrap();
        assert_eq!(imported.format, Format::PostmanV21);
        let ws = imported.workspace;
        assert_eq!(ws.base_environment.data["base"], "https://api.test");
        let Item::Folder(root) = &ws.items[0] else {
            panic!()
        };
        assert_eq!(
            root.auth,
            Auth::Bearer {
                token: "{{token}}".into(),
                prefix: "Bearer".into()
            }
        );
        let Item::Folder(users) = &root.children[0] else {
            panic!()
        };
        let Item::Request(create) = &users.children[0] else {
            panic!()
        };
        assert_eq!(create.url, "{{base}}/users");
        assert_eq!(create.params.len(), 2);
        assert!(!create.params[1].enabled);
        assert_eq!(create.headers[0].value, "{% uuid %}");
        assert_eq!(
            create.body,
            Body::Json {
                text: "{\"a\":1}".into()
            }
        );
        assert_eq!(create.auth, Auth::Inherit);
        let Item::Request(upload) = &users.children[1] else {
            panic!()
        };
        assert_eq!(
            upload.auth,
            Auth::Basic {
                username: "u".into(),
                password: "p".into()
            }
        );
        let Body::Multipart { fields } = &upload.body else {
            panic!()
        };
        assert_eq!(
            fields[0].value,
            MultipartValue::File {
                path: "/tmp/a.txt".into(),
                content_type: None
            }
        );
        let Item::Request(ping) = &root.children[1] else {
            panic!()
        };
        assert_eq!(ping.method, Method::Get);
    }

    #[test]
    fn rejects_v1() {
        let doc = json!({"info": {"name": "x", "schema": "https://schema.getpostman.com/json/collection/v1.0.0/collection.json"}, "item": []});
        assert!(import(&doc.to_string()).is_err());
    }
}
