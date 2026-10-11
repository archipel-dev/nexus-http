//! Response pane: status line, Preview (pretty + JSONPath filter), Raw,
//! Headers, Cookies, Timeline and Tests tabs, and response history.

use crate::state::{NexusState, Outcome, ResponseTab};
use crate::theme::{Palette, mono, semibold};
use crate::widgets::{
    self, Icon, Syntax, caption, code_view, field_frame, pill, shortcut, tab_strip,
};
use egui::{Align, CornerRadius, Layout, RichText, Sense, Ui, vec2};
use nexus_http_core::assertions::AssertionResult;
use nexus_http_core::model::{Id, Protocol};
use nexus_http_core::response::{BodyKind, Response, TimelineKind, format_duration, format_size};

#[derive(Default)]
pub struct ResponseUi {
    pub filter: String,
    /// Index into the request's history (0 = newest) when browsing old responses.
    pub history_index: Option<usize>,
    last_request: Option<Id>,
}

pub fn show(ui: &mut Ui, state: &mut NexusState, id: &Id, local: &mut ResponseUi) {
    let pal = Palette::of(ui);
    let protocol = state
        .workspace
        .find_request(id)
        .map(|(_, r)| r.protocol)
        .unwrap_or_default();
    if !protocol.is_http() {
        stream_view(ui, state, id, local, protocol, &pal);
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

    // Header row (same height as the URL bar so both panes line up).
    ui.add_space(10.0);
    let header = ui.horizontal(|ui| {
        ui.set_min_height(36.0);
        ui.add_space(12.0);
        if sending {
            ui.add(egui::Spinner::new().size(14.0).color(pal.accent));
            let elapsed = state
                .sending_elapsed()
                .map(|d| d.as_secs_f64() * 1000.0)
                .unwrap_or(0.0);
            ui.label(
                RichText::new(format!("Sending…  {}", format_duration(elapsed))).color(pal.weak),
            );
            return;
        }
        match &outcome {
            Some(Outcome::Response(r, _)) => {
                let label = format!("{} {}", r.status, r.reason);
                pill(ui, label.trim(), pal.status(r.status), true);
                ui.add_space(4.0);
                pill(ui, &format_duration(r.timings.total_ms), pal.weak, false).on_hover_text(
                    format!(
                        "Prepare {}\nWaiting {}\nDownload {}",
                        format_duration(r.timings.prepare_ms),
                        format_duration(r.timings.wait_ms),
                        format_duration(r.timings.download_ms)
                    ),
                );
                pill(ui, &format_size(r.body.len() as u64), pal.weak, false);
                if r.redirects > 0 {
                    pill(
                        ui,
                        &format!(
                            "{} redirect{}",
                            r.redirects,
                            if r.redirects == 1 { "" } else { "s" }
                        ),
                        pal.purple,
                        false,
                    );
                }
                if r.truncated {
                    pill(ui, "truncated", pal.warning, false);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.add_space(10.0);
                    history_menu(ui, state, id, local, &pal);
                });
            }
            Some(Outcome::Error(_)) => {
                pill(ui, "Error", pal.error, true);
            }
            None => {}
        }
    });
    if sending {
        widgets::busy_bar(ui, header.response.rect, pal.accent);
        ui.ctx().request_repaint();
    }
    ui.add_space(4.0);

    let Some(outcome) = outcome else {
        if !sending {
            idle(ui, &pal);
        }
        return;
    };
    let (response, checks) = match outcome {
        Outcome::Error(message) => {
            error_card(ui, &pal, &message);
            return;
        }
        Outcome::Response(r, checks) => (r, checks),
    };

    // Tabs
    let cookies = response.headers_named("set-cookie").count();
    let passed = checks.iter().filter(|c| c.passed).count();
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        tab_strip(
            ui,
            &mut state.response_tab,
            &ResponseTab::ALL,
            |t| t.label().to_owned(),
            |t| match t {
                ResponseTab::Headers => Some(response.headers.len().to_string()),
                ResponseTab::Cookies if cookies > 0 => Some(cookies.to_string()),
                ResponseTab::Tests if !checks.is_empty() => {
                    Some(format!("{passed}/{}", checks.len()))
                }
                _ => None,
            },
        );
    });
    ui.painter()
        .hline(ui.max_rect().x_range(), ui.cursor().top(), pal.hairline());
    ui.add_space(6.0);

    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| match state.response_tab {
            ResponseTab::Preview => preview(ui, &response, local, &pal),
            ResponseTab::Raw => code_view(ui, &response.body_text(), Syntax::Plain, "raw"),
            ResponseTab::Headers => headers(ui, &response, &pal),
            ResponseTab::Cookies => cookies_tab(ui, &response, state, &pal),
            ResponseTab::Timeline => timeline(ui, &response, &pal),
            ResponseTab::Tests => tests(ui, &checks, &pal),
        });
}

