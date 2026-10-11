//! OpenAPI 3.x importer: one request per operation, grouped into folders by
//! tag, with a `base_url` variable from `servers`, path parameters as template
//! variables, example bodies (from examples or generated from schemas) and auth
//! from security schemes.

use super::{Format, ImportError, Imported, str_of};
use crate::model::{
    ApiKeyLocation, Auth, Body, Folder, Item, KeyValue, Method, MultipartField, MultipartValue,
    OAuth2Config, OAuth2Grant, Request, Workspace,
};
use serde_json::{Map, Value, json};

const METHODS: [&str; 8] = [
    "get", "put", "post", "delete", "options", "head", "patch", "trace",
];
const MAX_DEPTH: usize = 8;

pub fn import(doc: &Value) -> Result<Imported, ImportError> {
    let paths = doc
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| ImportError::invalid("OpenAPI document", "missing paths"))?;
    let mut warnings = Vec::new();
    let title = doc
        .pointer("/info/title")
        .and_then(Value::as_str)
        .unwrap_or("OpenAPI import");
    let mut workspace = Workspace::new(title);
    workspace.description = doc
        .pointer("/info/description")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_owned();
    let vars = &mut workspace.base_environment.data;
    vars.insert("base_url".into(), Value::String(server_url(doc)));

    // Folders in the order of the document's `tags`, then first appearance.
    let mut folders: Vec<Folder> = doc
        .get("tags")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .map(|name| {
            let mut f = Folder::new(name);
            f.auth = Auth::Inherit;
            f
        })
        .collect();
    let mut loose: Vec<Item> = Vec::new();

    for (path, path_item) in paths {
        let path_item = resolve(doc, path_item, 0);
        let shared_params = path_item.get("parameters").cloned().unwrap_or(Value::Null);
        for method in METHODS {
            let Some(op) = path_item.get(method) else {
                continue;
            };
            let request = operation(doc, path, method, op, &shared_params, vars, &mut warnings);
            let tag = op
                .get("tags")
                .and_then(Value::as_array)
                .and_then(|t| t.first())
                .and_then(Value::as_str);
            match tag {
                Some(tag) => {
                    let index = match folders.iter().position(|f| f.name == tag) {
                        Some(i) => i,
                        None => {
                            let mut f = Folder::new(tag);
                            f.auth = Auth::Inherit;
                            folders.push(f);
                            folders.len() - 1
                        }
                    };
                    folders[index].children.push(Item::Request(request));
                }
                None => loose.push(Item::Request(request)),
            }
        }
    }
    workspace.items = folders
        .into_iter()
        .filter(|f| !f.children.is_empty())
        .map(Item::Folder)
        .chain(loose)
        .collect();
    Ok(Imported {
        format: Format::OpenApi3,
        workspace,
        warnings,
    })
}

fn server_url(doc: &Value) -> String {
    let Some(server) = doc.pointer("/servers/0") else {
        return "http://localhost".into();
    };
    let mut url = str_of(server, "url").to_owned();
    if let Some(vars) = server.get("variables").and_then(Value::as_object) {
        for (name, var) in vars {
            url = url.replace(&format!("{{{name}}}"), str_of(var, "default"));
        }
    }
    url.trim_end_matches('/').to_owned()
}

