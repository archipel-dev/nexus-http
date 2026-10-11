//! Response assertions ("tests"): declarative checks on status, headers,
//! JSONPath values, body text and timing, evaluated after each response and by
//! the collection runner.

use crate::response::Response;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "arg", rename_all = "snake_case")]
pub enum Source {
    Status,
    Header(String),
    JsonPath(String),
    Body,
    ResponseTimeMs,
    BodySize,
}

impl Source {
    pub fn label(&self) -> String {
        match self {
            Source::Status => "status".into(),
            Source::Header(h) => format!("header {h}"),
            Source::JsonPath(p) => p.clone(),
            Source::Body => "body".into(),
            Source::ResponseTimeMs => "response time (ms)".into(),
            Source::BodySize => "body size (bytes)".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Operator {
    Equals,
    NotEquals,
    Contains,
    NotContains,
    Exists,
    NotExists,
    LessThan,
    GreaterThan,
}

impl Operator {
    pub const ALL: [Operator; 8] = [
        Operator::Equals,
        Operator::NotEquals,
        Operator::Contains,
        Operator::NotContains,
        Operator::Exists,
        Operator::NotExists,
        Operator::LessThan,
        Operator::GreaterThan,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Operator::Equals => "equals",
            Operator::NotEquals => "does not equal",
            Operator::Contains => "contains",
            Operator::NotContains => "does not contain",
            Operator::Exists => "exists",
            Operator::NotExists => "does not exist",
            Operator::LessThan => "is less than",
            Operator::GreaterThan => "is greater than",
        }
    }

    pub fn needs_value(self) -> bool {
        !matches!(self, Operator::Exists | Operator::NotExists)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assertion {
    pub source: Source,
    pub operator: Operator,
    #[serde(default)]
    pub expected: String,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

impl Assertion {
    pub fn new(source: Source, operator: Operator, expected: impl Into<String>) -> Self {
        Self {
            source,
            operator,
            expected: expected.into(),
            enabled: true,
        }
    }

    pub fn describe(&self) -> String {
        if self.operator.needs_value() {
            format!(
                "{} {} {}",
                self.source.label(),
                self.operator.label(),
                self.expected
            )
        } else {
            format!("{} {}", self.source.label(), self.operator.label())
        }
    }

    /// Checks the assertion against a response.
    pub fn check(&self, response: &Response) -> AssertionResult {
        let actual = self.actual(response);
        let passed = match (&actual, self.operator) {
            (Err(_), Operator::NotExists) => true,
            (Err(_), _) => false,
            (Ok(_), Operator::Exists) => true,
            (Ok(_), Operator::NotExists) => false,
            (Ok(a), Operator::Equals) => values_equal(a, &self.expected),
            (Ok(a), Operator::NotEquals) => !values_equal(a, &self.expected),
            (Ok(a), Operator::Contains) => a.contains(&self.expected),
            (Ok(a), Operator::NotContains) => !a.contains(&self.expected),
            (Ok(a), Operator::LessThan) => compare(a, &self.expected).is_some_and(|o| o.is_lt()),
            (Ok(a), Operator::GreaterThan) => compare(a, &self.expected).is_some_and(|o| o.is_gt()),
        };
        AssertionResult {
            description: self.describe(),
            passed,
            actual: match actual {
                Ok(a) => a,
                Err(e) => e,
            },
        }
    }

    fn actual(&self, response: &Response) -> Result<String, String> {
        match &self.source {
            Source::Status => Ok(response.status.to_string()),
            Source::Header(name) => response
                .header(name)
                .map(str::to_owned)
                .ok_or_else(|| format!("no {name} header")),
            Source::JsonPath(path) => crate::filter::select_scalar(&response.body_text(), path),
            Source::Body => Ok(response.body_text()),
            Source::ResponseTimeMs => Ok(format!("{:.0}", response.timings.total_ms)),
            Source::BodySize => Ok(response.body.len().to_string()),
        }
    }
}

fn values_equal(actual: &str, expected: &str) -> bool {
    if actual == expected {
        return true;
    }
    match (actual.trim().parse::<f64>(), expected.trim().parse::<f64>()) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

fn compare(actual: &str, expected: &str) -> Option<std::cmp::Ordering> {
    let a = actual.trim().parse::<f64>().ok()?;
    let b = expected.trim().parse::<f64>().ok()?;
    a.partial_cmp(&b)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssertionResult {
    pub description: String,
    pub passed: bool,
    /// The observed value, or why it could not be read.
    pub actual: String,
}

/// Checks all enabled assertions.
pub fn check_all(assertions: &[Assertion], response: &Response) -> Vec<AssertionResult> {
    assertions
        .iter()
        .filter(|a| a.enabled)
        .map(|a| a.check(response))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::response::Timings;

    fn response() -> Response {
        Response {
            request_id: None,
            method: "GET".into(),
            url: "http://x/".into(),
            status: 201,
            reason: "Created".into(),
            http_version: "HTTP/1.1".into(),
            headers: vec![("Content-Type".into(), "application/json".into())],
            body: br#"{"id":42,"name":"Ada","tags":["a"]}"#.to_vec(),
            truncated: false,
            timings: Timings {
                total_ms: 120.0,
                ..Timings::default()
            },
            timeline: Vec::new(),
            redirects: 0,
            started_at: 0,
        }
    }

    #[test]
    fn checks_common_assertions() {
        let r = response();
        let cases = [
            (
                Assertion::new(Source::Status, Operator::Equals, "201"),
                true,
            ),
            (
                Assertion::new(Source::Status, Operator::LessThan, "300"),
                true,
            ),
            (
                Assertion::new(
                    Source::Header("content-type".into()),
                    Operator::Contains,
                    "json",
                ),
                true,
            ),
            (
                Assertion::new(Source::Header("x-missing".into()), Operator::NotExists, ""),
                true,
            ),
            (
                Assertion::new(Source::JsonPath("$.id".into()), Operator::Equals, "42.0"),
                true,
            ),
            (
                Assertion::new(
                    Source::JsonPath("$.name".into()),
                    Operator::NotEquals,
                    "Bob",
                ),
                true,
            ),
            (
                Assertion::new(Source::JsonPath("$.nope".into()), Operator::Exists, ""),
                false,
            ),
            (
                Assertion::new(Source::Body, Operator::Contains, "Ada"),
                true,
            ),
            (
                Assertion::new(Source::ResponseTimeMs, Operator::LessThan, "100"),
                false,
            ),
            (
                Assertion::new(Source::BodySize, Operator::GreaterThan, "10"),
                true,
            ),
        ];
        for (assertion, expected) in cases {
            let result = assertion.check(&r);
            assert_eq!(
                result.passed, expected,
                "{} (actual {})",
                result.description, result.actual
            );
        }
    }

    #[test]
    fn skips_disabled() {
        let mut a = Assertion::new(Source::Status, Operator::Equals, "500");
        a.enabled = false;
        assert!(check_all(&[a], &response()).is_empty());
    }
}
