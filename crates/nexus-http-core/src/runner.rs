//! Collection runner: sends the requests of a workspace or folder in order,
//! feeding each response into the history so later requests can chain on
//! earlier ones, and checks every request's assertions.

use crate::assertions::{self, AssertionResult};
use crate::cookies::CookieJar;
use crate::history::History;
use crate::model::{Id, Item, Workspace};
use crate::{Client, execute};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOptions {
    /// Folder to run; `None` runs the whole workspace.
    pub scope: Option<Id>,
    pub iterations: u32,
    /// Pause between requests.
    pub delay: Duration,
    /// Stop at the first failed request or assertion.
    pub bail: bool,
}

impl Default for RunOptions {
    fn default() -> Self {
        Self {
            scope: None,
            iterations: 1,
            delay: Duration::ZERO,
            bail: false,
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
            let result = execute(workspace, id, base_dir, client, Some(jar), history, cancel);
            let step = match result {
                Ok(response) => {
                    let checks = assertions::check_all(&request.assertions, &response);
                    let step = StepResult {
                        iteration,
                        request_id: id.clone(),
                        name: request.name.clone(),
                        method: request.method.as_str().to_owned(),
                        status: Some(response.status),
                        duration_ms: response.timings.total_ms,
                        error: None,
                        assertions: checks,
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
