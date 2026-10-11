//! Response pane: status line, Preview (pretty + JSONPath filter), Raw,
//! Headers, Cookies, Timeline and Tests tabs, and response history.
//!
//! Built only from archipel-kit. The six tabs are views of the same response, so they are a
//! `SegmentedControl` (kit docs: "switching views"), not editor `TabBar` tabs.
//! The body text itself still goes through `widgets::code_view` until the kit has a
//! `CodeView`.

use crate::state::{NexusState, Outcome, ResponseTab};
use crate::widgets::{Syntax, code_view};
use archipel_kit::behaviour::WidgetExt;
use archipel_kit::components::{
    Banner, Button, Cell, Column, Dropdown, EmptyState, IconButton, ListRow, ProgressBar,
    SectionHeader, SegmentedControl, Severity, Spinner, Table, Tag, TextField, Tooltip,
    TooltipExt, VirtualList,
};
use archipel_kit::tokens::size;
use archipel_kit::{Glyph, Tone, egui};
use egui::Ui;
use nexus_http_core::assertions::AssertionResult;
use nexus_http_core::model::{Id, Protocol};
use nexus_http_core::response::{BodyKind, Response, TimelineKind, format_duration, format_size};
use std::fmt;

/// One entry of the history dropdown; equal when it points at the same response.
#[derive(Clone)]
struct HistoryChoice {
    index: usize,
    text: String,
}

impl PartialEq for HistoryChoice {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index
    }
}

impl fmt::Display for HistoryChoice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.text)
    }
}

#[derive(Default)]
pub struct ResponseUi {
    pub filter: String,
    /// Index into the request's history (0 = newest) when browsing old responses.
    pub history_index: Option<usize>,
    last_request: Option<Id>,
    headers: TableState,
    set_cookies: TableState,
    jar: TableState,
    timeline: TableState,
    tests: Option<usize>,
    events: Option<usize>,
}

/// Column widths (user-resizable) and the selected row of one kit `Table`.
#[derive(Default)]
pub(crate) struct TableState {
    pub(crate) columns: Vec<Column>,
    pub(crate) selected: Option<usize>,
}

impl TableState {
    /// Creates the columns once, then lets the last one fill the width.
    pub(crate) fn columns(&mut self, ui: &Ui, spec: &[(&str, f32, bool)]) -> (&mut [Column], &mut Option<usize>) {
        if self.columns.is_empty() {
            self.columns = spec
                .iter()
                .map(|&(title, width, mono)| {
                    let column = Column::new(title, width);
                    if mono { column.mono() } else { column }
                })
                .collect();
        }
        let others: f32 = self.columns.iter().rev().skip(1).map(|c| c.width).sum();
        if let Some(last) = self.columns.last_mut() {
            last.width = (ui.available_width() - others - 24.0).max(120.0);
        }
        (&mut self.columns, &mut self.selected)
    }
}

/// Height left for a table's rows below its header.
pub(crate) fn rows_height(ui: &Ui) -> f32 {
    (ui.available_height() - size::HEADER_HEIGHT).max(size::ROW_HEIGHT * 3.0)
}

