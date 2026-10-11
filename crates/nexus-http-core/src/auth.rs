//! Authentication schemes. Static schemes are applied while preparing a request;
//! Digest answers a `401` challenge; AWS SigV4 signs the final request bytes;
//! OAuth 2.0 helpers build authorization URLs, run token requests and capture the
//! browser redirect on a loopback port.

use crate::model::{OAuth2Config, OAuth2Grant, OAuth2Token};
use crate::template::hex;
use base64::Engine as _;
use hmac::{Hmac, Mac};
use sha2::{Digest as _, Sha256};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::time::{Duration, Instant};
use url::Url;

type HmacSha256 = Hmac<Sha256>;

/// `Authorization` value for HTTP Basic.
pub fn basic_header(username: &str, password: &str) -> String {
    let raw = format!("{username}:{password}");
    format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(raw)
    )
}

// ---------------------------------------------------------------------------
// Digest

/// A parsed `WWW-Authenticate: Digest ...` challenge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DigestChallenge {
    pub realm: String,
    pub nonce: String,
    pub opaque: Option<String>,
    pub algorithm: String,
    /// Chosen quality of protection (`auth`), if the server offered any.
    pub qop: Option<String>,
}

impl DigestChallenge {
    /// Parses a header value; returns `None` when it is not a Digest challenge.
    pub fn parse(header: &str) -> Option<Self> {
        let rest = header.trim();
        if rest.len() < 7 || !rest[..7].eq_ignore_ascii_case("digest ") {
            return None;
        }
        let params = parse_auth_params(&rest[7..]);
        let qop = params.get("qop").and_then(|q| {
            q.split(',')
                .map(str::trim)
                .find(|q| q.eq_ignore_ascii_case("auth"))
                .map(str::to_owned)
        });
        Some(Self {
            realm: params.get("realm").cloned().unwrap_or_default(),
            nonce: params.get("nonce")?.clone(),
            opaque: params.get("opaque").cloned(),
            algorithm: params
                .get("algorithm")
                .cloned()
                .unwrap_or_else(|| "MD5".into()),
            qop,
        })
    }

    /// The `Authorization` header answering this challenge.
    pub fn respond(
        &self,
        method: &str,
        uri: &str,
        username: &str,
        password: &str,
        cnonce: &str,
        nc: u32,
    ) -> Result<String, String> {
        let algorithm = self.algorithm.to_ascii_uppercase();
        let hash: fn(&str) -> String = match algorithm.trim_end_matches("-SESS") {
            "MD5" => |s| hex(&md5::Md5::digest(s.as_bytes())),
            "SHA-256" => |s| hex(&Sha256::digest(s.as_bytes())),
            other => return Err(format!("unsupported digest algorithm {other}")),
        };
        let mut ha1 = hash(&format!("{username}:{}:{password}", self.realm));
        if algorithm.ends_with("-SESS") {
            ha1 = hash(&format!("{ha1}:{}:{cnonce}", self.nonce));
        }
        let ha2 = hash(&format!("{method}:{uri}"));
        let nc = format!("{nc:08x}");
        let response = match &self.qop {
            Some(qop) => hash(&format!("{ha1}:{}:{nc}:{cnonce}:{qop}:{ha2}", self.nonce)),
            None => hash(&format!("{ha1}:{}:{ha2}", self.nonce)),
        };
        let mut header = format!(
            "Digest username=\"{}\", realm=\"{}\", nonce=\"{}\", uri=\"{uri}\", algorithm={}, response=\"{response}\"",
            quote(username),
            quote(&self.realm),
            quote(&self.nonce),
            self.algorithm,
        );
        if let Some(qop) = &self.qop {
            header.push_str(&format!(", qop={qop}, nc={nc}, cnonce=\"{cnonce}\""));
        }
        if let Some(opaque) = &self.opaque {
            header.push_str(&format!(", opaque=\"{}\"", quote(opaque)));
        }
        Ok(header)
    }
}

