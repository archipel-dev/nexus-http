//! Response history per request. The latest response also feeds
//! `{% response %}` template tags (request chaining).

use crate::filter;
use crate::model::Id;
use crate::response::Response;
use crate::template::{ResponseAttribute, ResponseSource};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Responses kept per request.
pub const DEFAULT_LIMIT: usize = 20;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct History {
    /// Newest last.
    pub entries: BTreeMap<Id, Vec<Response>>,
    #[serde(skip)]
    limit: Option<usize>,
}

impl History {
    pub fn with_limit(limit: usize) -> Self {
        Self {
            entries: BTreeMap::new(),
            limit: Some(limit.max(1)),
        }
    }

    pub fn push(&mut self, id: Id, response: Response) {
        let limit = self.limit.unwrap_or(DEFAULT_LIMIT);
        let list = self.entries.entry(id).or_default();
        list.push(response);
        if list.len() > limit {
            let excess = list.len() - limit;
            list.drain(..excess);
        }
    }

    pub fn latest(&self, id: &Id) -> Option<&Response> {
        self.entries.get(id)?.last()
    }

    /// Newest first.
    pub fn for_request(&self, id: &Id) -> impl Iterator<Item = &Response> {
        self.entries.get(id).into_iter().flatten().rev()
    }

    pub fn clear(&mut self, id: &Id) {
        self.entries.remove(id);
    }
}

impl ResponseSource for History {
    fn response_value(
        &self,
        request_id: &str,
        attribute: ResponseAttribute,
        filter_expr: &str,
    ) -> Result<String, String> {
        let response = self
            .latest(&Id::from(request_id))
            .ok_or_else(|| format!("request {request_id} has no response yet; send it first"))?;
        match attribute {
            ResponseAttribute::Status => Ok(response.status.to_string()),
            ResponseAttribute::Url => Ok(response.url.clone()),
            ResponseAttribute::Header => response
                .header(filter_expr)
                .map(str::to_owned)
                .ok_or_else(|| format!("response of {request_id} has no {filter_expr} header")),
            ResponseAttribute::Body => {
                let body = response.body_text();
                if filter_expr.trim().is_empty() {
                    Ok(body)
                } else {
                    filter::select_scalar(&body, filter_expr)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::Timings;
    use crate::template::Context;

    fn response(status: u16, body: &str) -> Response {
        Response {
            request_id: None,
            method: "GET".into(),
            url: "http://x/login".into(),
            status,
            reason: String::new(),
            http_version: "HTTP/1.1".into(),
            headers: vec![("X-Token".into(), "hdr".into())],
            body: body.as_bytes().to_vec(),
            truncated: false,
            timings: Timings::default(),
            timeline: Vec::new(),
            redirects: 0,
            started_at: 0,
        }
    }

    #[test]
    fn keeps_bounded_history_newest_last() {
        let mut h = History::with_limit(2);
        for s in [200, 201, 202] {
            h.push(Id::from("r"), response(s, "{}"));
        }
        let statuses: Vec<_> = h.for_request(&Id::from("r")).map(|r| r.status).collect();
        assert_eq!(statuses, [202, 201]);
    }

    #[test]
    fn chains_values_into_templates() {
        let mut h = History::default();
        h.push(Id::from("login"), response(200, r#"{"token":"abc"}"#));
        let ctx = Context::new(Default::default()).with_responses(&h);
        assert_eq!(
            ctx.render("{% response 'body', 'login', '$.token' %}")
                .unwrap(),
            "abc"
        );
        assert_eq!(
            ctx.render("{% response 'header', 'login', 'x-token' %}")
                .unwrap(),
            "hdr"
        );
        assert_eq!(
            ctx.render("{% response 'status', 'login' %}").unwrap(),
            "200"
        );
        let err = ctx
            .render("{% response 'body', 'other', '$' %}")
            .unwrap_err();
        assert!(err.message.contains("send it first"));
    }
}