fn idle(ui: &mut Ui, pal: &Palette) {
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() / 3.5).max(16.0));
        ui.label(
            RichText::new("No response yet")
                .size(16.0)
                .color(pal.strong),
        );
        ui.add_space(14.0);
        for (label, keys) in [
            ("Send request", &["Ctrl", "Enter"][..]),
            ("Focus URL", &["Ctrl", "L"]),
            ("Switch request", &["Ctrl", "P"]),
            ("Manage environments", &["Ctrl", "E"]),
            ("New request", &["Ctrl", "N"]),
        ] {
            ui.horizontal(|ui| {
                let w = 290.0;
                ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
                ui.allocate_ui_with_layout(
                    vec2(w, 24.0),
                    Layout::left_to_right(Align::Center),
                    |ui| {
                        let label_w = ui.label(RichText::new(label).color(pal.weak)).rect.width();
                        ui.add_space(
                            (w - label_w - widgets::shortcut_width(ui, keys) - 8.0).max(0.0),
                        );
                        shortcut(ui, keys);
                    },
                );
            });
        }
    });
}

fn error_card(ui: &mut Ui, pal: &Palette, message: &str) {
    ui.add_space(12.0);
    egui::Frame::new()
        .fill(pal.error.gamma_multiply(0.08))
        .stroke(egui::Stroke::new(1.0_f32, pal.error.gamma_multiply(0.5)))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(egui::Margin::same(14))
        .outer_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(
                RichText::new("The request could not be completed")
                    .font(semibold(ui, 14.0))
                    .color(pal.error),
            );
            ui.add_space(6.0);
            ui.add(
                egui::Label::new(RichText::new(message).font(mono(12.5)).color(pal.text)).wrap(),
            );
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
            ui.label(RichText::new(hint).color(pal.weak));
        });
}

