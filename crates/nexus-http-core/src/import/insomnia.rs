//! Insomnia export importers: v4 (`_type: export` JSON) and v5 (YAML collection).
//! Field names follow Insomnia's public export format; template syntax is
//! compatible, and base64-encoded `{% response %}` filters are decoded.

use super::{Format, ImportError, Imported, split_query, str_of};
use crate::model::{
    ApiKeyLocation, Auth, Body, Environment, Folder, Id, Item, KeyValue, Method, MultipartField,
    MultipartValue, OAuth2Config, OAuth2Grant, Request, Workspace,
};
use base64::Engine as _;
use serde_json::{Map, Value};
use std::collections::HashMap;

/// Imports an Insomnia v4 export.
pub fn import_v4(doc: &Value) -> Result<Imported, ImportError> {
    let resources = doc
        .get("resources")
        .and_then(Value::as_array)
        .ok_or_else(|| ImportError::invalid("Insomnia v4 export", "missing resources"))?;
    let mut warnings = Vec::new();
    let workspaces: Vec<&Value> = resources
        .iter()
        .filter(|r| str_of(r, "_type") == "workspace")
        .collect();
    let root = workspaces
        .first()
        .ok_or_else(|| ImportError::invalid("Insomnia v4 export", "no workspace resource"))?;
    if workspaces.len() > 1 {
        warnings.push(format!(
            "export contains {} workspaces; imported only \"{}\"",
            workspaces.len(),
            str_of(root, "name")
        ));
    }
    let root_id = str_of(root, "_id");
    let mut children: HashMap<&str, Vec<&Value>> = HashMap::new();
    for r in resources {
        children.entry(str_of(r, "parentId")).or_default().push(r);
    }
    for list in children.values_mut() {
        list.sort_by(|a, b| {
            let ka = a.get("metaSortKey").and_then(Value::as_f64).unwrap_or(0.0);
            let kb = b.get("metaSortKey").and_then(Value::as_f64).unwrap_or(0.0);
            ka.partial_cmp(&kb).unwrap_or(std::cmp::Ordering::Equal)
        });
    }

    let mut workspace = Workspace::new(non_empty(str_of(root, "name"), "Insomnia import"));
    workspace.description = str_of(root, "description").to_owned();
    workspace.items = v4_items(root_id, &children, &mut warnings);

    let base = children
        .get(root_id)
        .into_iter()
        .flatten()
        .find(|r| str_of(r, "_type") == "environment");
    if let Some(base) = base {
        workspace.base_environment = environment(base, "Base Environment");
        for sub in children
            .get(str_of(base, "_id"))
            .into_iter()
            .flatten()
            .filter(|r| str_of(r, "_type") == "environment")
        {
            workspace.environments.push(environment(sub, "Environment"));
        }
    }
    Ok(Imported {
        format: Format::InsomniaV4,
        workspace,
        warnings,
    })
}

fn v4_items(
    parent: &str,
    children: &HashMap<&str, Vec<&Value>>,
    warnings: &mut Vec<String>,
) -> Vec<Item> {
    let mut items = Vec::new();
    for r in children.get(parent).into_iter().flatten() {
        match str_of(r, "_type") {
            "request" => items.push(Item::Request(request(r, str_of(r, "_id"), warnings))),
            "request_group" => {
                let mut folder = folder(r, str_of(r, "_id"));
                folder.children = v4_items(str_of(r, "_id"), children, warnings);
                items.push(Item::Folder(folder));
            }
            "grpc_request" | "websocket_request" | "websocket_payload" => warnings.push(format!(
                "skipped {} \"{}\" (not supported yet)",
                str_of(r, "_type").replace('_', " "),
                str_of(r, "name")
            )),
            _ => {}
        }
    }
    items
}

