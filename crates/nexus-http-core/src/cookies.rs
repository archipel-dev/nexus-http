//! Per-workspace cookie jar following the RFC 6265 storage and matching rules
//! that matter in practice: host-only vs. domain cookies, path prefixes, secure,
//! expiry via `Max-Age`/`Expires`, and replacement by (name, domain, path).

use serde::{Deserialize, Serialize};
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCookie {
    pub name: String,
    pub value: String,
    /// Lower-case domain without a leading dot.
    pub domain: String,
    /// Only sent to exactly `domain` (no `Domain` attribute was given).
    #[serde(default)]
    pub host_only: bool,
    pub path: String,
    #[serde(default)]
    pub secure: bool,
    #[serde(default)]
    pub http_only: bool,
    /// Unix seconds; `None` for session cookies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CookieJar {
    pub cookies: Vec<StoredCookie>,
}

impl CookieJar {
    /// Stores a `Set-Cookie` header received from `url`. Invalid or foreign-domain
    /// cookies are ignored, as browsers do.
    pub fn store(&mut self, url: &Url, set_cookie: &str) {
        self.store_at(url, set_cookie, unix_now());
    }

    fn store_at(&mut self, url: &Url, set_cookie: &str, now: u64) {
        let Ok(parsed) = cookie::Cookie::parse(set_cookie.to_owned()) else {
            return;
        };
        let Some(host) = url.host_str().map(str::to_ascii_lowercase) else {
            return;
        };
        let (domain, host_only) = match parsed.domain() {
            Some(d) => {
                let d = d.trim_start_matches('.').to_ascii_lowercase();
                if !domain_matches(&host, &d) {
                    return;
                }
                (d, false)
            }
            None => (host, true),
        };
        let path = match parsed.path() {
            Some(p) if p.starts_with('/') => p.to_owned(),
            _ => default_path(url),
        };
        let expires = if let Some(max_age) = parsed.max_age() {
            Some(now.saturating_add_signed(max_age.whole_seconds()))
        } else {
            parsed
                .expires_datetime()
                .map(|t| u64::try_from(t.unix_timestamp()).unwrap_or(0))
        };
        self.cookies
            .retain(|c| !(c.name == parsed.name() && c.domain == domain && c.path == path));
        if expires.is_some_and(|e| e <= now) {
            return;
        }
        self.cookies.push(StoredCookie {
            name: parsed.name().to_owned(),
            value: parsed.value().to_owned(),
            domain,
            host_only,
            path,
            secure: parsed.secure().unwrap_or(false),
            http_only: parsed.http_only().unwrap_or(false),
            expires,
        });
    }

    /// The `Cookie` header value for a request to `url`, if any cookie matches.
    pub fn header_for(&self, url: &Url) -> Option<String> {
        self.header_at(url, unix_now())
    }

    fn header_at(&self, url: &Url, now: u64) -> Option<String> {
        let host = url.host_str()?.to_ascii_lowercase();
        let path = url.path();
        let secure = url.scheme() == "https" || url.scheme() == "wss";
        let mut matching: Vec<&StoredCookie> = self
            .cookies
            .iter()
            .filter(|c| c.expires.is_none_or(|e| e > now))
            .filter(|c| {
                if c.host_only {
                    c.domain == host
                } else {
                    domain_matches(&host, &c.domain)
                }
            })
            .filter(|c| path_matches(path, &c.path))
            .filter(|c| !c.secure || secure)
            .collect();
        // Longer paths first, as RFC 6265 recommends.
        matching.sort_by_key(|c| std::cmp::Reverse(c.path.len()));
        (!matching.is_empty()).then(|| {
            matching
                .iter()
                .map(|c| format!("{}={}", c.name, c.value))
                .collect::<Vec<_>>()
                .join("; ")
        })
    }

    pub fn remove_expired(&mut self) {
        let now = unix_now();
        self.cookies.retain(|c| c.expires.is_none_or(|e| e > now));
    }

    pub fn clear(&mut self) {
        self.cookies.clear();
    }
}

fn domain_matches(host: &str, domain: &str) -> bool {
    host == domain
        || (host.ends_with(domain)
            && host.as_bytes().get(host.len() - domain.len() - 1) == Some(&b'.')
            && host.parse::<std::net::IpAddr>().is_err())
}

fn path_matches(request: &str, cookie: &str) -> bool {
    request == cookie
        || (request.starts_with(cookie)
            && (cookie.ends_with('/') || request.as_bytes().get(cookie.len()) == Some(&b'/')))
}

fn default_path(url: &Url) -> String {
    let path = url.path();
    match path.rfind('/') {
        Some(0) | None => "/".into(),
        Some(i) => path[..i].to_owned(),
    }
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn host_only_and_domain_cookies() {
        let mut jar = CookieJar::default();
        jar.store_at(
            &url("https://api.example.com/v1/login"),
            "sid=1; Path=/",
            100,
        );
        jar.store_at(
            &url("https://api.example.com/"),
            "pref=dark; Domain=example.com",
            100,
        );
        jar.store_at(
            &url("https://api.example.com/"),
            "evil=1; Domain=other.com",
            100,
        );
        assert_eq!(
            jar.header_at(&url("https://api.example.com/x"), 100)
                .as_deref(),
            Some("sid=1; pref=dark")
        );
        assert_eq!(
            jar.header_at(&url("https://www.example.com/"), 100)
                .as_deref(),
            Some("pref=dark")
        );
        assert_eq!(jar.header_at(&url("https://notexample.com/"), 100), None);
    }

    #[test]
    fn path_secure_and_expiry() {
        let mut jar = CookieJar::default();
        let u = url("https://h.test/app/page");
        jar.store_at(&u, "a=1", 100); // default path /app
        jar.store_at(&u, "s=1; Secure; Path=/", 100);
        jar.store_at(&u, "t=1; Max-Age=10; Path=/", 100);
        assert_eq!(
            jar.header_at(&url("https://h.test/app/x"), 105).as_deref(),
            Some("a=1; s=1; t=1")
        );
        assert_eq!(
            jar.header_at(&url("http://h.test/apples"), 105).as_deref(),
            Some("t=1")
        );
        assert_eq!(jar.header_at(&url("http://h.test/"), 200), None);
        // Max-Age=0 deletes.
        jar.store_at(&u, "a=1; Max-Age=0", 300);
        assert!(!jar.cookies.iter().any(|c| c.name == "a"));
    }

    #[test]
    fn replaces_same_cookie() {
        let mut jar = CookieJar::default();
        let u = url("http://h.test/");
        jar.store_at(&u, "a=1", 0);
        jar.store_at(&u, "a=2", 0);
        assert_eq!(jar.cookies.len(), 1);
        assert_eq!(jar.header_at(&u, 0).as_deref(), Some("a=2"));
    }
}