fn history_menu(ui: &mut Ui, state: &NexusState, id: &Id, local: &mut ResponseUi, pal: &Palette) {
    let count = state.history.for_request(id).count();
    if count == 0 {
        return;
    }
    let label = match local.history_index {
        None | Some(0) => "Latest".to_owned(),
        Some(i) => format!("{} of {count}", i + 1),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(label, egui::FontId::proportional(12.0), pal.weak);
    let (rect, resp) = ui.allocate_exact_size(vec2(galley.size().x + 30.0, 24.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), pal.hover);
    }
    ui.painter().galley(
        egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        pal.weak,
    );
    widgets::paint_icon(
        ui.painter(),
        egui::Rect::from_center_size(
            egui::pos2(rect.right() - 11.0, rect.center().y),
            vec2(10.0, 10.0),
        ),
        Icon::Chevron(true),
        pal.weak,
    );
    let resp = resp.on_hover_text("Response history");
    egui::Popup::menu(&resp).width(240.0).show(|ui| {
        for (i, r) in state.history.for_request(id).enumerate() {
            let when = relative_time(r.started_at);
            let text = RichText::new(format!(
                "{}  {}  ·  {}  ·  {when}",
                r.status,
                r.reason,
                format_duration(r.timings.total_ms)
            ))
            .size(12.0)
            .color(pal.status(r.status));
            if ui
                .selectable_label(local.history_index.unwrap_or(0) == i, text)
                .clicked()
            {
                local.history_index = (i > 0).then_some(i);
            }
        }
    });
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

fn preview(ui: &mut Ui, response: &Response, local: &mut ResponseUi, pal: &Palette) {
    let kind = response.body_kind();
    let filter_h = if kind == BodyKind::Json { 40.0 } else { 0.0 };
    let body_h = ui.available_height() - filter_h;
    ui.allocate_ui(vec2(ui.available_width(), body_h), |ui| match kind {
        BodyKind::Empty => widgets::empty_state(ui, "Empty body", "The response has no content."),
        BodyKind::Json => {
            match nexus_http_core::filter::filter_json_text(&response.body_text(), &local.filter) {
                Ok(text) => code_view(ui, &text, Syntax::Json, "pretty"),
                Err(e) => {
                    ui.label(RichText::new(e).color(pal.warning));
                }
            }
        }
        BodyKind::Image => widgets::empty_state(
            ui,
            "Image response",
            &format!(
                "{} · {}",
                response.content_type().unwrap_or("image"),
                format_size(response.body.len() as u64)
            ),
        ),
        BodyKind::Binary => widgets::empty_state(
            ui,
            "Binary response",
            &format!(
                "{} of {}",
                format_size(response.body.len() as u64),
                response.content_type().unwrap_or("unknown type")
            ),
        ),
        _ => code_view(ui, &response.body_text(), Syntax::Plain, "pretty-text"),
    });
    if kind == BodyKind::Json {
        ui.add_space(6.0);
        field_frame(pal).show(ui, |ui| {
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                widgets::paint_icon(ui.painter(), r, Icon::Search, pal.weak);
                ui.add(
                    egui::TextEdit::singleline(&mut local.filter)
                        .frame(false)
                        .font(mono(12.5))
                        .hint_text("Filter with JSONPath, e.g. $.items[*].name")
                        .desired_width(ui.available_width()),
                );
            });
        });
    }
}

fn headers(ui: &mut Ui, response: &Response, pal: &Palette) {
    egui::ScrollArea::vertical()
        .id_salt("resp-headers")
        .show(ui, |ui| {
            egui::Grid::new("resp-headers-grid")
                .num_columns(2)
                .striped(true)
                .spacing(vec2(18.0, 6.0))
                .show(ui, |ui| {
                    ui.label(RichText::new("Name").size(11.5).color(pal.weak));
                    ui.label(RichText::new("Value").size(11.5).color(pal.weak));
                    ui.end_row();
                    for (name, value) in &response.headers {
                        ui.add(
                            egui::Label::new(
                                RichText::new(name).font(mono(12.5)).color(pal.syntax_key),
                            )
                            .selectable(true),
                        );
                        ui.add(
                            egui::Label::new(RichText::new(value).font(mono(12.5)))
                                .selectable(true)
                                .wrap(),
                        );
                        ui.end_row();
                    }
                });
        });
}

fn cookies_tab(ui: &mut Ui, response: &Response, state: &mut NexusState, pal: &Palette) {
    let set: Vec<&str> = response.headers_named("set-cookie").collect();
    if set.is_empty() {
        caption(ui, "This response set no cookies");
    } else {
        caption(ui, "Set by this response");
        ui.add_space(4.0);
        for raw in set {
            ui.add(
                egui::Label::new(RichText::new(raw).font(mono(12.5)))
                    .selectable(true)
                    .wrap(),
            );
        }
    }
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        caption(
            ui,
            &format!("Workspace cookie jar · {}", state.jar.cookies.len()),
        );
        if !state.jar.cookies.is_empty() && ui.small_button("Clear all").clicked() {
            state.jar.clear();
        }
    });
    ui.add_space(4.0);
    egui::Grid::new("jar")
        .num_columns(4)
        .striped(true)
        .spacing(vec2(16.0, 5.0))
        .show(ui, |ui| {
            for h in ["Name", "Value", "Domain", "Path"] {
                ui.label(RichText::new(h).size(11.5).color(pal.weak));
            }
            ui.end_row();
            for c in &state.jar.cookies {
                ui.label(
                    RichText::new(&c.name)
                        .font(mono(12.5))
                        .color(pal.syntax_key),
                );
                ui.label(RichText::new(&c.value).font(mono(12.5)));
                ui.label(&c.domain);
                ui.label(&c.path);
                ui.end_row();
            }
        });
}