fn quote(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn parse_auth_params(input: &str) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    let mut chars = input.chars().peekable();
    loop {
        while chars.peek().is_some_and(|c| c.is_whitespace() || *c == ',') {
            chars.next();
        }
        let key: String = chars.by_ref().take_while(|c| *c != '=').collect();
        if key.is_empty() {
            break;
        }
        let mut value = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => value.extend(chars.next()),
                    '"' => break,
                    c => value.push(c),
                }
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == ',' {
                    break;
                }
                value.push(c);
                chars.next();
            }
        }
        params.insert(key.trim().to_ascii_lowercase(), value.trim().to_owned());
    }
    params
}

// ---------------------------------------------------------------------------
// AWS Signature Version 4

pub struct SigV4<'a> {
    pub access_key_id: &'a str,
    pub secret_access_key: &'a str,
    pub session_token: &'a str,
    pub region: &'a str,
    pub service: &'a str,
}

impl SigV4<'_> {
    /// Signs a request in place: adds `x-amz-date`, optional
    /// `x-amz-security-token`/`x-amz-content-sha256`, and `Authorization`.
    /// `amz_date` is `YYYYMMDD'T'HHMMSS'Z'`.
    pub fn sign(
        &self,
        method: &str,
        url: &Url,
        headers: &mut Vec<(String, String)>,
        body: &[u8],
        amz_date: &str,
    ) -> Result<(), String> {
        if amz_date.len() != 16 {
            return Err(format!("invalid AWS date {amz_date}"));
        }
        let host = match (url.host_str(), url.port()) {
            (Some(h), Some(p)) => format!("{h}:{p}"),
            (Some(h), None) => h.to_owned(),
            _ => return Err("AWS SigV4 needs a URL with a host".into()),
        };
        let payload_hash = hex(&Sha256::digest(body));
        headers.retain(|(n, _)| {
            !n.eq_ignore_ascii_case("authorization") && !n.eq_ignore_ascii_case("x-amz-date")
        });
        headers.push(("x-amz-date".into(), amz_date.into()));
        if !self.session_token.is_empty() {
            headers.push(("x-amz-security-token".into(), self.session_token.into()));
        }
        if self.service == "s3"
            && !headers
                .iter()
                .any(|(n, _)| n.eq_ignore_ascii_case("x-amz-content-sha256"))
        {
            headers.push(("x-amz-content-sha256".into(), payload_hash.clone()));
        }

        let mut signed: BTreeMap<String, String> = BTreeMap::new();
        signed.insert("host".into(), host);
        for (name, value) in headers.iter() {
            let lower = name.to_ascii_lowercase();
            if lower.starts_with("x-amz-") || lower == "content-type" {
                let value = value.split_whitespace().collect::<Vec<_>>().join(" ");
                signed
                    .entry(lower)
                    .and_modify(|v| {
                        v.push(',');
                        v.push_str(&value);
                    })
                    .or_insert(value);
            }
        }
        let canonical_headers: String = signed.iter().map(|(k, v)| format!("{k}:{v}\n")).collect();
        let signed_headers = signed.keys().cloned().collect::<Vec<_>>().join(";");

        let path = if url.path().is_empty() {
            "/"
        } else {
            url.path()
        };
        let canonical_uri = if self.service == "s3" {
            path.to_owned()
        } else {
            path.split('/')
                .map(aws_encode)
                .collect::<Vec<_>>()
                .join("/")
        };
        let mut query: Vec<(String, String)> = url
            .query_pairs()
            .map(|(k, v)| (aws_encode(&k), aws_encode(&v)))
            .collect();
        query.sort();
        let canonical_query = query
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&");

        let canonical = format!(
            "{method}\n{canonical_uri}\n{canonical_query}\n{canonical_headers}\n{signed_headers}\n{payload_hash}"
        );
        let date = &amz_date[..8];
        let scope = format!("{date}/{}/{}/aws4_request", self.region, self.service);
        let string_to_sign = format!(
            "AWS4-HMAC-SHA256\n{amz_date}\n{scope}\n{}",
            hex(&Sha256::digest(canonical.as_bytes()))
        );
        let k_date = hmac(format!("AWS4{}", self.secret_access_key).as_bytes(), date);
        let k_region = hmac(&k_date, self.region);
        let k_service = hmac(&k_region, self.service);
        let k_signing = hmac(&k_service, "aws4_request");
        let signature = hex(&hmac(&k_signing, &string_to_sign));
        headers.push((
            "Authorization".into(),
            format!(
                "AWS4-HMAC-SHA256 Credential={}/{scope}, SignedHeaders={signed_headers}, Signature={signature}",
                self.access_key_id
            ),
        ));
        Ok(())
    }
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    // HMAC accepts keys of any length, so construction cannot fail.
    let mut mac = HmacSha256::new_from_slice(key).unwrap_or_else(|_| unreachable!());
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

