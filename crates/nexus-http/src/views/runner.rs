//! Collection runner view: scope and options, live progress and per-request
//! results with assertion details.

use crate::state::NexusState;
use crate::theme::{Palette, mono, semibold};
use crate::widgets::{
    self, Icon, caption, field_frame, paint_icon, pill, primary_button, secondary_button,
};
use egui::{Align, CornerRadius, Layout, RichText, Sense, Ui, pos2, vec2};
use nexus_http_core::Client;
use nexus_http_core::cookies::CookieJar;
use nexus_http_core::history::History;
use nexus_http_core::model::{Id, Item, Method};
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
    expanded: HashSet<usize>,
    elapsed: Option<Duration>,
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
        self.expanded.clear();
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
            // Failures open expanded so their details are visible at once.
            self.expanded = self
                .steps
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.passed())
                .map(|(i, _)| i)
                .collect();
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
    let pal = Palette::of(ui);
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

    egui::Frame::new().inner_margin(egui::Margin::same(16)).show(ui, |ui| {
        // Header
        ui.horizontal(|ui| {
            ui.label(RichText::new("Collection Runner").font(semibold(ui, 16.0)).color(pal.strong));
            ui.label(RichText::new(format!("·  {scope_name}")).size(14.0).color(pal.weak));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if widgets::icon_button(ui, Icon::Close, pal.weak, "Back to requests").clicked() {
                    *close = true;
                }
            });
        });
        ui.add_space(12.0);

        // Options
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            caption(ui, "Run");
            let mut scopes = vec![(0, None, format!("Whole workspace ({})", state.workspace.name))];
            let mut list = Vec::new();
            folders(&state.workspace.items, 0, &mut list);
            scopes.extend(list.into_iter().map(|(d, id, name)| (d + 1, Some(id), name)));
            egui::ComboBox::from_id_salt("runner-scope")
                .selected_text(&scope_name)
                .width(220.0)
                .show_ui(ui, |ui| {
                    for (depth, id, name) in scopes {
                        let label = format!("{}{name}", "   ".repeat(depth));
                        if ui.selectable_label(local.options.scope == id, label).clicked() {
                            local.set_scope(id);
                        }
                    }
                });
            caption(ui, "Iterations");
            ui.add(egui::DragValue::new(&mut local.options.iterations).range(1..=100));
            caption(ui, "Delay (ms)");
            if local.delay_text.is_empty() {
                local.delay_text = local.options.delay.as_millis().to_string();
            }
            if ui.add(egui::TextEdit::singleline(&mut local.delay_text).desired_width(60.0)).changed() {
                local.options.delay = Duration::from_millis(local.delay_text.trim().parse().unwrap_or(0));
            }
            ui.checkbox(&mut local.options.bail, "Stop on first failure");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if local.is_running() {
                    if secondary_button(ui, "Stop").clicked() {
                        local.stop();
                    }
                } else if primary_button(ui, &format!("Run {} request{}", planned.len(), if planned.len() == 1 { "" } else { "s" }), !planned.is_empty()).clicked() {
                    state.flush();
                    local.start(state);
                }
            });
        });
        ui.add_space(14.0);

        // Progress + summary
        let total = local.job.as_ref().map(|j| j.planned).unwrap_or(local.steps.len()).max(1);
        let done = local.steps.len();
        let passed = local.steps.iter().filter(|s| s.passed()).count();
        let failed = done - passed;
        let (bar, _) = ui.allocate_exact_size(vec2(ui.available_width(), 6.0), Sense::hover());
        ui.painter().rect_filled(bar, CornerRadius::same(3), pal.hover);
        if done > 0 {
            let w = bar.width() * (done as f32 / total as f32).min(1.0);
            let pass_w = w * passed as f32 / done as f32;
            ui.painter().rect_filled(egui::Rect::from_min_size(bar.min, vec2(pass_w, bar.height())), CornerRadius::same(3), pal.success);
            if failed > 0 {
                ui.painter().rect_filled(egui::Rect::from_min_size(pos2(bar.left() + pass_w, bar.top()), vec2(w - pass_w, bar.height())), CornerRadius::same(3), pal.error);
            }
        }
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if local.is_running() {
                ui.add(egui::Spinner::new().size(14.0).color(pal.accent));
                ui.label(RichText::new(format!("Running {done} of {total}…")).color(pal.weak));
            } else if let Some(summary) = &local.summary {
                pill(ui, &format!("{} passed", summary.passed()), pal.success, true);
                if summary.failed() > 0 {
                    pill(ui, &format!("{} failed", summary.failed()), pal.error, true);
                }
                let (ok, all) = summary.assertions();
                if all > 0 {
                    pill(ui, &format!("{ok}/{all} assertions"), pal.weak, false);
                }
                pill(ui, &format_duration(local.elapsed.map(|d| d.as_secs_f64() * 1000.0).unwrap_or(summary.total_ms())), pal.weak, false);
                if summary.cancelled {
                    pill(ui, "stopped", pal.warning, false);
                }
            } else {
                ui.label(RichText::new(format!("{} request{} will run in tree order. Responses feed {{% response %}} chaining like manual sends.", planned.len(), if planned.len() == 1 { "" } else { "s" })).color(pal.weak));
            }
        });
        ui.add_space(10.0);

        // Results
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            if local.steps.is_empty() && !local.is_running() {
                list_plan(ui, state, &planned, &pal);
                return;
            }
            let iterations = local.options.iterations > 1;
            for (i, step) in local.steps.iter().enumerate() {
                let expanded = local.expanded.contains(&i);
                if step_row(ui, step, iterations, expanded, &pal).clicked() && !local.expanded.remove(&i) {
                    local.expanded.insert(i);
                }
                if expanded {
                    step_details(ui, step, &pal);
                }
            }
        });
    });
}

