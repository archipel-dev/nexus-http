//! Collection runner view: scope and options, live progress and per-request
//! results with assertion details.

use super::response::{TableState, rows_height};
use crate::state::NexusState;
use archipel_kit::behaviour::WidgetExt;
use archipel_kit::components::{
    Button, Cell, Checkbox, Dropdown, EmptyState, IconButton, ListRow, ProgressBar, SectionHeader,
    Spinner, Table, Tag, TextField, Tooltip, TooltipExt, VirtualList,
};
use archipel_kit::tokens::{size, typography};
use archipel_kit::{Glyph, Tone, egui};
use egui::{Align, Layout, RichText, Ui};
use nexus_http_core::Client;
use nexus_http_core::cookies::CookieJar;
use nexus_http_core::history::History;
use nexus_http_core::model::{Id, Item};
use nexus_http_core::response::format_duration;
use nexus_http_core::runner::{self, RunOptions, RunSummary, StepResult};
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

enum Event {
    Step(StepResult),
    Done(RunSummary, Box<History>, CookieJar),
}

struct Job {
    rx: Receiver<Event>,
    cancel: Arc<AtomicBool>,
    started: Instant,
    planned: usize,
}

#[derive(Default)]
pub struct RunnerUi {
    pub options: RunOptions,
    delay_text: String,
    steps: Vec<StepResult>,
    summary: Option<RunSummary>,
    job: Option<Job>,
    iterations_text: String,
    plan_table: TableState,
    results_table: TableState,
    details: Option<usize>,
    elapsed: Option<Duration>,
}

/// One entry of the scope dropdown; equal when it names the same folder.
#[derive(Clone)]
struct Scope {
    id: Option<Id>,
    text: String,
}

impl PartialEq for Scope {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.text)
    }
}

impl RunnerUi {
    pub fn set_scope(&mut self, scope: Option<Id>) {
        if self.job.is_none() {
            self.options.scope = scope;
        }
    }

    pub fn is_running(&self) -> bool {
        self.job.is_some()
    }

    pub fn start(&mut self, state: &NexusState) {
        if self.job.is_some() {
            return;
        }
        let workspace = state.workspace.clone();
        let mut jar = state.jar.clone();
        let mut history = state.history.clone();
        let base_dir = state.base_dir().to_owned();
        let options = self.options.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let (tx, rx) = channel();
        let planned = runner::plan(&workspace, options.scope.as_ref()).len()
            * options.iterations.max(1) as usize;
        std::thread::spawn(move || {
            let client = Client::new(workspace.settings.clone());
            let step_tx = tx.clone();
            let summary = runner::run(
                &workspace,
                &options,
                &base_dir,
                &client,
                &mut jar,
                &mut history,
                &flag,
                |step| {
                    let _ = step_tx.send(Event::Step(step.clone()));
                },
            );
            let _ = tx.send(Event::Done(summary, Box::new(history), jar));
        });
        self.steps.clear();
        self.summary = None;
        self.details = None;
        self.results_table.selected = None;
        self.elapsed = None;
        self.job = Some(Job {
            rx,
            cancel,
            started: Instant::now(),
            planned,
        });
    }