pub fn show(ui: &mut Ui, state: &mut NexusState, id: &Id, local: &mut ResponseUi) {
    let protocol = state
        .workspace
        .find_request(id)
        .map(|(_, r)| r.protocol)
        .unwrap_or_default();
    if !protocol.is_http() {
        stream_view(ui, state, id, local, protocol);
        return;
    }
    if local.last_request.as_ref() != Some(id) {
        local.history_index = None;
        local.last_request = Some(id.clone());
    }
    let sending = state.sending_id() == Some(id);
    let outcome = match local.history_index {
        Some(i) => state
            .history
            .for_request(id)
            .nth(i)
            .map(|r| Outcome::Response(Box::new(r.clone()), Vec::new())),
        None => state.outcome(id),
    };

    let mut copy = false;
    let mut save = false;
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(size::CONTENT_INSET);
        if sending {
            ui.add(Spinner::new().label("Sending"));
            let elapsed = state
                .sending_elapsed()
                .map(|d| d.as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
            ui.label(format!("Sending…  {}", format_duration(elapsed)));
            return;
        }
        match &outcome {
            Some(Outcome::Response(r, _)) => {
                let label = format!("{} {}", r.status, r.reason);
                ui.add(Tag::new(label.trim()).tone(Tone::status(r.status)));
                ui.add(Tag::new(format_duration(r.timings.total_ms)).tone(Tone::Secondary))
                    .tooltip(format!(
                        "Prepare {}\nWaiting {}\nDownload {}",
                        format_duration(r.timings.prepare_ms),
                        format_duration(r.timings.wait_ms),
                        format_duration(r.timings.download_ms)
                    ));
                ui.add(Tag::new(format_size(r.body.len() as u64)).tone(Tone::Secondary));
                if r.redirects > 0 {
                    ui.add(
                        Tag::new(format!(
                            "{} redirect{}",
                            r.redirects,
                            if r.redirects == 1 { "" } else { "s" }
                        ))
                        .tone(Tone::Link),
                    );
                }
                if r.truncated {
                    ui.add(Tag::new("truncated").tone(Tone::Warning));
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(size::CONTENT_INSET);
                    save = ui
                        .add(
                            IconButton::new(Glyph::Save, "Save body to file")
                                .with(Tooltip::new("Save body to file")),
                        )
                        .clicked();
                    copy = ui
                        .add(
                            IconButton::new(Glyph::Copy, "Copy body")
                                .with(Tooltip::new("Copy body")),
                        )
                        .clicked();
                    history_picker(ui, state, id, local);
                });
            }
            Some(Outcome::Error(_)) => {
                ui.add(Tag::new("Error").tone(Tone::Error));
            }
            None => {}
        }
    });
    if sending {
        ui.add_space(4.0);
        ui.add(ProgressBar::new(None).label("Sending request"));
        ui.ctx().request_repaint();
    }
    ui.add_space(4.0);

    let Some(outcome) = outcome else {
        if !sending {
            idle(ui);
        }
        return;
    };
    let (response, checks) = match outcome {
        Outcome::Error(message) => {
            error_card(ui, &message);
            return;
        }
        Outcome::Response(r, checks) => (r, checks),
    };

    if copy {
        ui.ctx().copy_text(response.body_text());
        state.notify("Response body copied");
    }
    if save {
        save_body(state, id, &response);
    }

    let cookies = response.headers_named("set-cookie").count();
    let passed = checks.iter().filter(|c| c.passed).count();
    ui.horizontal(|ui| {
        ui.add_space(size::CONTENT_INSET);
        let options = ResponseTab::ALL.map(|tab| {
            let label = match tab {
                ResponseTab::Headers => format!("{} ({})", tab.label(), response.headers.len()),
                ResponseTab::Cookies if cookies > 0 => format!("{} ({cookies})", tab.label()),
                ResponseTab::Tests if !checks.is_empty() => {
                    format!("{} ({passed}/{})", tab.label(), checks.len())
                }
                _ => tab.label().to_owned(),
            };
            (tab, label)
        });
        ui.add(SegmentedControl::new(&mut state.response_tab, options));
    });
    ui.add_space(6.0);

    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(size::CONTENT_INSET as i8, 0))
        .show(ui, |ui| match state.response_tab {
            ResponseTab::Preview => preview(ui, &response, local),
            // TODO(kit CodeView): replace with the kit CodeView when it lands.
            ResponseTab::Raw => code_view(ui, &response.body_text(), Syntax::Plain, "raw"),
            ResponseTab::Headers => headers(ui, &response, local),
            ResponseTab::Cookies => cookies_tab(ui, &response, state, local),
            ResponseTab::Timeline => timeline(ui, &response, local),
            ResponseTab::Tests => tests(ui, &checks, local),
        });
}

fn save_body(state: &mut NexusState, id: &Id, response: &Response) {
    let name = state
        .workspace
        .find_request(id)
        .map(|(_, r)| r.name.clone())
        .unwrap_or_else(|| "response".into());
    let stem: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let ext = match response.body_kind() {
        BodyKind::Json => "json",
        BodyKind::Image | BodyKind::Binary => "bin",
        _ => "txt",
    };
    let path = state.base_dir().join(format!("{stem}.response.{ext}"));
    match std::fs::write(&path, &response.body) {
        Ok(()) => state.notify(format!("Saved {}", path.display())),
        Err(e) => state.notify(format!("Could not save the response: {e}")),
    }
}

