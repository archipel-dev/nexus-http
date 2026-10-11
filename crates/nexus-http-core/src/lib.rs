//! Headless engine of the nexus-http API client plugin.
//!
//! This crate has no UI dependency. It owns the data model ([`model`]), git-friendly
//! storage ([`storage`]), templating with variables and template tags
//! ([`template`]), authentication ([`auth`]), request execution ([`exec`]), the
//! cookie jar ([`cookies`]), response history and chaining ([`history`]),
//! JSONPath filters ([`filter`]), response assertions ([`assertions`]), the collection
//! runner ([`runner`]), importers
//! ([`import`]) and code generation ([`codegen`]).

pub mod assertions;
pub mod auth;
pub mod codegen;
pub mod cookies;
pub mod exec;
pub mod filter;
pub mod graphql;
pub mod grpc;
pub mod history;
pub mod import;
pub mod model;
pub mod response;
pub mod runner;
pub mod storage;
pub mod stream;
pub mod template;

use std::path::Path;
use std::sync::atomic::AtomicBool;
use std::time::Instant;

pub use exec::{Client, ExecError, PreparedRequest};
pub use model::{Id, Workspace};
pub use response::Response;

/// Prepares and sends request `id`, using `history` for `{% response %}` chaining
/// and `jar` for cookies. Timings include template rendering.
pub fn execute(
    workspace: &Workspace,
    id: &Id,
    base_dir: &Path,
    client: &Client,
    jar: Option<&mut cookies::CookieJar>,
    history: &history::History,
    cancel: &AtomicBool,
) -> Result<Response, ExecError> {
    let started = Instant::now();
    let prepared = exec::prepare(
        workspace,
        id,
        &exec::PrepareOptions {
            base_dir,
            responses: history,
        },
    )?;
    let prepare_ms = started.elapsed().as_secs_f64() * 1000.0;
    let mut response = client.send(&prepared, jar, cancel)?;
    response.timings.prepare_ms = prepare_ms;
    response.timings.total_ms += prepare_ms;
    Ok(response)
}