    pub fn stop(&mut self) {
        if let Some(job) = &self.job {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }

    /// Collects progress; merges responses and cookies into `state` when done.
    pub fn poll(&mut self, state: &mut NexusState) -> bool {
        let Some(job) = &self.job else { return false };
        let started = job.started;
        let mut finished = None;
        while let Ok(event) = job.rx.try_recv() {
            match event {
                Event::Step(step) => self.steps.push(step),
                Event::Done(summary, history, jar) => finished = Some((summary, history, jar)),
            }
        }
        if let Some((summary, history, jar)) = finished {
            for id in summary
                .steps
                .iter()
                .map(|s| &s.request_id)
                .collect::<HashSet<_>>()
            {
                if let Some(entries) = history.entries.get(id) {
                    state.history.entries.insert(id.clone(), entries.clone());
                    state.outcomes.remove(id);
                }
            }
            state.jar = jar;
            state.mark_history_dirty();
            self.elapsed = Some(started.elapsed());
            // The first failure is selected so its details are visible at once.
            self.results_table.selected = self.steps.iter().position(|s| !s.passed());
            self.details = self.results_table.selected;
            self.summary = Some(summary);
            self.job = None;
        }
        self.job.is_some()
    }
}

/// Folders available as run scopes, with indentation depth.
fn folders(items: &[Item], depth: usize, out: &mut Vec<(usize, Id, String)>) {
    for item in items {
        if let Item::Folder(f) = item {
            out.push((depth, f.id.clone(), f.name.clone()));
            folders(&f.children, depth + 1, out);
        }
    }
}

pub fn show(ui: &mut Ui, state: &mut NexusState, local: &mut RunnerUi, close: &mut bool) {
    if local.poll(state) {
        ui.ctx().request_repaint_after(Duration::from_millis(50));
    }
    let scope_name = match local
        .options
        .scope
        .as_ref()
        .and_then(|id| state.workspace.find_item(id))
    {
        Some(item) => item.name().to_owned(),
        None => state.workspace.name.clone(),
    };
    let planned = runner::plan(&state.workspace, local.options.scope.as_ref());
    let plural = if planned.len() == 1 { "" } else { "s" };

    egui::Frame::new()
        .inner_margin(egui::Margin::same(16))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Collection Runner")
                        .font(typography::semibold(ui.ctx(), 16.0)),
                );
                ui.label(RichText::new(format!("·  {scope_name}")).size(14.0));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if ui
                        .add(
                            IconButton::new(Glyph::Close, "Back to requests")
                                .with(Tooltip::new("Back to requests").shortcut("Esc")),
                        )
                        .clicked()
                    {
                        *close = true;
                    }
                });
            });
            ui.add_space(12.0);
            options_row(ui, state, local, &scope_name, planned.len());
            ui.add_space(12.0);

            let total = local
                .job
                .as_ref()
                .map(|j| j.planned)
                .unwrap_or(local.steps.len())
                .max(1);
            let done = local.steps.len();
            if local.is_running() {
                ui.horizontal(|ui| {
                    ui.add(Spinner::new().label("Running"));
                    ui.label(format!("Running {done} of {total}…"));
                });
                ui.add_space(4.0);
                ui.add(ProgressBar::new(Some(done as f32 / total as f32)).label("Run progress"));
            } else if let Some(summary) = &local.summary {
                ui.horizontal(|ui| {
                    ui.add(Tag::new(format!("{} passed", summary.passed())).tone(Tone::Success));
                    let streamed = summary.steps.iter().filter(|s| s.streamed.is_some()).count();
                    if streamed > 0 {
                        ui.add(Tag::new(format!("{streamed} streamed")).tone(Tone::Accent));
                    }
                    if summary.failed() > 0 {
                        ui.add(
                            Tag::new(format!("{} failed", summary.failed())).tone(Tone::Error),
                        );
                    }
                    let (ok, all) = summary.assertions();
                    if all > 0 {
                        ui.add(Tag::new(format!("{ok}/{all} assertions")).tone(Tone::Secondary));
                    }
                    let ms = local
                        .elapsed
                        .map(|d| d.as_secs_f64() * 1000.0)
                        .unwrap_or(summary.total_ms());
                    ui.add(Tag::new(format_duration(ms)).tone(Tone::Secondary));
                    if summary.cancelled {
                        ui.add(Tag::new("stopped").tone(Tone::Warning));
                    }
                });
            } else {
                ui.label(format!(
                    "{} request{plural} will run in tree order. Responses feed {{% response %}} chaining like manual sends.",
                    planned.len()
                ));
            }
            ui.add_space(10.0);

            if local.steps.is_empty() && !local.is_running() {
                plan_table(ui, state, local, &planned);
            } else {
                results(ui, local);
            }
        });
}