/// Imports an Insomnia v5 YAML collection (already parsed).
pub fn import_v5(doc: &Value) -> Result<Imported, ImportError> {
    let mut warnings = Vec::new();
    let kind = str_of(doc, "type");
    if !kind.starts_with("collection.") {
        return Err(ImportError::invalid(
            "Insomnia v5 export",
            format!("only collections can be imported, not `{kind}`"),
        ));
    }
    let mut workspace = Workspace::new(non_empty(str_of(doc, "name"), "Insomnia import"));
    workspace.items = v5_items(
        doc.get("collection")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]),
        &mut warnings,
    );
    if let Some(envs) = doc.get("environments") {
        workspace.base_environment = environment(envs, "Base Environment");
        for sub in envs
            .get("subEnvironments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            workspace.environments.push(environment(sub, "Environment"));
        }
    }
    Ok(Imported {
        format: Format::InsomniaV5,
        workspace,
        warnings,
    })
}

fn v5_items(list: &[Value], warnings: &mut Vec<String>) -> Vec<Item> {
    list.iter()
        .map(|entry| {
            let id = entry
                .pointer("/meta/id")
                .and_then(Value::as_str)
                .unwrap_or("");
            if let Some(kids) = entry.get("children").and_then(Value::as_array) {
                let mut f = folder(entry, id);
                f.children = v5_items(kids, warnings);
                Item::Folder(f)
            } else {
                Item::Request(request(entry, id, warnings))
            }
        })
        .collect()
}

fn keep_or_new_id(raw: &str, prefix: &str) -> Id {
    let id = Id::from(raw);
    if id.is_valid() { id } else { Id::new(prefix) }
}

fn folder(r: &Value, id: &str) -> Folder {
    let mut f = Folder::new(non_empty(str_of(r, "name"), "Folder"));
    f.id = keep_or_new_id(id, "fld");
    f.description = str_of(r, "description").to_owned();
    f.headers = pairs(r.get("headers"));
    if let Some(auth) = r
        .get("authentication")
        .filter(|a| a.as_object().is_some_and(|o| !o.is_empty()))
    {
        f.auth = convert_auth(auth);
    }
    if let Some(Value::Object(env)) = r.get("environment") {
        f.variables = convert_map(env);
    }
    f
}

fn request(r: &Value, id: &str, warnings: &mut Vec<String>) -> Request {
    let method = Method::from(non_empty(str_of(r, "method"), "GET").to_owned());
    let (url, mut params) = split_query(&convert_template(str_of(r, "url")));
    let mut req = Request::new(non_empty(str_of(r, "name"), "Request"), method, url);
    req.id = keep_or_new_id(id, "req");
    params.extend(pairs(r.get("parameters")));
    req.params = params;
    req.headers = pairs(r.get("headers"));
    req.description = str_of(r, "description").to_owned();
    if let Some(body) = r.get("body") {
        req.body = convert_body(body, warnings, str_of(r, "name"));
    }
    // Requests default to inheriting auth from their folder.
    req.auth = match r.get("authentication") {
        Some(a) if a.as_object().is_some_and(|o| !o.is_empty()) => convert_auth(a),
        _ => Auth::Inherit,
    };
    let follow = r
        .get("settingFollowRedirects")
        .or_else(|| r.pointer("/settings/followRedirects"))
        .and_then(Value::as_str);
    req.settings.follow_redirects = match follow {
        Some("on") => Some(true),
        Some("off") => Some(false),
        _ => None,
    };
    req
}

fn pairs(list: Option<&Value>) -> Vec<KeyValue> {
    list.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|p| !str_of(p, "name").is_empty())
        .map(|p| KeyValue {
            name: convert_template(str_of(p, "name")),
            value: convert_template(str_of(p, "value")),
            enabled: !p.get("disabled").and_then(Value::as_bool).unwrap_or(false),
            description: str_of(p, "description").to_owned(),
        })
        .collect()
}