/// RFC 3986 encoding of everything but unreserved characters.
fn aws_encode(s: &str) -> String {
    let decoded = percent_decode(s);
    let mut out = String::new();
    for b in decoded.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

/// `YYYYMMDDTHHMMSSZ` for the current time.
pub fn amz_date_now() -> String {
    let iso = crate::template::iso8601(crate::cookies::unix_now(), 0);
    format!(
        "{}{}{}T{}{}{}Z",
        &iso[0..4],
        &iso[5..7],
        &iso[8..10],
        &iso[11..13],
        &iso[14..16],
        &iso[17..19]
    )
}

// ---------------------------------------------------------------------------
// OAuth 2.0

/// A PKCE verifier and its S256 challenge (RFC 7636).
#[derive(Debug, Clone)]
pub struct Pkce {
    pub verifier: String,
    pub challenge: String,
}

impl Pkce {
    pub fn new() -> Self {
        use rand::Rng as _;
        let bytes: [u8; 32] = rand::thread_rng().r#gen();
        Self::from_verifier(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
    }

    pub fn from_verifier(verifier: String) -> Self {
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .encode(Sha256::digest(verifier.as_bytes()));
        Self {
            verifier,
            challenge,
        }
    }
}

impl Default for Pkce {
    fn default() -> Self {
        Self::new()
    }
}

/// The browser URL for the authorization-code and implicit grants.
pub fn authorization_url(
    config: &OAuth2Config,
    state: &str,
    pkce: Option<&Pkce>,
) -> Result<Url, String> {
    let mut url = Url::parse(&config.authorization_url)
        .map_err(|e| format!("invalid authorization URL: {e}"))?;
    {
        let mut q = url.query_pairs_mut();
        let response_type = match config.grant {
            OAuth2Grant::Implicit => "token",
            _ => "code",
        };
        q.append_pair("response_type", response_type);
        q.append_pair("client_id", &config.client_id);
        if !config.redirect_url.is_empty() {
            q.append_pair("redirect_uri", &config.redirect_url);
        }
        if !config.scope.is_empty() {
            q.append_pair("scope", &config.scope);
        }
        if !config.audience.is_empty() {
            q.append_pair("audience", &config.audience);
        }
        q.append_pair("state", state);
        if let Some(pkce) = pkce {
            q.append_pair("code_challenge", &pkce.challenge);
            q.append_pair("code_challenge_method", "S256");
        }
    }
    Ok(url)
}

/// Form fields and optional Basic header of a token request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRequest {
    pub url: String,
    pub form: Vec<(String, String)>,
    pub basic_auth: Option<String>,
}

/// Builds the token endpoint request for a grant. `code`/`verifier` are needed
/// for the authorization-code grant.
pub fn token_request(
    config: &OAuth2Config,
    code: Option<&str>,
    verifier: Option<&str>,
) -> Result<TokenRequest, String> {
    if config.access_token_url.is_empty() {
        return Err("set the access token URL".into());
    }
    let mut form: Vec<(String, String)> = Vec::new();
    let mut push = |k: &str, v: &str| form.push((k.to_owned(), v.to_owned()));
    match config.grant {
        OAuth2Grant::AuthorizationCode => {
            push("grant_type", "authorization_code");
            push(
                "code",
                code.ok_or("authorization code missing; complete the browser sign-in first")?,
            );
            if !config.redirect_url.is_empty() {
                push("redirect_uri", &config.redirect_url);
            }
            if let Some(v) = verifier {
                push("code_verifier", v);
            }
        }
        OAuth2Grant::ClientCredentials => push("grant_type", "client_credentials"),
        OAuth2Grant::Password => {
            push("grant_type", "password");
            push("username", &config.username);
            push("password", &config.password);
        }
        OAuth2Grant::Implicit => {
            return Err("the implicit grant receives its token from the redirect".into());
        }
    }
    if !config.scope.is_empty() && config.grant != OAuth2Grant::AuthorizationCode {
        push("scope", &config.scope);
    }
    if !config.audience.is_empty() {
        push("audience", &config.audience);
    }
    let basic_auth = if config.credentials_in_header {
        Some(basic_header(&config.client_id, &config.client_secret))
    } else {
        push("client_id", &config.client_id);
        if !config.client_secret.is_empty() {
            push("client_secret", &config.client_secret);
        }
        None
    };
    Ok(TokenRequest {
        url: config.access_token_url.clone(),
        form,
        basic_auth,
    })
}

/// A refresh-token request.
pub fn refresh_request(config: &OAuth2Config, refresh_token: &str) -> TokenRequest {
    let mut form = vec![
        ("grant_type".to_owned(), "refresh_token".to_owned()),
        ("refresh_token".to_owned(), refresh_token.to_owned()),
    ];
    let basic_auth = if config.credentials_in_header {
        Some(basic_header(&config.client_id, &config.client_secret))
    } else {
        form.push(("client_id".into(), config.client_id.clone()));
        if !config.client_secret.is_empty() {
            form.push(("client_secret".into(), config.client_secret.clone()));
        }
        None
    };
    TokenRequest {
        url: config.access_token_url.clone(),
        form,
        basic_auth,
    }
}

/// Parses a token endpoint JSON response (or an `error` response).
pub fn parse_token_response(body: &[u8], now: u64) -> Result<OAuth2Token, String> {
    let value: serde_json::Value = serde_json::from_slice(body).or_else(|_| {
        // Some providers (GitHub) answer form-encoded.
        let map: serde_json::Map<String, serde_json::Value> = url::form_urlencoded::parse(body)
            .map(|(k, v)| (k.into_owned(), serde_json::Value::String(v.into_owned())))
            .collect();
        if map.is_empty() {
            Err("token response is neither JSON nor form-encoded".to_owned())
        } else {
            Ok(serde_json::Value::Object(map))
        }
    })?;
    if let Some(error) = value.get("error").and_then(|e| e.as_str()) {
        let description = value
            .get("error_description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        return Err(format!("token endpoint returned {error} {description}")
            .trim()
            .to_owned());
    }
    token_from_map(&value, now)
}

fn token_from_map(value: &serde_json::Value, now: u64) -> Result<OAuth2Token, String> {
    let get = |k: &str| value.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    let access_token = get("access_token").ok_or("token response has no access_token")?;
    let expires_in = value.get("expires_in").and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
    });
    Ok(OAuth2Token {
        access_token,
        refresh_token: get("refresh_token"),
        token_type: get("token_type"),
        expires_at: expires_in.map(|s| now + s),
    })
}