fn list_plan(ui: &mut Ui, state: &NexusState, planned: &[Id], pal: &Palette) {
    for id in planned {
        let Some((folders, request)) = state.workspace.find_request(id) else {
            continue;
        };
        ui.horizontal(|ui| {
            ui.set_min_height(28.0);
            widgets::request_label(ui, request, 40.0);
            ui.add_space(6.0);
            ui.label(RichText::new(&request.name).color(pal.text));
            let path = folders
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>()
                .join(" / ");
            if !path.is_empty() {
                ui.label(RichText::new(path).size(12.0).color(pal.faint));
            }
            if !request.assertions.is_empty() {
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new(format!(
                            "{} assertion{}",
                            request.assertions.len(),
                            if request.assertions.len() == 1 {
                                ""
                            } else {
                                "s"
                            }
                        ))
                        .size(12.0)
                        .color(pal.weak),
                    );
                });
            }
        });
    }
}

fn step_row(
    ui: &mut Ui,
    step: &StepResult,
    iterations: bool,
    expanded: bool,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 34.0), Sense::click());
    if resp.hovered() || expanded {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), pal.hover);
    }
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(vec2(8.0, 0.0)))
            .layout(Layout::left_to_right(Align::Center)),
    );
    let (icon_rect, _) = child.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
    let (icon, color) = if step.passed() {
        (Icon::Check, pal.success)
    } else {
        (Icon::Close, pal.error)
    };
    paint_icon(child.painter(), icon_rect, icon, color);
    child.add_space(4.0);
    widgets::method_label(&mut child, &Method::from(step.method.clone()), 40.0);
    child.add_space(6.0);
    child.label(RichText::new(&step.name).color(pal.strong));
    if iterations {
        child.label(
            RichText::new(format!("#{}", step.iteration))
                .size(12.0)
                .color(pal.faint),
        );
    }
    child.with_layout(Layout::right_to_left(Align::Center), |ui| {
        ui.label(
            RichText::new(format_duration(step.duration_ms))
                .font(mono(12.0))
                .color(pal.weak),
        );
        match step.status {
            Some(s) => {
                pill(ui, &s.to_string(), pal.status(s), true);
            }
            None => {
                pill(ui, "error", pal.error, true);
            }
        }
        if !step.assertions.is_empty() {
            let ok = step.assertions.iter().filter(|a| a.passed).count();
            ui.label(
                RichText::new(format!("{ok}/{} assertions", step.assertions.len()))
                    .size(12.0)
                    .color(if ok == step.assertions.len() {
                        pal.weak
                    } else {
                        pal.error
                    }),
            );
        }
    });
    resp
}

fn step_details(ui: &mut Ui, step: &StepResult, pal: &Palette) {
    field_frame(pal)
        .inner_margin(egui::Margin::symmetric(14, 8))
        .outer_margin(egui::Margin {
            left: 30,
            right: 4,
            top: 2,
            bottom: 6,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            if let Some(e) = &step.error {
                ui.label(RichText::new(e).font(mono(12.0)).color(pal.error));
            }
            if step.assertions.is_empty() && step.error.is_none() {
                ui.label(RichText::new("No assertions on this request.").color(pal.weak));
            }
            for a in &step.assertions {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    paint_icon(
                        ui.painter(),
                        r,
                        if a.passed { Icon::Check } else { Icon::Close },
                        if a.passed { pal.success } else { pal.error },
                    );
                    ui.label(&a.description);
                    if !a.passed {
                        ui.label(RichText::new(format!("got {}", a.actual)).color(pal.weak));
                    }
                });
            }
        });
}