fn options_row(
    ui: &mut Ui,
    state: &mut NexusState,
    local: &mut RunnerUi,
    scope_name: &str,
    planned: usize,
) {
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = 12.0;
        let running = local.is_running();

        let mut scopes = vec![Scope {
            id: None,
            text: format!("Whole workspace ({})", state.workspace.name),
        }];
        let mut list = Vec::new();
        folders(&state.workspace.items, 0, &mut list);
        scopes.extend(list.into_iter().map(|(depth, id, name)| Scope {
            id: Some(id),
            text: format!("{}{name}", "  ".repeat(depth + 1)),
        }));
        let mut current = scopes
            .iter()
            .find(|s| s.id == local.options.scope)
            .cloned()
            .unwrap_or_else(|| scopes[0].clone());
        let before = current.id.clone();
        ui.add_enabled_ui(!running, |ui| {
            ui.add(
                Dropdown::new("runner-scope", &mut current, scopes)
                    .label(format!("Run scope: {scope_name}"))
                    .width(240.0),
            );
        });
        if current.id != before {
            local.set_scope(current.id);
        }

        if local.iterations_text.is_empty() {
            local.iterations_text = local.options.iterations.to_string();
        }
        let parsed = local.iterations_text.trim().parse::<u32>();
        let invalid = !matches!(parsed, Ok(1..=100));
        let edit = ui.add(
            TextField::new(&mut local.iterations_text)
                .label("Iterations")
                .error(invalid.then_some("1 to 100"))
                .enabled(!running)
                .width(80.0),
        );
        if edit.changed()
            && let Ok(n @ 1..=100) = local.iterations_text.trim().parse::<u32>()
        {
            local.options.iterations = n;
        }

        if local.delay_text.is_empty() {
            local.delay_text = local.options.delay.as_millis().to_string();
        }
        let delay = ui.add(
            TextField::new(&mut local.delay_text)
                .label("Delay (ms)")
                .enabled(!running)
                .width(90.0),
        );
        if delay.changed() {
            local.options.delay =
                Duration::from_millis(local.delay_text.trim().parse().unwrap_or(0));
        }
        ui.add_space(4.0);
        ui.add_enabled(
            !running,
            Checkbox::new(&mut local.options.bail, "Stop on first failure"),
        );
        ui.with_layout(Layout::right_to_left(Align::TOP), |ui| {
            if running {
                if ui
                    .add(Button::secondary("Stop").icon(Glyph::Stop))
                    .clicked()
                {
                    local.stop();
                }
            } else {
                let label = format!(
                    "Run {planned} request{}",
                    if planned == 1 { "" } else { "s" }
                );
                let run = ui.add(Button::primary(label).icon(Glyph::Run).enabled(planned > 0));
                if planned == 0 {
                    run.tooltip("This scope has no requests");
                } else if run.clicked() {
                    state.flush();
                    local.start(state);
                }
            }
        });
    });
}

fn plan_table(ui: &mut Ui, state: &NexusState, local: &mut RunnerUi, planned: &[Id]) {
    if planned.is_empty() {
        EmptyState::new("Nothing to run")
            .icon(Glyph::Run)
            .hint("Add requests to this folder or pick another scope.")
            .show(ui);
        return;
    }
    let height = rows_height(ui);
    let rows: Vec<_> = planned
        .iter()
        .filter_map(|id| state.workspace.find_request(id))
        .collect();
    let (columns, selected) = local.plan_table.columns(
        ui,
        &[
            ("Method", 80.0, true),
            ("Request", 240.0, false),
            ("Assertions", 100.0, false),
            ("Folder", 300.0, false),
        ],
    );
    Table::new("runner-plan", columns, rows.len(), selected)
        .name("Requests to run")
        .height(height)
        .show(ui, |row, col| {
            let (folders, request) = &rows[row];
            match col {
                0 => {
                    let method = match request.protocol {
                        nexus_http_core::model::Protocol::Sse => "SSE".to_owned(),
                        nexus_http_core::model::Protocol::WebSocket => "WS".to_owned(),
                        _ => request.method.to_string(),
                    };
                    Cell::new(method.clone()).tone(Tone::method(&method))
                }
                1 => Cell::new(request.name.as_str()),
                2 => Cell::new(match request.assertions.len() {
                    0 => String::new(),
                    n => n.to_string(),
                })
                .muted(),
                _ => Cell::new(
                    folders
                        .iter()
                        .map(|f| f.name.as_str())
                        .collect::<Vec<_>>()
                        .join(" / "),
                )
                .muted(),
            }
        });
}