fn convert_body(body: &Value, warnings: &mut Vec<String>, name: &str) -> Body {
    let mime = str_of(body, "mimeType").to_ascii_lowercase();
    let text = convert_template(str_of(body, "text"));
    match mime.as_str() {
        "" if text.is_empty() && body.get("fileName").is_none() => Body::None,
        "application/json" => Body::Json { text },
        "application/xml" | "text/xml" => Body::Xml { text },
        "application/x-www-form-urlencoded" => Body::FormUrlEncoded {
            fields: pairs(body.get("params")),
        },
        "multipart/form-data" => Body::Multipart {
            fields: body
                .get("params")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|p| MultipartField {
                    name: str_of(p, "name").to_owned(),
                    value: if str_of(p, "type") == "file" {
                        MultipartValue::File {
                            path: str_of(p, "fileName").to_owned(),
                            content_type: None,
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
        "application/graphql" => {
            let parsed: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            Body::GraphQl {
                query: str_of(&parsed, "query").to_owned(),
                variables: match parsed.get("variables") {
                    Some(Value::Null) | None => String::new(),
                    Some(v) => serde_json::to_string_pretty(v).unwrap_or_default(),
                },
                operation_name: parsed
                    .get("operationName")
                    .and_then(Value::as_str)
                    .map(str::to_owned),
            }
        }
        _ if body
            .get("fileName")
            .and_then(Value::as_str)
            .is_some_and(|f| !f.is_empty()) =>
        {
            Body::Binary {
                path: str_of(body, "fileName").to_owned(),
            }
        }
        "" => Body::Text {
            text,
            content_type: None,
        },
        other => {
            if text.is_empty() && body.get("params").is_some() {
                warnings.push(format!("{name}: body type {other} imported as empty text"));
            }
            Body::Text {
                text,
                content_type: Some(other.to_owned()),
            }
        }
    }
}

fn convert_auth(a: &Value) -> Auth {
    if a.get("disabled").and_then(Value::as_bool) == Some(true) {
        return Auth::None;
    }
    let s = |k: &str| convert_template(str_of(a, k));
    match str_of(a, "type") {
        "basic" => Auth::Basic {
            username: s("username"),
            password: s("password"),
        },
        "bearer" => Auth::Bearer {
            token: s("token"),
            prefix: non_empty(&s("prefix"), "Bearer").to_owned(),
        },
        "digest" => Auth::Digest {
            username: s("username"),
            password: s("password"),
        },
        "apikey" => Auth::ApiKey {
            key: s("key"),
            value: s("value"),
            location: match str_of(a, "addTo") {
                "queryParams" => ApiKeyLocation::Query,
                "cookie" => ApiKeyLocation::Cookie,
                _ => ApiKeyLocation::Header,
            },
        },
        "iam" => Auth::AwsSigV4 {
            access_key_id: s("accessKeyId"),
            secret_access_key: s("secretAccessKey"),
            session_token: s("sessionToken"),
            region: s("region"),
            service: s("service"),
        },
        "oauth2" => Auth::OAuth2(Box::new(OAuth2Config {
            grant: match str_of(a, "grantType") {
                "client_credentials" => OAuth2Grant::ClientCredentials,
                "password" => OAuth2Grant::Password,
                "implicit" => OAuth2Grant::Implicit,
                _ => OAuth2Grant::AuthorizationCode,
            },
            authorization_url: s("authorizationUrl"),
            access_token_url: s("accessTokenUrl"),
            client_id: s("clientId"),
            client_secret: s("clientSecret"),
            redirect_url: s("redirectUrl"),
            scope: s("scope"),
            audience: s("audience"),
            username: s("username"),
            password: s("password"),
            use_pkce: a.get("usePkce").and_then(Value::as_bool).unwrap_or(false),
            credentials_in_header: str_of(a, "credentialsInBody") == "false"
                || a.get("credentialsInBody").and_then(Value::as_bool) == Some(false),
            header_prefix: non_empty(&s("tokenPrefix"), "Bearer").to_owned(),
            token: None,
        })),
        "none" | "" => Auth::None,
        _ => Auth::None,
    }
}

fn environment(r: &Value, fallback: &str) -> Environment {
    let mut env = Environment::new(non_empty(str_of(r, "name"), fallback));
    let id = r
        .get("_id")
        .or_else(|| r.pointer("/meta/id"))
        .and_then(Value::as_str)
        .unwrap_or("");
    env.id = keep_or_new_id(id, "env");
    if let Some(Value::Object(data)) = r.get("data") {
        env.data = convert_map(data);
    }
    env.color = r.get("color").and_then(Value::as_str).map(str::to_owned);
    env
}

fn convert_map(map: &Map<String, Value>) -> Map<String, Value> {
    map.iter()
        .map(|(k, v)| (k.clone(), convert_value(v)))
        .collect()
}

fn convert_value(v: &Value) -> Value {
    match v {
        Value::String(s) => Value::String(convert_template(s)),
        Value::Object(m) => Value::Object(convert_map(m)),
        Value::Array(a) => Value::Array(a.iter().map(convert_value).collect()),
        other => other.clone(),
    }
}

/// Rewrites Insomnia-specific template details: base64-encoded response tag
/// filters (`'b64::JC50b2tlbg==::46b'`) become plain filters, and extra
/// trigger arguments are dropped.
pub(crate) fn convert_template(text: &str) -> String {
    if !text.contains("{%") {
        return text.to_owned();
    }
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("{%") {
        out.push_str(&rest[..start]);
        let Some(end) = rest[start..].find("%}") else {
            out.push_str(&rest[start..]);
            return out;
        };
        let tag = &rest[start + 2..start + end];
        out.push_str(&convert_tag(tag));
        rest = &rest[start + end + 2..];
    }
    out.push_str(rest);
    out
}

fn convert_tag(tag: &str) -> String {
    let trimmed = tag.trim();
    if !trimmed.starts_with("response ") {
        return format!("{{%{tag}%}}");
    }
    let args: Vec<String> = trimmed["response ".len()..]
        .split(',')
        .map(|a| a.trim().trim_matches(|c| c == '\'' || c == '"').to_owned())
        .collect();
    let attribute = args.first().cloned().unwrap_or_default();
    let id = args.get(1).cloned().unwrap_or_default();
    let mut filter = args.get(2).cloned().unwrap_or_default();
    if let Some(encoded) = filter
        .strip_prefix("b64::")
        .and_then(|f| f.split("::").next())
        && let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded)
    {
        filter = String::from_utf8_lossy(&bytes).into_owned();
    }
    let attribute = match attribute.as_str() {
        "raw" => "body".to_owned(),
        other => other.to_owned(),
    };
    format!(
        "{{% response '{attribute}', '{id}', '{}' %}}",
        filter.replace('\'', "\\'")
    )
}

fn non_empty<'a>(s: &'a str, fallback: &'a str) -> &'a str {
    if s.trim().is_empty() { fallback } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::import;
    use serde_json::json;

    #[test]
    fn imports_v4_export() {
        let doc = json!({
            "_type": "export", "__export_format": 4,
            "resources": [
                {"_id": "wrk_1", "_type": "workspace", "name": "Shop API"},
                {"_id": "env_base", "_type": "environment", "parentId": "wrk_1", "name": "Base Environment", "data": {"host": "localhost"}},
                {"_id": "env_prod", "_type": "environment", "parentId": "env_base", "name": "Production", "data": {"host": "shop.example"}, "color": "#f00"},
                {"_id": "fld_1", "_type": "request_group", "parentId": "wrk_1", "name": "Orders",
                    "authentication": {"type": "bearer", "token": "{{ _.token }}"}},
                {"_id": "req_2", "_type": "request", "parentId": "fld_1", "name": "Create", "method": "POST", "metaSortKey": 2,
                    "url": "{{ _.host }}/orders?draft=true",
                    "body": {"mimeType": "application/json", "text": "{\"sku\": 1}"},
                    "headers": [{"name": "X-A", "value": "1"}, {"name": "X-B", "value": "2", "disabled": true}],
                    "authentication": {}},
                {"_id": "req_1", "_type": "request", "parentId": "fld_1", "name": "Login", "method": "POST", "metaSortKey": 1,
                    "url": "{{ _.host }}/login",
                    "body": {"mimeType": "application/x-www-form-urlencoded", "params": [{"name": "user", "value": "ada"}]},
                    "authentication": {"type": "basic", "username": "a", "password": "b"}},
                {"_id": "req_3", "_type": "request", "parentId": "wrk_1", "name": "Me", "method": "GET",
                    "url": "{{ _.host }}/me",
                    "headers": [{"name": "Authorization", "value": "Bearer {% response 'body', 'req_1', 'b64::JC50b2tlbg==::46b', 'never', 60 %}"}]},
                {"_id": "ws_1", "_type": "websocket_request", "parentId": "wrk_1", "name": "Live"}
            ]
        });
        let imported = import(&doc.to_string()).unwrap();
        assert_eq!(imported.format, Format::InsomniaV4);
        let ws = imported.workspace;
        assert_eq!(ws.name, "Shop API");
        assert_eq!(ws.base_environment.data["host"], "localhost");
        assert_eq!(ws.environments[0].name, "Production");
        let Item::Folder(folder) = &ws.items[0] else {
            panic!()
        };
        assert_eq!(
            folder.auth,
            Auth::Bearer {
                token: "{{ _.token }}".into(),
                prefix: "Bearer".into()
            }
        );
        let Item::Request(login) = &folder.children[0] else {
            panic!()
        };
        assert_eq!(login.name, "Login");
        assert!(matches!(login.body, Body::FormUrlEncoded { .. }));
        let Item::Request(create) = &folder.children[1] else {
            panic!()
        };
        assert_eq!(create.id.as_str(), "req_2");
        assert_eq!(create.params, vec![KeyValue::new("draft", "true")]);
        assert!(!create.headers[1].enabled);
        assert_eq!(create.auth, Auth::Inherit);
        let Item::Request(me) = &ws.items[1] else {
            panic!()
        };
        assert_eq!(
            me.headers[0].value,
            "Bearer {% response 'body', 'req_1', '$.token' %}"
        );
        assert_eq!(imported.warnings.len(), 1);
    }

    #[test]
    fn imports_v5_yaml() {
        let yaml = r#"
type: collection.insomnia.rest/5.0
name: Petstore
meta:
  id: wrk_5
collection:
  - name: Pets
    meta:
      id: fld_5
    children:
      - url: "{{ _.base }}/pets"
        name: List pets
        meta:
          id: req_5
        method: GET
        parameters:
          - name: limit
            value: "10"
        authentication:
          type: apikey
          key: X-Key
          value: k
          addTo: queryParams
  - url: "{{ _.base }}/graphql"
    name: Query
    meta:
      id: req_6
    method: POST
    body:
      mimeType: application/graphql
      text: '{"query":"{ pets { id } }","variables":{"a":1}}'
environments:
  name: Base Environment
  meta:
    id: env_5
  data:
    base: http://localhost:8080
  subEnvironments:
    - name: Staging
      meta:
        id: env_6
      data:
        base: https://staging.example
"#;
        let imported = import(yaml).unwrap();
        assert_eq!(imported.format, Format::InsomniaV5);
        let ws = imported.workspace;
        assert_eq!(ws.name, "Petstore");
        assert_eq!(ws.environments[0].data["base"], "https://staging.example");
        let Item::Folder(pets) = &ws.items[0] else {
            panic!()
        };
        let Item::Request(list) = &pets.children[0] else {
            panic!()
        };
        assert_eq!(list.params[0], KeyValue::new("limit", "10"));
        assert_eq!(
            list.auth,
            Auth::ApiKey {
                key: "X-Key".into(),
                value: "k".into(),
                location: ApiKeyLocation::Query
            }
        );
        let Item::Request(gql) = &ws.items[1] else {
            panic!()
        };
        let Body::GraphQl {
            query, variables, ..
        } = &gql.body
        else {
            panic!()
        };
        assert_eq!(query, "{ pets { id } }");
        assert!(variables.contains("\"a\": 1"));
    }
}