fn timeline(ui: &mut Ui, response: &Response, pal: &Palette) {
    egui::ScrollArea::both()
        .id_salt("timeline")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for entry in &response.timeline {
                let (prefix, color) = match entry.kind {
                    TimelineKind::Info => ("* ", pal.weak),
                    TimelineKind::RequestHeader => ("> ", pal.info),
                    TimelineKind::RequestBody => ("| ", pal.text),
                    TimelineKind::ResponseHeader => ("< ", pal.success),
                    TimelineKind::ResponseBody => ("| ", pal.text),
                    TimelineKind::Error => ("! ", pal.error),
                };
                for line in entry.text.lines() {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{prefix}{line}"))
                                .font(mono(12.0))
                                .color(color),
                        )
                        .selectable(true)
                        .extend(),
                    );
                }
            }
        });
}

fn tests(ui: &mut Ui, checks: &[AssertionResult], pal: &Palette) {
    if checks.is_empty() {
        widgets::empty_state(
            ui,
            "No assertions",
            "Add assertions in the request's Tests tab.",
        );
        return;
    }
    for c in checks {
        ui.horizontal(|ui| {
            let (icon, color) = if c.passed {
                (Icon::Check, pal.success)
            } else {
                (Icon::Close, pal.error)
            };
            let (r, _) = ui.allocate_exact_size(vec2(18.0, 18.0), Sense::hover());
            widgets::paint_icon(ui.painter(), r, icon, color);
            ui.label(&c.description);
            if !c.passed {
                ui.label(RichText::new(format!("got {}", c.actual)).color(pal.weak));
            }
        });
    }
}

