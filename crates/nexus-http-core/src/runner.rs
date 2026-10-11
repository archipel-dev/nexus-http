//! Collection runner: sends the requests of a workspace or folder in order,
//! feeding each response into the history so later requests can chain on
//! earlier ones, and checks every request's assertions.

use crate::assertions::{self, AssertionResult};
use crate::cookies::CookieJar;
use crate::history::History;
use crate::model::{Id, Item, Protocol, Workspace};
use crate::response::{Response, Timings};
use crate::stream::{Direction, StreamSession, connect_sse, connect_websocket};
use crate::{Client, exec, execute};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    /// Folder to run; `None` runs the whole workspace.
    pub scope: Option<Id>,
    pub iterations: u32,
    /// Pause between requests.
    pub delay: Duration,
    /// Stop at the first failed request or assertion.
    pub bail: bool,
    /// Streaming requests (SSE/WebSocket) are connected and read until this
    /// many messages arrived or `stream_timeout` elapsed, whichever is first.
    pub stream_messages: u32,
    pub stream_timeout: Duration,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            scope: None,
            iterations: 1,
            delay: Duration::ZERO,
            bail: false,
            stream_messages: 3,
            stream_timeout: Duration::from_secs(3),
        }
    }
}

/// Outcome of one request in a run.
#[derive(Debug, Clone, PartialEq)]
pub struct StepResult {
    pub iteration: u32,
    pub request_id: Id,
    pub name: String,
    pub method: String,
    pub status: Option<u16>,
    pub duration_ms: f64,
    pub error: Option<String>,
    pub assertions: Vec<AssertionResult>,
    /// Messages received by a streaming request; `None` for plain HTTP.
    pub streamed: Option<u32>,
}