fn results(ui: &mut Ui, local: &mut RunnerUi) {
    let iterations = local.options.iterations > 1;
    let details_h = 150.0;
    let table_h = (rows_height(ui) - details_h).max(size::ROW_HEIGHT * 3.0);
    let steps = &local.steps;
    let (columns, selected) = local.results_table.columns(
        ui,
        &[
            ("Result", 100.0, false),
            ("Method", 80.0, true),
            ("Request", 240.0, false),
            ("Iteration", 80.0, false),
            ("Status", 80.0, true),
            ("Assertions", 110.0, false),
            ("Time", 90.0, true),
        ],
    );
    let out = Table::new("runner-results", columns, steps.len(), selected)
        .name("Run results")
        .height(table_h)
        .show(ui, |row, col| {
            let step = &steps[row];
            match col {
                0 if step.passed() && step.streamed.is_some() => {
                    Cell::new("Streamed").icon(Glyph::Check).tone(Tone::Accent)
                }
                0 if step.passed() => Cell::new("Pass").icon(Glyph::Check).tone(Tone::Success),
                0 => Cell::new("Fail").icon(Glyph::Close).tone(Tone::Error),
                1 => Cell::new(step.method.as_str()).tone(Tone::method(&step.method)),
                2 => Cell::new(step.name.as_str()),
                3 if iterations => Cell::new(format!("#{}", step.iteration)).muted(),
                3 => Cell::new(""),
                4 => match step.status {
                    Some(s) => Cell::new(s.to_string()).tone(Tone::status(s)),
                    None => Cell::new("error").tone(Tone::Error),
                },
                5 if step.assertions.is_empty() => Cell::new(""),
                5 => {
                    let ok = step.assertions.iter().filter(|a| a.passed).count();
                    let cell = Cell::new(format!("{ok}/{}", step.assertions.len()));
                    if ok == step.assertions.len() {
                        cell.muted()
                    } else {
                        cell.tone(Tone::Error)
                    }
                }
                _ => Cell::new(format_duration(step.duration_ms)).muted(),
            }
        });
    if out.selection_changed || out.activated.is_some() {
        local.details = local.results_table.selected;
    }
    let Some(step) = local.details.and_then(|i| local.steps.get(i)) else {
        return;
    };
    ui.add(SectionHeader::new(format!("{} · details", step.name)));
    let mut lines: Vec<(Glyph, Tone, String, Option<String>)> = Vec::new();
    if let Some(e) = &step.error {
        lines.push((Glyph::Error, Tone::Error, e.clone(), None));
    }
    for a in &step.assertions {
        let (glyph, tone) = if a.passed {
            (Glyph::Check, Tone::Success)
        } else {
            (Glyph::Close, Tone::Error)
        };
        let got = (!a.passed).then(|| format!("got {}", a.actual));
        lines.push((glyph, tone, a.description.clone(), got));
    }
    if let Some(n) = step.streamed {
        let text = format!("Received {n} message{} (bounded receive)", plural_s(n));
        lines.insert(0, (Glyph::Check, Tone::Accent, text, None));
    }
    if lines.is_empty() {
        ui.label("No assertions on this request.");
        return;
    }
    let mut none = None;
    VirtualList::new("runner-details", lines.len(), &mut none)
        .name("Assertion details")
        .height(details_h - size::HEADER_HEIGHT)
        .show(ui, |i| {
            let (glyph, tone, text, got) = &lines[i];
            let row = ListRow::new(text).icon(*glyph).icon_tone(*tone);
            match got {
                Some(got) => row.secondary(got),
                None => row,
            }
        });
}

fn plural_s(n: u32) -> &'static str {
    if n == 1 { "" } else { "s" }
}