fn idle(ui: &mut Ui) {
    EmptyState::new("No response yet")
        .icon(Glyph::Send)
        .hint("Send the request with Ctrl+Enter. Focus the URL with Ctrl+L, switch requests with Ctrl+P.")
        .show(ui);
}

fn error_card(ui: &mut Ui, message: &str) {
    ui.add_space(8.0);
    let hint = if message.contains("undefined variable") {
        "Define the variable in the environment (Ctrl+E) or fix its name."
    } else if message.contains("could not connect") {
        "Check the host and port, and whether a proxy is required (Settings)."
    } else if message.contains("timed out") {
        "Increase the timeout in the request's Settings tab."
    } else if message.contains("certificate") {
        "Turn off TLS verification for this request or add a CA certificate."
    } else {
        "Open the Timeline tab after a retry for the full exchange."
    };
    Banner::new(
        Severity::Error,
        format!("The request could not be completed: {message}"),
    )
    .show(ui);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(size::CONTENT_INSET);
        ui.label(hint);
    });
}

fn history_picker(ui: &mut Ui, state: &NexusState, id: &Id, local: &mut ResponseUi) {
    let choices: Vec<HistoryChoice> = state
        .history
        .for_request(id)
        .enumerate()
        .map(|(index, r)| HistoryChoice {
            index,
            text: format!(
                "{} {} · {} · {}",
                r.status,
                r.reason,
                format_duration(r.timings.total_ms),
                relative_time(r.started_at)
            )
            .replace("  ", " "),
        })
        .collect();
    if choices.is_empty() {
        return;
    }
    let mut current = choices[local.history_index.unwrap_or(0).min(choices.len() - 1)].clone();
    let before = current.index;
    ui.add(
        Dropdown::new("response-history", &mut current, choices)
            .label("Response history")
            .width(250.0),
    );
    if current.index != before {
        local.history_index = (current.index > 0).then_some(current.index);
    }
}