fn operation(
    doc: &Value,
    path: &str,
    method: &str,
    op: &Value,
    shared_params: &Value,
    vars: &mut Map<String, Value>,
    warnings: &mut Vec<String>,
) -> Request {
    let method_enum = Method::from(method.to_owned());
    let name = [str_of(op, "summary"), str_of(op, "operationId")]
        .into_iter()
        .find(|s| !s.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| format!("{} {path}", method.to_ascii_uppercase()));

    // `/pets/{petId}` → `{{ _.petId }}`.
    let mut url = String::from("{{ _.base_url }}");
    let mut rest = path;
    while let Some(start) = rest.find('{') {
        url.push_str(&rest[..start]);
        match rest[start..].find('}') {
            Some(end) => {
                let var = &rest[start + 1..start + end];
                url.push_str(&format!("{{{{ _.{var} }}}}"));
                vars.entry(var.to_owned())
                    .or_insert(Value::String(String::new()));
                rest = &rest[start + end + 1..];
            }
            None => {
                url.push_str(&rest[start..]);
                rest = "";
            }
        }
    }
    url.push_str(rest);

    let mut request = Request::new(name, method_enum, url);
    request.description = str_of(op, "description").to_owned();
    request.auth = Auth::None;

    let params = shared_params.as_array().into_iter().flatten().chain(
        op.get("parameters")
            .and_then(Value::as_array)
            .into_iter()
            .flatten(),
    );
    for param in params {
        let param = resolve(doc, param, 0);
        let name = str_of(&param, "name");
        let value = param
            .get("example")
            .or_else(|| param.pointer("/schema/example"))
            .or_else(|| param.pointer("/schema/default"))
            .or_else(|| param.pointer("/schema/enum/0"))
            .map(crate::template::value_to_text)
            .unwrap_or_default();
        let required = param
            .get("required")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let kv = KeyValue {
            name: name.to_owned(),
            value: value.clone(),
            enabled: required || !value.is_empty(),
            description: str_of(&param, "description").to_owned(),
        };
        match str_of(&param, "in") {
            "query" => request.params.push(kv),
            "header" => request.headers.push(kv),
            "path" if !value.is_empty() => {
                vars.insert(name.to_owned(), Value::String(value));
            }
            _ => {}
        }
    }

    if let Some(body) = op.get("requestBody").map(|b| resolve(doc, b, 0)) {
        request.body = request_body(doc, &body, warnings, &request.name);
    }

    let security = op.get("security").or_else(|| doc.get("security"));
    request.auth = security_auth(doc, security, vars);
    request
}

fn request_body(doc: &Value, body: &Value, warnings: &mut Vec<String>, name: &str) -> Body {
    let Some(content) = body.get("content").and_then(Value::as_object) else {
        return Body::None;
    };
    let example = |media: &Value| -> Value {
        media
            .get("example")
            .cloned()
            .or_else(|| {
                media
                    .get("examples")
                    .and_then(Value::as_object)
                    .and_then(|e| e.values().next())
                    .map(|e| resolve(doc, e, 0))
                    .and_then(|e| e.get("value").cloned())
            })
            .unwrap_or_else(|| {
                media
                    .get("schema")
                    .map(|s| sample(doc, s, 0))
                    .unwrap_or(Value::Null)
            })
    };
    if let Some((_, media)) = content.iter().find(|(ct, _)| ct.contains("json")) {
        let value = example(media);
        return Body::Json {
            text: serde_json::to_string_pretty(&value).unwrap_or_default(),
        };
    }
    if let Some(media) = content.get("application/x-www-form-urlencoded") {
        let value = example(media);
        return Body::FormUrlEncoded {
            fields: value
                .as_object()
                .into_iter()
                .flatten()
                .map(|(k, v)| KeyValue::new(k, crate::template::value_to_text(v)))
                .collect(),
        };
    }
    if let Some(media) = content.get("multipart/form-data") {
        let schema = media
            .get("schema")
            .map(|s| resolve(doc, s, 0))
            .unwrap_or(Value::Null);
        return Body::Multipart {
            fields: schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(k, prop)| {
                    let prop = resolve(doc, prop, 0);
                    let is_file =
                        str_of(&prop, "format") == "binary" || str_of(&prop, "format") == "base64";
                    MultipartField {
                        name: k.clone(),
                        value: if is_file {
                            MultipartValue::File {
                                path: String::new(),
                                content_type: None,
                            }
                        } else {
                            MultipartValue::Text {
                                value: crate::template::value_to_text(&sample(doc, &prop, 0)),
                            }
                        },
                        enabled: true,
                    }
                })
                .collect(),
        };
    }
    if content.keys().any(|ct| ct.contains("xml")) {
        return Body::Xml {
            text: String::new(),
        };
    }
    if let Some((ct, _)) = content.iter().next() {
        if ct == "application/octet-stream" {
            return Body::Binary {
                path: String::new(),
            };
        }
        warnings.push(format!("{name}: request body {ct} imported as empty text"));
        return Body::Text {
            text: String::new(),
            content_type: Some(ct.clone()),
        };
    }
    Body::None
}

