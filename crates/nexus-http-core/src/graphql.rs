//! GraphQL support: schema introspection and context-aware field completion
//! for the query editor.

use serde_json::Value;
use std::collections::BTreeMap;

/// The standard introspection query (types, fields, arguments, enum values).
pub const INTROSPECTION_QUERY: &str = "query IntrospectionQuery {
  __schema {
    queryType { name }
    mutationType { name }
    subscriptionType { name }
    types {
      kind name description
      fields(includeDeprecated: true) {
        name description isDeprecated
        args { name description type { ...TypeRef } defaultValue }
        type { ...TypeRef }
      }
      inputFields { name description type { ...TypeRef } defaultValue }
      enumValues(includeDeprecated: true) { name description }
    }
  }
}
fragment TypeRef on __Type {
  kind name
  ofType { kind name ofType { kind name ofType { kind name ofType { kind name ofType { kind name } } } } }
}";

/// Sends the introspection query to request `id`'s URL (with its headers and
/// auth) and parses the schema.
pub fn fetch_schema(
    workspace: &crate::model::Workspace,
    id: &crate::model::Id,
    base_dir: &std::path::Path,
    client: &crate::Client,
    history: &crate::history::History,
    cancel: &std::sync::atomic::AtomicBool,
) -> Result<Schema, String> {
    let (folders, request) = workspace
        .find_request(id)
        .ok_or_else(|| format!("request {id} not found"))?;
    let mut introspect = request.clone();
    introspect.method = crate::model::Method::Post;
    introspect.body = crate::model::Body::GraphQl {
        query: INTROSPECTION_QUERY.to_owned(),
        variables: String::new(),
        operation_name: Some("IntrospectionQuery".into()),
    };
    let ctx = crate::template::Context::for_request(workspace, &folders).with_responses(history);
    let prepared =
        crate::exec::prepare_request(&introspect, &folders, &workspace.settings, &ctx, base_dir)
            .map_err(|e| e.to_string())?;
    let response = client
        .send(&prepared, None, cancel)
        .map_err(|e| e.to_string())?;
    if !response.is_success() {
        return Err(format!(
            "introspection returned {} {}",
            response.status, response.reason
        ));
    }
    let value: Value = serde_json::from_slice(&response.body)
        .map_err(|e| format!("introspection response is not JSON: {e}"))?;
    Schema::from_introspection(&value)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Argument {
    pub name: String,
    pub type_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Field {
    pub name: String,
    /// Display form such as `[Product!]!`.
    pub type_ref: String,
    /// The innermost named type (`Product`).
    pub named: String,
    pub args: Vec<Argument>,
    pub description: String,
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDef {
    pub name: String,
    pub kind: String,
    pub description: String,
    pub fields: Vec<Field>,
    pub enum_values: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Schema {
    pub query_type: Option<String>,
    pub mutation_type: Option<String>,
    pub subscription_type: Option<String>,
    pub types: BTreeMap<String, TypeDef>,
}

impl Schema {
    /// Parses an introspection response (`{"data": {"__schema": …}}` or the bare `__schema`).
    pub fn from_introspection(response: &Value) -> Result<Schema, String> {
        if let Some(errors) = response.get("errors").and_then(Value::as_array)
            && !errors.is_empty()
            && response.pointer("/data/__schema").is_none()
        {
            let first = errors[0]
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            return Err(format!("introspection failed: {first}"));
        }
        let schema = response
            .pointer("/data/__schema")
            .or_else(|| response.get("__schema"))
            .ok_or(
                "the response contains no __schema; is introspection disabled on this server?",
            )?;
        let root = |key: &str| {
            schema
                .pointer(&format!("/{key}/name"))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let mut types = BTreeMap::new();
        for t in schema
            .get("types")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(name) = t.get("name").and_then(Value::as_str) else {
                continue;
            };
            let fields = t
                .get("fields")
                .or_else(|| t.get("inputFields"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|f| {
                    let name = f.get("name")?.as_str()?.to_owned();
                    let ty = f.get("type")?;
                    Some(Field {
                        name,
                        type_ref: type_ref(ty),
                        named: named(ty),
                        args: f
                            .get("args")
                            .and_then(Value::as_array)
                            .into_iter()
                            .flatten()
                            .filter_map(|a| {
                                Some(Argument {
                                    name: a.get("name")?.as_str()?.to_owned(),
                                    type_ref: type_ref(a.get("type")?),
                                })
                            })
                            .collect(),
                        description: f
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                        deprecated: f
                            .get("isDeprecated")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    })
                })
                .collect();
            types.insert(
                name.to_owned(),
                TypeDef {
                    name: name.to_owned(),
                    kind: t
                        .get("kind")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    description: t
                        .get("description")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_owned(),
                    fields,
                    enum_values: t
                        .get("enumValues")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(|v| v.get("name").and_then(Value::as_str).map(str::to_owned))
                        .collect(),
                },
            );
        }
        Ok(Schema {
            query_type: root("queryType"),
            mutation_type: root("mutationType"),
            subscription_type: root("subscriptionType"),
            types,
        })
    }

    /// User-facing types (no `__` introspection types), sorted.
    pub fn visible_types(&self) -> impl Iterator<Item = &TypeDef> {
        self.types.values().filter(|t| !t.name.starts_with("__"))
    }

    /// Type name of the selection set enclosing byte offset `cursor`.
    pub fn type_at(&self, query: &str, cursor: usize) -> Option<String> {
        let path = selection_path(&query[..cursor.min(query.len())])?;
        let mut current = match &path.root {
            Root::Operation(op) => match op.as_str() {
                "mutation" => self.mutation_type.clone()?,
                "subscription" => self.subscription_type.clone()?,
                _ => self.query_type.clone()?,
            },
            Root::Fragment(on) => on.clone(),
        };
        for field in &path.fields {
            let def = self.types.get(&current)?;
            current = def.fields.iter().find(|f| &f.name == field)?.named.clone();
        }
        Some(current)
    }

    /// The text completing `field` and where the caret goes in it (chars): fields with
    /// required arguments get `(arg: )` with the caret at the first value, object fields a
    /// selection set `{  }`.
    pub fn insertion(&self, field: &Field) -> (String, usize) {
        let required: Vec<&str> = field
            .args
            .iter()
            .filter(|a| a.type_ref.ends_with('!'))
            .map(|a| a.name.as_str())
            .collect();
        let mut out = field.name.clone();
        let mut caret = None;
        if !required.is_empty() {
            out.push('(');
            caret = Some(out.chars().count() + required[0].len() + 2);
            out.push_str(
                &required
                    .iter()
                    .map(|a| format!("{a}: "))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            out.push(')');
        }
        let is_object = self
            .types
            .get(&field.named)
            .is_some_and(|t| !t.fields.is_empty() && t.kind != "INPUT_OBJECT");
        if is_object {
            out.push_str(" {  }");
            if caret.is_none() {
                caret = Some(out.chars().count() - 2);
            }
        }
        let caret = caret.unwrap_or(out.chars().count());
        (out, caret)
    }

    /// Fields of the selection set at `cursor` that start with the identifier
    /// being typed (case-insensitive), with that partial identifier's byte start.
    pub fn complete(&self, query: &str, cursor: usize) -> Option<(usize, Vec<&Field>)> {
        let cursor = cursor.min(query.len());
        let before = &query[..cursor];
        let start = before
            .char_indices()
            .rev()
            .take_while(|(_, c)| c.is_ascii_alphanumeric() || *c == '_')
            .last()
            .map(|(i, _)| i)
            .unwrap_or(cursor);
        // Inside an argument list `(…)` only arguments make sense; skip.
        if open_parens(&before[..start]) > 0 {
            return None;
        }
        let prefix = before[start..].to_lowercase();
        let type_name = self.type_at(query, start)?;
        let def = self.types.get(&type_name)?;
        let mut fields: Vec<&Field> = def
            .fields
            .iter()
            .filter(|f| {
                f.name.to_lowercase().starts_with(&prefix) && f.name.to_lowercase() != prefix
            })
            .collect();
        fields.sort_by_key(|f| (f.deprecated, f.name.clone()));
        Some((start, fields))
    }
}

fn type_ref(ty: &Value) -> String {
    match ty.get("kind").and_then(Value::as_str) {
        Some("NON_NULL") => format!("{}!", ty.get("ofType").map(type_ref).unwrap_or_default()),
        Some("LIST") => format!("[{}]", ty.get("ofType").map(type_ref).unwrap_or_default()),
        _ => ty
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_owned(),
    }
}

fn named(ty: &Value) -> String {
    match ty.get("ofType") {
        Some(inner) if !inner.is_null() => named(inner),
        _ => ty
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned(),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Root {
    Operation(String),
    Fragment(String),
}

#[derive(Debug, PartialEq, Eq)]
struct SelectionPath {
    root: Root,
    fields: Vec<String>,
}

fn open_parens(text: &str) -> i32 {
    let mut depth = 0;
    for t in tokens(text) {
        match t {
            Token::Punct('(') => depth += 1,
            Token::Punct(')') => depth -= 1,
            Token::Punct('{') | Token::Punct('}') => depth = 0,
            _ => {}
        }
    }
    depth.max(0)
}

#[derive(Debug, PartialEq, Eq)]
enum Token<'a> {
    Name(&'a str),
    Punct(char),
}

/// Lexes names and punctuation, skipping strings, comments and whitespace.
fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if c == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                if bytes[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            out.push(Token::Name(&text[start..i]));
        } else {
            if b"{}()".contains(&c) {
                out.push(Token::Punct(c as char));
            } else if c == b':' {
                out.push(Token::Punct(':'));
            }
            i += 1;
            while i < bytes.len() && !text.is_char_boundary(i) {
                i += 1;
            }
        }
    }
    out
}

/// The stack of fields whose selection sets enclose the end of `text`.
fn selection_path(text: &str) -> Option<SelectionPath> {
    let toks = tokens(text);
    // Stack entries: field name owning each open `{` (None for the operation root).
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut root = Root::Operation("query".into());
    let mut last_name: Option<String> = None;
    let mut prev_name: Option<String> = None;
    let mut paren = 0;
    let mut i = 0;
    while i < toks.len() {
        match &toks[i] {
            Token::Punct('(') => paren += 1,
            Token::Punct(')') => paren -= 1,
            _ if paren > 0 => {}
            Token::Name(n) if stack.is_empty() => {
                match *n {
                    "query" | "mutation" | "subscription" => {
                        root = Root::Operation((*n).to_owned())
                    }
                    "fragment" => {
                        // fragment Name on Type
                        if let (Some(Token::Name("on")), Some(Token::Name(t))) =
                            (toks.get(i + 2), toks.get(i + 3))
                        {
                            root = Root::Fragment((*t).to_owned());
                        }
                    }
                    _ => {}
                }
                last_name = None;
            }
            Token::Name("on") => {
                // Inline fragment `... on Type {`: the selection keeps the parent type.
                last_name = None;
                i += 1;
            }
            Token::Name(n) => {
                prev_name = last_name.take();
                last_name = Some((*n).to_owned());
            }
            Token::Punct(':') => {
                // `alias: field` — the field name follows.
                last_name = None;
                let _ = prev_name.take();
            }
            Token::Punct('{') => {
                if stack.is_empty() {
                    stack.push(None);
                } else {
                    stack.push(last_name.take());
                }
            }
            Token::Punct('}') => {
                stack.pop();
                if stack.is_empty() {
                    root = Root::Operation("query".into());
                }
            }
            _ => {}
        }
        i += 1;
    }
    if stack.is_empty() {
        return None;
    }
    Some(SelectionPath {
        root,
        // `None` entries are inline fragments or the root; they keep the type.
        fields: stack.into_iter().skip(1).flatten().collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub fn sample() -> Value {
        let t = |kind: &str, name: &str| json!({"kind": kind, "name": name, "ofType": null});
        let non_null = |inner: Value| json!({"kind": "NON_NULL", "name": null, "ofType": inner});
        let list = |inner: Value| json!({"kind": "LIST", "name": null, "ofType": inner});
        json!({"data": {"__schema": {
            "queryType": {"name": "Query"},
            "mutationType": {"name": "Mutation"},
            "subscriptionType": null,
            "types": [
                {"kind": "OBJECT", "name": "Query", "fields": [
                    {"name": "products", "args": [{"name": "first", "type": t("SCALAR", "Int")}], "type": non_null(list(non_null(t("OBJECT", "Product"))))},
                    {"name": "product", "args": [{"name": "sku", "type": non_null(t("SCALAR", "String"))}], "type": t("OBJECT", "Product")},
                    {"name": "me", "args": [], "type": t("OBJECT", "User")}
                ]},
                {"kind": "OBJECT", "name": "Mutation", "fields": [
                    {"name": "createProduct", "args": [], "type": t("OBJECT", "Product")}
                ]},
                {"kind": "OBJECT", "name": "Product", "fields": [
                    {"name": "sku", "args": [], "type": non_null(t("SCALAR", "String"))},
                    {"name": "name", "args": [], "type": t("SCALAR", "String")},
                    {"name": "stock", "args": [], "type": t("SCALAR", "Int"), "isDeprecated": false},
                    {"name": "supplier", "args": [], "type": t("OBJECT", "User")}
                ]},
                {"kind": "OBJECT", "name": "User", "fields": [
                    {"name": "email", "args": [], "type": t("SCALAR", "String")}
                ]},
                {"kind": "ENUM", "name": "Status", "fields": null, "enumValues": [{"name": "OPEN"}, {"name": "DONE"}]},
                {"kind": "OBJECT", "name": "__Type", "fields": []}
            ]
        }}})
    }

    #[test]
    fn parses_introspection() {
        let schema = Schema::from_introspection(&sample()).unwrap();
        assert_eq!(schema.query_type.as_deref(), Some("Query"));
        let products = &schema.types["Query"].fields[0];
        assert_eq!(products.type_ref, "[Product!]!");
        assert_eq!(products.named, "Product");
        assert_eq!(products.args[0].name, "first");
        assert_eq!(schema.types["Status"].enum_values, ["OPEN", "DONE"]);
        assert!(schema.visible_types().all(|t| !t.name.starts_with("__")));
        let err =
            Schema::from_introspection(&json!({"errors": [{"message": "introspection disabled"}]}))
                .unwrap_err();
        assert!(err.contains("introspection disabled"));
    }

    #[test]
    fn resolves_types_and_completes_fields() {
        let schema = Schema::from_introspection(&sample()).unwrap();
        let q = "query Inv($n: Int) {\n  products(first: $n) {\n    sku\n    supplier { em";
        assert_eq!(schema.type_at(q, q.len()).as_deref(), Some("User"));
        let (start, fields) = schema.complete(q, q.len()).unwrap();
        assert_eq!(&q[start..], "em");
        assert_eq!(
            fields.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            ["email"]
        );

        let q2 = "{ products { s";
        let names: Vec<_> = schema
            .complete(q2, q2.len())
            .unwrap()
            .1
            .iter()
            .map(|f| f.name.clone())
            .collect();
        assert_eq!(names, ["sku", "stock", "supplier"]);

        let m = "mutation { cr";
        assert_eq!(
            schema.complete(m, m.len()).unwrap().1[0].name,
            "createProduct"
        );

        let frag = "fragment P on Product { na";
        assert_eq!(schema.complete(frag, frag.len()).unwrap().1[0].name, "name");

        let aliased = "{ first: product(sku: \"a{b\") { # comment {\n su";
        assert_eq!(
            schema.type_at(aliased, aliased.len()).as_deref(),
            Some("Product")
        );

        let in_args = "{ product(s";
        assert!(schema.complete(in_args, in_args.len()).is_none());
        assert!(schema.complete("", 0).is_none());
        let inline = "{ me { ... on User { e";
        assert_eq!(
            schema.type_at(inline, inline.len()).as_deref(),
            Some("User")
        );
    }
}

#[cfg(test)]
mod insertion_tests {
    use super::*;
    use serde_json::Value;

    #[test]
    fn insertion_adds_arguments_and_selection_sets() {
        let t = |kind: &str, name: &str| serde_json::json!({"kind": kind, "name": name, "ofType": null});
        let nn =
            |inner: Value| serde_json::json!({"kind": "NON_NULL", "name": null, "ofType": inner});
        let doc = serde_json::json!({"data": {"__schema": {"queryType": {"name": "Query"}, "types": [
            {"kind": "OBJECT", "name": "Query", "fields": [
                {"name": "product", "args": [{"name": "sku", "type": nn(t("SCALAR", "String"))}], "type": t("OBJECT", "Product")},
                {"name": "me", "args": [], "type": t("OBJECT", "Product")}
            ]},
            {"kind": "OBJECT", "name": "Product", "fields": [{"name": "sku", "args": [], "type": t("SCALAR", "String")}]}
        ]}}});
        let schema = Schema::from_introspection(&doc).expect("schema");
        let field = |ty: &str, name: &str| {
            schema.types[ty]
                .fields
                .iter()
                .find(|f| f.name == name)
                .expect("field")
                .clone()
        };
        assert_eq!(
            schema.insertion(&field("Query", "product")),
            ("product(sku: ) {  }".into(), 13)
        );
        assert_eq!(
            schema.insertion(&field("Product", "sku")),
            ("sku".into(), 3)
        );
        assert_eq!(
            schema.insertion(&field("Query", "me")),
            ("me {  }".into(), 5)
        );
    }
}
