//! Background OAuth 2.0 token fetching for the Auth tab's "Fetch Token". The flow itself is `nexus_http_core::auth::fetch_token`; this runs it off the UI thread
//! and opens the system browser for it.

use nexus_http_core::auth;
use nexus_http_core::model::{OAuth2Config, OAuth2Token, WorkspaceSettings};
use std::sync::mpsc::{Receiver, channel};

pub struct TokenJob {
    rx: Receiver<Result<OAuth2Token, String>>,
}

impl TokenJob {
    /// Starts fetching a token for `config` (already template-rendered).
    pub fn start(config: OAuth2Config, settings: WorkspaceSettings) -> Self {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(auth::fetch_token(&config, settings, &open_browser));
        });
        Self { rx }
    }

    pub fn poll(&self) -> Option<Result<OAuth2Token, String>> {
        self.rx.try_recv().ok()
    }
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