fn stream_view(
    ui: &mut Ui,
    state: &mut NexusState,
    id: &Id,
    local: &mut ResponseUi,
    protocol: Protocol,
    pal: &Palette,
) {
    use nexus_http_core::stream::Direction;
    let connected = state.stream_connected(id);
    let events = state
        .streams
        .get(id)
        .map(|s| s.events.as_slice())
        .unwrap_or(&[]);
    ui.add_space(10.0);
    let mut clear = false;
    ui.horizontal(|ui| {
        ui.set_min_height(36.0);
        ui.add_space(12.0);
        if connected {
            pill(ui, "Connected", pal.success, true);
        } else if events.is_empty() {
            pill(ui, "Not connected", pal.weak, true);
        } else if events.iter().any(|e| e.direction == Direction::Error) {
            pill(ui, "Failed", pal.error, true);
        } else {
            pill(ui, "Closed", pal.weak, true);
        }
        let received = events
            .iter()
            .filter(|e| e.direction == Direction::Incoming)
            .count();
        let sent = events
            .iter()
            .filter(|e| e.direction == Direction::Outgoing)
            .count();
        pill(ui, &format!("{received} received"), pal.weak, false);
        if matches!(protocol, Protocol::WebSocket | Protocol::Grpc) {
            pill(ui, &format!("{sent} sent"), pal.weak, false);
        }
        if let Some(last) = events.last() {
            pill(ui, &format_duration(last.at_ms), pal.weak, false);
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(10.0);
            if !events.is_empty() && widgets::secondary_button(ui, "Clear").clicked() {
                clear = true;
            }
        });
    });
    if connected {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(100));
    }
    ui.add_space(4.0);
    ui.painter()
        .hline(ui.max_rect().x_range(), ui.cursor().top(), pal.hairline());
    ui.add_space(8.0);

    if events.is_empty() {
        let hint = match protocol {
            Protocol::WebSocket => "Connect, then send messages from the Message tab (Ctrl+Enter).",
            Protocol::Grpc => {
                "Pick a method, then Connect (Ctrl+Enter) to call it. Client-streaming calls take more messages from the Message tab."
            }
            _ => "Connect to stream events as they arrive.",
        };
        widgets::empty_state(ui, &format!("{} stream", protocol.label()), hint);
        return;
    }
    let filter = local.filter.to_lowercase();
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            let list_h = ui.available_height() - 42.0;
            egui::ScrollArea::vertical()
                .id_salt("stream-events")
                .auto_shrink([false, false])
                .max_height(list_h)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for event in events.iter().filter(|e| {
                        filter.is_empty()
                            || e.data.to_lowercase().contains(&filter)
                            || e.label.to_lowercase().contains(&filter)
                    }) {
                        let (icon_color, tint) = match event.direction {
                            Direction::Incoming => (pal.success, pal.success.gamma_multiply(0.06)),
                            Direction::Outgoing => (pal.info, pal.info.gamma_multiply(0.06)),
                            Direction::Info => (pal.weak, egui::Color32::TRANSPARENT),
                            Direction::Error => (pal.error, pal.error.gamma_multiply(0.08)),
                        };
                        egui::Frame::new()
                            .fill(tint)
                            .corner_radius(CornerRadius::same(6))
                            .inner_margin(egui::Margin::symmetric(8, 5))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal_top(|ui| {
                                    let (r, _) =
                                        ui.allocate_exact_size(vec2(14.0, 16.0), Sense::hover());
                                    let c = r.center();
                                    match event.direction {
                                        Direction::Incoming => arrow(ui, c, false, icon_color),
                                        Direction::Outgoing => arrow(ui, c, true, icon_color),
                                        _ => {
                                            ui.painter().circle_filled(c, 3.0, icon_color);
                                        }
                                    }
                                    ui.label(
                                        RichText::new(format!(
                                            "{:>7}",
                                            format_duration(event.at_ms)
                                        ))
                                        .font(mono(11.5))
                                        .color(pal.faint),
                                    );
                                    if event.label != "message" {
                                        pill(ui, &event.label, icon_color, false);
                                    }
                                    if let Some(eid) = &event.id {
                                        ui.label(
                                            RichText::new(format!("id {eid}"))
                                                .size(11.5)
                                                .color(pal.faint),
                                        );
                                    }
                                    let pretty =
                                        serde_json::from_str::<serde_json::Value>(&event.data)
                                            .ok()
                                            .filter(|v| v.is_object() || v.is_array())
                                            .and_then(|v| serde_json::to_string(&v).ok());
                                    let text = pretty.unwrap_or_else(|| event.data.clone());
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(text).font(mono(12.0)).color(
                                                if event.direction == Direction::Info {
                                                    pal.weak
                                                } else {
                                                    pal.text
                                                },
                                            ),
                                        )
                                        .wrap()
                                        .selectable(true),
                                    );
                                });
                            });
                        ui.add_space(3.0);
                    }
                });
            ui.add_space(6.0);
            field_frame(pal).show(ui, |ui| {
                ui.horizontal(|ui| {
                    let (r, _) = ui.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    widgets::paint_icon(ui.painter(), r, Icon::Search, pal.weak);
                    ui.add(
                        egui::TextEdit::singleline(&mut local.filter)
                            .frame(false)
                            .hint_text("Filter events")
                            .desired_width(ui.available_width()),
                    );
                });
            });
        });
    if clear && let Some(s) = state.streams.get_mut(id) {
        s.events.clear();
    }
}

fn arrow(ui: &Ui, c: egui::Pos2, up: bool, color: egui::Color32) {
    let s = egui::Stroke::new(1.6_f32, color);
    let (tip, tail, wing) = if up {
        (-5.0, 5.0, -1.5)
    } else {
        (5.0, -5.0, 1.5)
    };
    ui.painter()
        .line_segment([egui::pos2(c.x, c.y + tail), egui::pos2(c.x, c.y + tip)], s);
    ui.painter().line_segment(
        [
            egui::pos2(c.x - 3.5, c.y + wing),
            egui::pos2(c.x, c.y + tip),
        ],
        s,
    );
    ui.painter().line_segment(
        [
            egui::pos2(c.x + 3.5, c.y + wing),
            egui::pos2(c.x, c.y + tip),
        ],
        s,
    );
}
