//! Response filters: JSONPath (RFC 9535) over JSON bodies, used by the response
//! viewer's filter box, `{% response %}` chaining and assertions.

use serde_json::Value;
use serde_json_path::JsonPath;

/// Evaluates `filter` against a JSON document and returns the matches.
pub fn json_path(document: &Value, filter: &str) -> Result<Vec<Value>, String> {
    let path = JsonPath::parse(filter.trim()).map_err(|e| format!("invalid JSONPath: {e}"))?;
    Ok(path.query(document).all().into_iter().cloned().collect())
}

/// Filters a JSON body. An empty filter returns the whole body pretty-printed;
/// a single match is returned on its own, multiple matches as an array.
pub fn filter_json_text(body: &str, filter: &str) -> Result<String, String> {
    let document: Value =
        serde_json::from_str(body).map_err(|e| format!("response is not valid JSON: {e}"))?;
    if filter.trim().is_empty() {
        return serde_json::to_string_pretty(&document).map_err(|e| e.to_string());
    }
    let matches = json_path(&document, filter)?;
    let value = match matches.len() {
        0 => return Err(format!("no match for {}", filter.trim())),
        1 => matches.into_iter().next().unwrap_or(Value::Null),
        _ => Value::Array(matches),
    };
    serde_json::to_string_pretty(&value).map_err(|e| e.to_string())
}

/// Like [`filter_json_text`] but returns bare strings unquoted, which is what a
/// chained value inserted into a header or URL needs.
pub fn select_scalar(body: &str, filter: &str) -> Result<String, String> {
    let document: Value =
        serde_json::from_str(body).map_err(|e| format!("response is not valid JSON: {e}"))?;
    let matches = json_path(&document, filter)?;
    match matches.as_slice() {
        [] => Err(format!("no match for {}", filter.trim())),
        [single] => Ok(crate::template::value_to_text(single)),
        many => Ok(Value::Array(many.to_vec()).to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = r#"{"token":"abc","users":[{"id":1,"name":"Ada"},{"id":2,"name":"Linus"}]}"#;

    #[test]
    fn selects_scalars_and_lists() {
        assert_eq!(select_scalar(BODY, "$.token").unwrap(), "abc");
        assert_eq!(select_scalar(BODY, "$.users[1].id").unwrap(), "2");
        assert_eq!(
            select_scalar(BODY, "$.users[*].name").unwrap(),
            r#"["Ada","Linus"]"#
        );
    }

    #[test]
    fn filters_pretty_and_reports_errors() {
        assert_eq!(
            filter_json_text(BODY, "$.users[0].name").unwrap(),
            "\"Ada\""
        );
        assert!(filter_json_text(BODY, "").unwrap().starts_with("{\n"));
        assert!(
            filter_json_text(BODY, "$.nope")
                .unwrap_err()
                .contains("no match")
        );
        assert!(
            filter_json_text(BODY, "$[")
                .unwrap_err()
                .contains("invalid JSONPath")
        );
        assert!(
            filter_json_text("nope", "$")
                .unwrap_err()
                .contains("not valid JSON")
        );
    }
}