fn security_auth(doc: &Value, security: Option<&Value>, vars: &mut Map<String, Value>) -> Auth {
    let Some(requirement) = security
        .and_then(Value::as_array)
        .and_then(|list| list.first())
        .and_then(Value::as_object)
    else {
        return Auth::None;
    };
    let Some((scheme_name, scopes)) = requirement.iter().next() else {
        return Auth::None;
    };
    let Some(scheme) = doc
        .pointer(&format!(
            "/components/securitySchemes/{}",
            escape_pointer(scheme_name)
        ))
        .map(|s| resolve(doc, s, 0))
    else {
        return Auth::None;
    };
    let mut var = |name: &str| {
        vars.entry(name.to_owned())
            .or_insert(Value::String(String::new()));
        format!("{{{{ _.{name} }}}}")
    };
    match (
        str_of(&scheme, "type"),
        str_of(&scheme, "scheme").to_ascii_lowercase().as_str(),
    ) {
        ("http", "basic") => Auth::Basic {
            username: var("username"),
            password: var("password"),
        },
        ("http", "bearer") => Auth::Bearer {
            token: var("token"),
            prefix: "Bearer".into(),
        },
        ("http", "digest") => Auth::Digest {
            username: var("username"),
            password: var("password"),
        },
        ("apiKey", _) => Auth::ApiKey {
            key: str_of(&scheme, "name").to_owned(),
            value: var("api_key"),
            location: match str_of(&scheme, "in") {
                "query" => ApiKeyLocation::Query,
                "cookie" => ApiKeyLocation::Cookie,
                _ => ApiKeyLocation::Header,
            },
        },
        ("oauth2", _) | ("openIdConnect", _) => {
            let flows = scheme.get("flows").cloned().unwrap_or(Value::Null);
            let (grant, flow) = [
                ("authorizationCode", OAuth2Grant::AuthorizationCode),
                ("clientCredentials", OAuth2Grant::ClientCredentials),
                ("password", OAuth2Grant::Password),
                ("implicit", OAuth2Grant::Implicit),
            ]
            .into_iter()
            .find_map(|(key, grant)| flows.get(key).map(|f| (grant, f.clone())))
            .unwrap_or((OAuth2Grant::AuthorizationCode, Value::Null));
            let scope = scopes
                .as_array()
                .map(|s| {
                    s.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .unwrap_or_default();
            Auth::OAuth2(Box::new(OAuth2Config {
                grant,
                authorization_url: str_of(&flow, "authorizationUrl").to_owned(),
                access_token_url: str_of(&flow, "tokenUrl").to_owned(),
                client_id: var("client_id"),
                client_secret: var("client_secret"),
                redirect_url: "http://127.0.0.1:8765/callback".into(),
                scope,
                use_pkce: true,
                header_prefix: "Bearer".into(),
                ..Default::default()
            }))
        }
        _ => Auth::None,
    }
}

fn escape_pointer(s: &str) -> String {
    s.replace('~', "~0").replace('/', "~1")
}

/// Follows `$ref` (local references only).
fn resolve(doc: &Value, value: &Value, depth: usize) -> Value {
    match value.get("$ref").and_then(Value::as_str) {
        Some(reference) if depth < MAX_DEPTH => match reference.strip_prefix('#') {
            Some(pointer) => doc
                .pointer(pointer)
                .map(|target| resolve(doc, target, depth + 1))
                .unwrap_or(Value::Null),
            None => Value::Null,
        },
        Some(_) => Value::Null,
        None => value.clone(),
    }
}

/// A plausible example value for a schema.
fn sample(doc: &Value, schema: &Value, depth: usize) -> Value {
    if depth > MAX_DEPTH {
        return Value::Null;
    }
    let schema = resolve(doc, schema, 0);
    for key in ["example", "default"] {
        if let Some(v) = schema.get(key) {
            return v.clone();
        }
    }
    if let Some(v) = schema.pointer("/enum/0") {
        return v.clone();
    }
    if let Some(all) = schema.get("allOf").and_then(Value::as_array) {
        let mut merged = Map::new();
        for part in all {
            if let Value::Object(m) = sample(doc, part, depth + 1) {
                merged.extend(m);
            }
        }
        return Value::Object(merged);
    }
    for key in ["oneOf", "anyOf"] {
        if let Some(first) = schema.get(key).and_then(|v| v.get(0)) {
            return sample(doc, first, depth + 1);
        }
    }
    let kind = match schema.get("type") {
        Some(Value::String(t)) => t.as_str(),
        Some(Value::Array(types)) => types
            .iter()
            .filter_map(Value::as_str)
            .find(|t| *t != "null")
            .unwrap_or("null"),
        _ if schema.get("properties").is_some() => "object",
        _ => "",
    };
    match kind {
        "object" => Value::Object(
            schema
                .get("properties")
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
                .map(|(k, v)| (k.clone(), sample(doc, v, depth + 1)))
                .collect(),
        ),
        "array" => match schema.get("items") {
            Some(items) => Value::Array(vec![sample(doc, items, depth + 1)]),
            None => json!([]),
        },
        "string" => Value::String(
            match str_of(&schema, "format") {
                "date-time" => "2024-01-01T00:00:00Z",
                "date" => "2024-01-01",
                "email" => "user@example.com",
                "uuid" => "00000000-0000-0000-0000-000000000000",
                "uri" | "url" => "https://example.com",
                _ => "string",
            }
            .to_owned(),
        ),
        "integer" => json!(0),
        "number" => json!(0.0),
        "boolean" => json!(true),
        _ => Value::Null,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::import;

    const PETSTORE: &str = r##"
openapi: 3.0.3
info:
  title: Petstore
servers:
  - url: https://{env}.petstore.test/v1/
    variables:
      env:
        default: api
tags:
  - name: pets
security:
  - bearerAuth: []
paths:
  /pets:
    get:
      summary: List pets
      tags: [pets]
      parameters:
        - $ref: '#/components/parameters/Limit'
        - name: X-Request-Id
          in: header
          schema: {type: string, format: uuid}
    post:
      operationId: createPet
      tags: [pets]
      requestBody:
        content:
          application/json:
            schema:
              $ref: '#/components/schemas/NewPet'
  /pets/{petId}:
    parameters:
      - name: petId
        in: path
        required: true
        schema: {type: integer, example: 7}
    delete:
      tags: [pets]
      security:
        - apiKey: []
  /health:
    get: {}
components:
  parameters:
    Limit:
      name: limit
      in: query
      schema: {type: integer, default: 20}
  schemas:
    NewPet:
      type: object
      properties:
        name: {type: string, example: Rex}
        born: {type: string, format: date}
        tags:
          type: array
          items: {type: string}
  securitySchemes:
    bearerAuth: {type: http, scheme: bearer}
    apiKey: {type: apiKey, in: header, name: X-Api-Key}
"##;

    #[test]
    fn imports_petstore() {
        let imported = import(PETSTORE).unwrap();
        assert_eq!(imported.format, Format::OpenApi3);
        let ws = imported.workspace;
        assert_eq!(ws.name, "Petstore");
        assert_eq!(
            ws.base_environment.data["base_url"],
            "https://api.petstore.test/v1"
        );
        assert_eq!(ws.base_environment.data["petId"], "7");
        let Item::Folder(pets) = &ws.items[0] else {
            panic!()
        };
        assert_eq!(pets.children.len(), 3);
        let Item::Request(list) = &pets.children[0] else {
            panic!()
        };
        assert_eq!(list.name, "List pets");
        assert_eq!(list.url, "{{ _.base_url }}/pets");
        assert_eq!(list.params, vec![KeyValue::new("limit", "20")]);
        assert_eq!(list.headers[0].name, "X-Request-Id");
        assert_eq!(
            list.auth,
            Auth::Bearer {
                token: "{{ _.token }}".into(),
                prefix: "Bearer".into()
            }
        );
        let Item::Request(create) = &pets.children[1] else {
            panic!()
        };
        assert_eq!(create.name, "createPet");
        let Body::Json { text } = &create.body else {
            panic!()
        };
        let body: Value = serde_json::from_str(text).unwrap();
        assert_eq!(
            body,
            json!({"name": "Rex", "born": "2024-01-01", "tags": ["string"]})
        );
        let Item::Request(delete) = &pets.children[2] else {
            panic!()
        };
        assert_eq!(delete.url, "{{ _.base_url }}/pets/{{ _.petId }}");
        assert!(matches!(delete.auth, Auth::ApiKey { .. }));
        let Item::Request(health) = &ws.items[1] else {
            panic!()
        };
        assert_eq!(health.name, "GET /health");
    }

    #[test]
    fn survives_cyclic_refs() {
        let doc = json!({
            "openapi": "3.1.0", "info": {"title": "Cyclic"},
            "paths": {"/n": {"post": {"requestBody": {"content": {"application/json": {"schema": {"$ref": "#/components/schemas/Node"}}}}}}},
            "components": {"schemas": {"Node": {"type": "object", "properties": {"next": {"$ref": "#/components/schemas/Node"}}}}}
        });
        let imported = import(&doc.to_string()).unwrap();
        let Item::Request(req) = &imported.workspace.items[0] else {
            panic!()
        };
        assert!(matches!(req.body, Body::Json { .. }));
    }
}