/// Extracts the `code` from an authorization-code redirect, checking `state`.
pub fn code_from_redirect(redirect: &Url, expected_state: &str) -> Result<String, String> {
    let params: BTreeMap<String, String> = redirect.query_pairs().into_owned().collect();
    if let Some(error) = params.get("error") {
        return Err(format!(
            "authorization failed: {error} {}",
            params
                .get("error_description")
                .map(String::as_str)
                .unwrap_or("")
        )
        .trim()
        .to_owned());
    }
    if params.get("state").map(String::as_str) != Some(expected_state) {
        return Err("state mismatch in redirect; possible CSRF, try again".into());
    }
    params
        .get("code")
        .cloned()
        .ok_or_else(|| "redirect has no code parameter".into())
}

/// Extracts the token from an implicit-grant redirect fragment.
pub fn token_from_implicit_redirect(
    redirect: &Url,
    expected_state: &str,
    now: u64,
) -> Result<OAuth2Token, String> {
    let fragment = redirect.fragment().unwrap_or("");
    let map: serde_json::Map<String, serde_json::Value> =
        url::form_urlencoded::parse(fragment.as_bytes())
            .map(|(k, v)| (k.into_owned(), serde_json::Value::String(v.into_owned())))
            .collect();
    if map.get("state").and_then(|s| s.as_str()) != Some(expected_state) {
        return Err("state mismatch in redirect; possible CSRF, try again".into());
    }
    token_from_map(&serde_json::Value::Object(map), now)
}