fn relative_time(started_ms: u64) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let secs = now.saturating_sub(started_ms) / 1000;
    match secs {
        0..=9 => "just now".into(),
        10..=59 => format!("{secs} s ago"),
        60..=3599 => format!("{} min ago", secs / 60),
        3600..=86_399 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

fn preview(ui: &mut Ui, response: &Response, local: &mut ResponseUi) {
    let kind = response.body_kind();
    let filter_h = if kind == BodyKind::Json { 64.0 } else { 0.0 };
    let body_h = (ui.available_height() - filter_h).max(40.0);
    ui.allocate_ui(egui::vec2(ui.available_width(), body_h), |ui| match kind {
        BodyKind::Empty => {
            EmptyState::new("Empty body")
                .hint("The response has no content.")
                .show(ui);
        }
        BodyKind::Json => {
            match nexus_http_core::filter::filter_json_text(&response.body_text(), &local.filter) {
                // TODO(kit CodeView): replace with the kit CodeView when it lands.
                Ok(text) => code_view(ui, &text, Syntax::Json, "pretty"),
                Err(e) => {
                    Banner::new(Severity::Warning, e).show(ui);
                }
            }
        }
        BodyKind::Image => {
            EmptyState::new("Image response")
                .icon(Glyph::Image)
                .hint(format!(
                    "{} · {}",
                    response.content_type().unwrap_or("image"),
                    format_size(response.body.len() as u64)
                ))
                .show(ui);
        }
        BodyKind::Binary => {
            EmptyState::new("Binary response")
                .icon(Glyph::File)
                .hint(format!(
                    "{} of {}",
                    format_size(response.body.len() as u64),
                    response.content_type().unwrap_or("unknown type")
                ))
                .show(ui);
        }
        // TODO(kit CodeView): replace with the kit CodeView when it lands.
        _ => code_view(ui, &response.body_text(), Syntax::Plain, "pretty-text"),
    });
    if kind == BodyKind::Json {
        ui.add_space(4.0);
        ui.add(
            TextField::search(&mut local.filter)
                .label("JSONPath filter")
                .placeholder("e.g. $.items[*].name")
                .monospace(true),
        );
    }
}

fn headers(ui: &mut Ui, response: &Response, local: &mut ResponseUi) {
    let height = rows_height(ui);
    let state = &mut local.headers;
    let (columns, selected) = state.columns(ui, &[("Name", 220.0, true), ("Value", 400.0, true)]);
    Table::new(
        "resp-headers",
        columns,
        response.headers.len(),
        selected,
    )
    .name("Response headers")
    .height(height)
    .show(ui, |row, col| {
        let (name, value) = &response.headers[row];
        match col {
            0 => Cell::new(name.as_str()).tone(Tone::Accent),
            _ => Cell::new(value.as_str()),
        }
    });
}

fn cookies_tab(ui: &mut Ui, response: &Response, state: &mut NexusState, local: &mut ResponseUi) {
    let set: Vec<&str> = response.headers_named("set-cookie").collect();
    ui.add(SectionHeader::new("Set by this response"));
    if set.is_empty() {
        ui.label("This response set no cookies");
    } else {
        let (columns, selected) = local.set_cookies.columns(ui, &[("Set-Cookie", 400.0, true)]);
        Table::new(
            "resp-set-cookies",
            columns,
            set.len(),
            selected,
        )
        .name("Cookies set by this response")
        .height((size::ROW_HEIGHT * set.len() as f32).min(120.0))
        .show(ui, |row, _| Cell::new(set[row]));
    }
    ui.add(SectionHeader::new(format!(
        "Workspace cookie jar · {}",
        state.jar.cookies.len()
    )));
    if state.jar.cookies.is_empty() {
        return;
    }
    if ui.add(Button::ghost("Clear all").small(true)).clicked() {
        state.jar.clear();
        return;
    }
    let height = rows_height(ui);
    let jar = &state.jar.cookies;
    let (columns, selected) = local.jar.columns(
        ui,
        &[
            ("Name", 160.0, true),
            ("Value", 200.0, true),
            ("Domain", 160.0, false),
            ("Path", 120.0, false),
        ],
    );
    Table::new("resp-jar", columns, jar.len(), selected)
        .name("Cookie jar")
        .height(height)
        .show(ui, |row, col| {
            let c = &jar[row];
            match col {
                0 => Cell::new(c.name.as_str()).tone(Tone::Accent),
                1 => Cell::new(c.value.as_str()),
                2 => Cell::new(c.domain.as_str()),
                _ => Cell::new(c.path.as_str()),
            }
        });
}

fn timeline(ui: &mut Ui, response: &Response, local: &mut ResponseUi) {
    let lines: Vec<(&str, Tone, &str)> = response
        .timeline
        .iter()
        .flat_map(|entry| {
            let (prefix, tone) = match entry.kind {
                TimelineKind::Info => ("*", Tone::Secondary),
                TimelineKind::RequestHeader => (">", Tone::Link),
                TimelineKind::RequestBody => ("|", Tone::Default),
                TimelineKind::ResponseHeader => ("<", Tone::Success),
                TimelineKind::ResponseBody => ("|", Tone::Default),
                TimelineKind::Error => ("!", Tone::Error),
            };
            entry.text.lines().map(move |line| (prefix, tone, line))
        })
        .collect();
    let height = rows_height(ui);
    let (columns, selected) = local.timeline.columns(ui, &[("Dir", 40.0, true), ("Message", 400.0, true)]);
    Table::new(
        "resp-timeline",
        columns,
        lines.len(),
        selected,
    )
    .name("Timeline")
    .height(height)
    .show(ui, |row, col| {
        let (prefix, tone, text) = lines[row];
        match col {
            0 => Cell::new(prefix).tone(tone),
            _ => Cell::new(text).tone(tone),
        }
    });
}

fn tests(ui: &mut Ui, checks: &[AssertionResult], local: &mut ResponseUi) {
    if checks.is_empty() {
        EmptyState::new("No assertions")
            .icon(Glyph::Test)
            .hint("Add assertions in the request's Tests tab.")
            .show(ui);
        return;
    }
    let height = ui.available_height().max(size::ROW_HEIGHT * 3.0);
    VirtualList::new("resp-tests", checks.len(), &mut local.tests)
        .name("Assertion results")
        .height(height)
        .show(ui, |i| {
            let c = &checks[i];
            let row = ListRow::new(&c.description);
            if c.passed {
                row.icon(Glyph::Check).icon_tone(Tone::Success)
            } else {
                row.icon(Glyph::Close)
                    .icon_tone(Tone::Error)
                    .secondary(format!("got {}", c.actual))
            }
        });
}

fn stream_view(
    ui: &mut Ui,
    state: &mut NexusState,
    id: &Id,
    local: &mut ResponseUi,
    protocol: Protocol,
) {
    use nexus_http_core::stream::Direction;
    let connected = state.stream_connected(id);
    let events = state
        .streams
        .get(id)
        .map(|s| s.events.as_slice())
        .unwrap_or(&[]);
    ui.add_space(8.0);
    let mut clear = false;
    ui.horizontal(|ui| {
        ui.add_space(size::CONTENT_INSET);
        if connected {
            ui.add(Tag::new("Connected").tone(Tone::Success));
        } else if events.is_empty() {
            ui.add(Tag::new("Not connected").tone(Tone::Secondary));
        } else if events.iter().any(|e| e.direction == Direction::Error) {
            ui.add(Tag::new("Failed").tone(Tone::Error));
        } else {
            ui.add(Tag::new("Closed").tone(Tone::Secondary));
        }
        let received = events
            .iter()
            .filter(|e| e.direction == Direction::Incoming)
            .count();
        let sent = events
            .iter()
            .filter(|e| e.direction == Direction::Outgoing)
            .count();
        ui.add(Tag::new(format!("{received} received")).tone(Tone::Secondary));
        if matches!(protocol, Protocol::WebSocket | Protocol::Grpc) {
            ui.add(Tag::new(format!("{sent} sent")).tone(Tone::Secondary));
        }
        if let Some(last) = events.last() {
            ui.add(Tag::new(format_duration(last.at_ms)).tone(Tone::Secondary));
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(size::CONTENT_INSET);
            if !events.is_empty() && ui.add(Button::secondary("Clear")).clicked() {
                clear = true;
            }
        });
    });
    if connected {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    ui.add_space(8.0);

    if events.is_empty() {
        let hint = match protocol {
            Protocol::WebSocket => "Connect, then send messages from the Message tab (Ctrl+Enter).",
            Protocol::Grpc => {
                "Pick a method, then Connect (Ctrl+Enter) to call it. Client-streaming calls take more messages from the Message tab."
            }
            _ => "Connect to stream events as they arrive.",
        };
        EmptyState::new(format!("{} stream", protocol.label()))
            .icon(Glyph::Plug)
            .hint(hint)
            .show(ui);
        return;
    }
    let needle = local.filter.to_lowercase();
    let shown: Vec<_> = events
        .iter()
        .filter(|e| {
            needle.is_empty()
                || e.data.to_lowercase().contains(&needle)
                || e.label.to_lowercase().contains(&needle)
        })
        .collect();
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(size::CONTENT_INSET as i8, 0))
        .show(ui, |ui| {
            let list_h = (ui.available_height() - 52.0).max(size::ROW_HEIGHT * 3.0);
            VirtualList::new("stream-events", shown.len(), &mut local.events)
                .name("Messages")
                .height(list_h)
                .show(ui, |i| {
                    let event = shown[i];
                    let (glyph, tone) = match event.direction {
                        Direction::Incoming => (Glyph::ArrowDown, Tone::Success),
                        Direction::Outgoing => (Glyph::ArrowUp, Tone::Link),
                        Direction::Info => (Glyph::Info, Tone::Secondary),
                        Direction::Error => (Glyph::Error, Tone::Error),
                    };
                    let text = serde_json::from_str::<serde_json::Value>(&event.data)
                        .ok()
                        .filter(|v| v.is_object() || v.is_array())
                        .and_then(|v| serde_json::to_string(&v).ok())
                        .unwrap_or_else(|| event.data.clone());
                    let mut meta = format_duration(event.at_ms);
                    if event.label != "message" {
                        meta = format!("{meta} · {}", event.label);
                    }
                    if let Some(eid) = &event.id {
                        meta = format!("{meta} · id {eid}");
                    }
                    let row = ListRow::new(text).icon(glyph).icon_tone(tone).secondary(meta);
                    if event.direction == Direction::Error {
                        row.label_tone(Tone::Error)
                    } else {
                        row
                    }
                });
            ui.add_space(4.0);
            ui.add(TextField::search(&mut local.filter).label("Filter events"));
        });
    if clear && let Some(s) = state.streams.get_mut(id) {
        s.events.clear();
        local.events = None;
    }
}