impl StepResult {
    pub fn passed(&self) -> bool {
        self.error.is_none() && self.assertions.iter().all(|a| a.passed)
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RunSummary {
    pub steps: Vec<StepResult>,
    pub cancelled: bool,
}

impl RunSummary {
    pub fn passed(&self) -> usize {
        self.steps.iter().filter(|s| s.passed()).count()
    }

    pub fn failed(&self) -> usize {
        self.steps.len() - self.passed()
    }

    pub fn assertions(&self) -> (usize, usize) {
        let all = self.steps.iter().flat_map(|s| &s.assertions);
        let total = all.clone().count();
        (all.filter(|a| a.passed).count(), total)
    }

    pub fn total_ms(&self) -> f64 {
        self.steps.iter().map(|s| s.duration_ms).sum()
    }
}

/// Request ids in run order for `scope` (depth-first, tree order).
pub fn plan(workspace: &Workspace, scope: Option<&Id>) -> Vec<Id> {
    fn walk(items: &[Item], out: &mut Vec<Id>) {
        for item in items {
            match item {
                Item::Request(r) => out.push(r.id.clone()),
                Item::Folder(f) => walk(&f.children, out),
            }
        }
    }
    let mut out = Vec::new();
    match scope.and_then(|id| workspace.find_item(id)) {
        Some(Item::Folder(f)) => walk(&f.children, &mut out),
        Some(Item::Request(r)) => out.push(r.id.clone()),
        None => walk(&workspace.items, &mut out),
    }
    out
}

/// Connects a streaming request, collects messages (bounded by the options) and
/// folds them into a [`Response`]: status 200 (SSE) or 101 (WebSocket), body = the
/// received messages, one per line, so the usual assertions apply.
fn run_stream(
    workspace: &Workspace,
    id: &Id,
    base_dir: &Path,
    jar: &CookieJar,
    history: &History,
    options: &RunOptions,
    cancel: &AtomicBool,
) -> Result<(Response, Option<u32>), exec::ExecError> {
    let started = Instant::now();
    let mut prepared = exec::prepare(
        workspace,
        id,
        &exec::PrepareOptions {
            base_dir,
            responses: history,
        },
    )?;
    let request = workspace.find_request(id).map(|(_, r)| r);
    let protocol = request.map_or(Protocol::Sse, |r| r.protocol);
    let mut session: StreamSession = if protocol == Protocol::WebSocket {
        // The body is the message composer, not part of the handshake.
        prepared.body.clear();
        prepared
            .headers
            .retain(|(n, _)| !n.eq_ignore_ascii_case("content-type"));
        connect_websocket(prepared.clone(), Some(jar.clone()))
    } else {
        connect_sse(
            prepared.clone(),
            workspace.settings.clone(),
            Some(jar.clone()),
        )
    };
    let deadline = Instant::now() + options.stream_timeout;
    let mut messages: Vec<String> = Vec::new();
    let mut opened = false;
    let mut error = None;
    while (messages.len() as u32) < options.stream_messages
        && !cancel.load(Ordering::Relaxed)
        && !session.is_finished()
    {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match session.recv_timeout(left.min(Duration::from_millis(100))) {
            Some(e) => match e.direction {
                Direction::Incoming => messages.push(e.data),
                Direction::Info if e.label == "open" => opened = true,
                Direction::Error => {
                    error = Some(e.data);
                    break;
                }
                _ => {}
            },
            None => {}
        }
    }
    session.close();
    if let Some(e) = error
        && messages.is_empty()
        && !opened
    {
        return Err(exec::ExecError::Network(e));
    }
    let total_ms = started.elapsed().as_secs_f64() * 1000.0;
    let count = messages.len() as u32;
    let response = Response {
        request_id: Some(id.clone()),
        method: if protocol == Protocol::WebSocket {
            "WS"
        } else {
            "SSE"
        }
        .into(),
        url: prepared.url.to_string(),
        status: if protocol == Protocol::WebSocket {
            101
        } else {
            200
        },
        reason: String::new(),
        http_version: String::new(),
        headers: Vec::new(),
        body: messages.join("\n").into_bytes(),
        truncated: false,
        timings: Timings {
            total_ms,
            ..Timings::default()
        },
        timeline: Vec::new(),
        redirects: 0,
        started_at: 0,
    };
    Ok((response, Some(count)))
}

/// Runs the plan. `on_step` is called after each request (for progress UIs).
#[allow(clippy::too_many_arguments)]
pub fn run(
    workspace: &Workspace,
    options: &RunOptions,
    base_dir: &Path,
    client: &Client,
    jar: &mut CookieJar,
    history: &mut History,
    cancel: &AtomicBool,
    mut on_step: impl FnMut(&StepResult),
) -> RunSummary {
    let ids = plan(workspace, options.scope.as_ref());
    let mut summary = RunSummary::default();
    'outer: for iteration in 1..=options.iterations.max(1) {
        for (index, id) in ids.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                summary.cancelled = true;
                break 'outer;
            }
            if index > 0 && !options.delay.is_zero() {
                std::thread::sleep(options.delay);
            }
            let Some((_, request)) = workspace.find_request(id) else {
                continue;
            };
            let result = if matches!(request.protocol, Protocol::Sse | Protocol::WebSocket) {
                run_stream(workspace, id, base_dir, jar, history, options, cancel)
            } else {
                execute(workspace, id, base_dir, client, Some(jar), history, cancel)
                    .map(|r| (r, None))
            };
            let step = match result {
                Ok((response, streamed)) => {
                    let checks = assertions::check_all(&request.assertions, &response);
                    let step = StepResult {
                        iteration,
                        request_id: id.clone(),
                        name: request.name.clone(),
                        method: if streamed.is_some() {
                            response.method.clone()
                        } else {
                            request.method.as_str().to_owned()
                        },
                        status: Some(response.status),
                        duration_ms: response.timings.total_ms,
                        error: None,
                        assertions: checks,
                        streamed,
                    };
                    history.push(id.clone(), response);
                    step
                }
                Err(e) => StepResult {
                    iteration,
                    request_id: id.clone(),
                    name: request.name.clone(),
                    method: request.method.as_str().to_owned(),
                    status: None,
                    duration_ms: 0.0,
                    error: Some(e.to_string()),
                    assertions: Vec::new(),
                    streamed: None,
                },
            };
            on_step(&step);
            let failed = !step.passed();
            summary.steps.push(step);
            if failed && options.bail {
                break 'outer;
            }
        }
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Folder, Method, Request};

    #[test]
    fn plans_in_tree_order_and_scopes_to_folders() {
        let mut ws = Workspace::new("W");
        let mut folder = Folder::new("F");
        let a = Request::new("a", Method::Get, "x");
        let b = Request::new("b", Method::Get, "x");
        let c = Request::new("c", Method::Get, "x");
        let (a_id, b_id, c_id, f_id) =
            (a.id.clone(), b.id.clone(), c.id.clone(), folder.id.clone());
        folder.children.push(Item::Request(b));
        ws.items.push(Item::Request(a));
        ws.items.push(Item::Folder(folder));
        ws.items.push(Item::Request(c));
        assert_eq!(plan(&ws, None), vec![a_id, b_id.clone(), c_id]);
        assert_eq!(plan(&ws, Some(&f_id)), vec![b_id]);
    }
}