/// Waits for the OAuth browser redirect on a loopback listener and answers the
/// browser with a short page. Returns the full redirect URL.
pub fn wait_for_redirect(listener: &TcpListener, timeout: Duration) -> Result<Url, String> {
    listener
        .set_nonblocking(true)
        .map_err(|e| format!("cannot poll redirect listener: {e}"))?;
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    let deadline = Instant::now() + timeout;
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                stream
                    .set_nonblocking(false)
                    .map_err(|e| format!("redirect connection error: {e}"))?;
                let mut reader = BufReader::new(&stream);
                let mut line = String::new();
                reader
                    .read_line(&mut line)
                    .map_err(|e| format!("cannot read redirect: {e}"))?;
                let target = line.split_whitespace().nth(1).unwrap_or("/");
                let page = "<!doctype html><title>Signed in</title><body style=\"font-family:sans-serif;padding:3em\"><h2>Authorization received</h2><p>You can close this window and return to nexus-http.</p></body>";
                let mut writer = &stream;
                let _ = write!(
                    writer,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{page}",
                    page.len()
                );
                return Url::parse(&format!("http://127.0.0.1:{port}{target}"))
                    .map_err(|e| format!("invalid redirect target: {e}"));
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err("timed out waiting for the browser redirect".into());
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => return Err(format!("redirect listener failed: {e}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn basic_header_encodes_credentials() {
        let header = basic_header("user", "pa:ss");
        let encoded = header.strip_prefix("Basic ").unwrap();
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .unwrap();
        assert_eq!(decoded, b"user:pa:ss");
    }

    #[test]
    fn digest_matches_rfc2617_example() {
        let challenge = DigestChallenge::parse(
            r#"Digest realm="testrealm@host.com", qop="auth,auth-int", nonce="dcd98b7102dd2f0e8b11d0f600bfb0c093", opaque="5ccc069c403ebaf9f0171e9517f40e41""#,
        )
        .unwrap();
        assert_eq!(challenge.qop.as_deref(), Some("auth"));
        let header = challenge
            .respond(
                "GET",
                "/dir/index.html",
                "Mufasa",
                "Circle Of Life",
                "0a4f113b",
                1,
            )
            .unwrap();
        assert!(
            header.contains("response=\"6629fae49393a05397450978507c4ef1\""),
            "{header}"
        );
        assert!(header.contains("nc=00000001"));
        assert!(header.contains("opaque=\"5ccc069c403ebaf9f0171e9517f40e41\""));
        assert!(DigestChallenge::parse("Basic realm=x").is_none());
    }

    #[test]
    fn sigv4_matches_aws_get_vanilla_vector() {
        let signer = SigV4 {
            access_key_id: "AKIDEXAMPLE",
            secret_access_key: "wJalrXUtnFEMI/K7MDENG+bPxRfiCYEXAMPLEKEY",
            session_token: "",
            region: "us-east-1",
            service: "service",
        };
        let mut headers = Vec::new();
        let url = Url::parse("https://example.amazonaws.com/").unwrap();
        signer
            .sign("GET", &url, &mut headers, b"", "20150830T123600Z")
            .unwrap();
        let auth = &headers
            .iter()
            .find(|(n, _)| n == "Authorization")
            .unwrap()
            .1;
        assert_eq!(
            auth,
            "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20150830/us-east-1/service/aws4_request, SignedHeaders=host;x-amz-date, Signature=5fa00fa31553b73ebf1942676e86291e8372ff2a2260956d9b8aae1d763fbf31"
        );
    }

    #[test]
    fn amz_date_has_expected_shape() {
        let d = amz_date_now();
        assert_eq!(d.len(), 16);
        assert_eq!(&d[8..9], "T");
    }

    #[test]
    fn pkce_matches_rfc7636_example() {
        let pkce = Pkce::from_verifier("dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".into());
        assert_eq!(
            pkce.challenge,
            "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM"
        );
        assert_eq!(Pkce::new().verifier.len(), 43);
    }

    fn oauth(grant: OAuth2Grant) -> OAuth2Config {
        OAuth2Config {
            grant,
            authorization_url: "https://auth.test/authorize".into(),
            access_token_url: "https://auth.test/token".into(),
            client_id: "cid".into(),
            client_secret: "sec".into(),
            redirect_url: "http://127.0.0.1:9000/cb".into(),
            scope: "read write".into(),
            ..Default::default()
        }
    }

    #[test]
    fn builds_authorization_url_with_pkce() {
        let pkce = Pkce::from_verifier("v".repeat(43));
        let url =
            authorization_url(&oauth(OAuth2Grant::AuthorizationCode), "st", Some(&pkce)).unwrap();
        let q: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
        assert_eq!(q["response_type"], "code");
        assert_eq!(q["scope"], "read write");
        assert_eq!(q["code_challenge_method"], "S256");
        let implicit = authorization_url(&oauth(OAuth2Grant::Implicit), "st", None).unwrap();
        assert!(implicit.as_str().contains("response_type=token"));
    }

    #[test]
    fn token_requests_per_grant() {
        let cc = token_request(&oauth(OAuth2Grant::ClientCredentials), None, None).unwrap();
        assert!(
            cc.form
                .contains(&("grant_type".into(), "client_credentials".into()))
        );
        assert!(cc.form.contains(&("client_secret".into(), "sec".into())));
        let mut cfg = oauth(OAuth2Grant::AuthorizationCode);
        cfg.credentials_in_header = true;
        let code = token_request(&cfg, Some("abc"), Some("ver")).unwrap();
        assert!(code.form.contains(&("code_verifier".into(), "ver".into())));
        assert!(code.basic_auth.is_some());
        assert!(token_request(&cfg, None, None).is_err());
        assert!(token_request(&oauth(OAuth2Grant::Implicit), None, None).is_err());
    }

    #[test]
    fn parses_token_and_redirects() {
        let token = parse_token_response(
            br#"{"access_token":"t","expires_in":60,"refresh_token":"r"}"#,
            100,
        )
        .unwrap();
        assert_eq!(token.expires_at, Some(160));
        assert_eq!(token.refresh_token.as_deref(), Some("r"));
        let form = parse_token_response(b"access_token=gh&token_type=bearer", 0).unwrap();
        assert_eq!(form.access_token, "gh");
        let err = parse_token_response(br#"{"error":"invalid_client"}"#, 0).unwrap_err();
        assert!(err.contains("invalid_client"));
        let redirect = Url::parse("http://127.0.0.1/cb?code=c1&state=s").unwrap();
        assert_eq!(code_from_redirect(&redirect, "s").unwrap(), "c1");
        assert!(code_from_redirect(&redirect, "other").is_err());
        let implicit =
            Url::parse("http://127.0.0.1/cb#access_token=a&state=s&expires_in=5").unwrap();
        assert_eq!(
            token_from_implicit_redirect(&implicit, "s", 1)
                .unwrap()
                .expires_at,
            Some(6)
        );
    }

    #[test]
    fn captures_loopback_redirect() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let client = std::thread::spawn(move || {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(b"GET /cb?code=z&state=q HTTP/1.1\r\nHost: x\r\n\r\n")
                .unwrap();
            let mut out = String::new();
            std::io::Read::read_to_string(&mut s, &mut out).unwrap();
            out
        });
        let url = wait_for_redirect(&listener, Duration::from_secs(5)).unwrap();
        assert_eq!(code_from_redirect(&url, "q").unwrap(), "z");
        assert!(client.join().unwrap().contains("Authorization received"));
    }
}
