//! Background OAuth 2.0 token fetching for the Auth tab's "Fetch Token".

use nexus_http_core::Client;
use nexus_http_core::auth::{self, Pkce};
use nexus_http_core::model::{OAuth2Config, OAuth2Grant, OAuth2Token, WorkspaceSettings};
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

/// How long to wait for the browser sign-in.
const BROWSER_TIMEOUT: Duration = Duration::from_secs(180);

pub struct TokenJob {
    rx: Receiver<Result<OAuth2Token, String>>,
}

impl TokenJob {
    /// Starts fetching a token for `config` (already template-rendered).
    pub fn start(config: OAuth2Config, settings: WorkspaceSettings) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(fetch(&config, settings));
        });
        Self { rx }
    }

    pub fn poll(&self) -> Option<Result<OAuth2Token, String>> {
        self.rx.try_recv().ok()
    }
}

fn fetch(config: &OAuth2Config, settings: WorkspaceSettings) -> Result<OAuth2Token, String> {
    let client = Client::new(settings);
    let cancel = AtomicBool::new(false);
    let request = match config.grant {
        OAuth2Grant::ClientCredentials | OAuth2Grant::Password => {
            auth::token_request(config, None, None)?
        }
        OAuth2Grant::AuthorizationCode | OAuth2Grant::Implicit => {
            let redirect = url::Url::parse(&config.redirect_url)
                .map_err(|e| format!("invalid redirect URL: {e}"))?;
            let host = redirect.host_str().unwrap_or("");
            if !matches!(host, "127.0.0.1" | "localhost" | "[::1]") {
                return Err("the redirect URL must point to 127.0.0.1 or localhost so nexus-http can receive the callback".into());
            }
            let port = redirect.port_or_known_default().unwrap_or(80);
            let listener = TcpListener::bind(("127.0.0.1", port))
                .map_err(|e| format!("cannot listen on port {port} for the redirect: {e}"))?;
            let state = uuid_like();
            let pkce =
                (config.grant == OAuth2Grant::AuthorizationCode && config.use_pkce).then(Pkce::new);
            let url = auth::authorization_url(config, &state, pkce.as_ref())?;
            open_browser(url.as_str())?;
            let redirected = auth::wait_for_redirect(&listener, BROWSER_TIMEOUT)?;
            if config.grant == OAuth2Grant::Implicit {
                // Browsers do not send the fragment; providers that return the
                // token in the query (some do) still work.
                return auth::token_from_implicit_redirect(&redirected, &state, now())
                    .map_err(|e| format!("{e} (the implicit grant returns the token in the URL fragment, which browsers do not send to the loopback listener; prefer Authorization Code with PKCE)"));
            }
            let code = auth::code_from_redirect(&redirected, &state)?;
            auth::token_request(
                config,
                Some(&code),
                pkce.as_ref().map(|p| p.verifier.as_str()),
            )?
        }
    };
    client
        .fetch_oauth_token(&request, &cancel)
        .map_err(|e| e.to_string())
}

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn uuid_like() -> String {
    let pkce = Pkce::new();
    pkce.verifier[..24].to_owned()
}

/// Opens `url` in the system browser.
pub fn open_browser(url: &str) -> Result<(), String> {
    let result = if cfg!(target_os = "macos") {
        std::process::Command::new("open").arg(url).spawn()
    } else if cfg!(target_os = "windows") {
        std::process::Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
    } else {
        std::process::Command::new("xdg-open").arg(url).spawn()
    };
    result
        .map(|_| ())
        .map_err(|e| format!("cannot open the browser ({e}); open this URL manually: {url}"))
}
