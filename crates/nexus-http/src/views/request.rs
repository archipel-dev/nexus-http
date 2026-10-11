//! Request editor: URL bar with method picker and Send, and the Params, Body,
//! Auth, Headers, Tests and Settings tabs.
//!
//! Built only from archipel-kit. The tabs are views of the same request, so they are a
//! `SegmentedControl` like the response pane's. Params, headers and form fields are the kit
//! `KeyValueTable`; every value is a `{{variable}}` field (see `complete`); bodies are the kit
//! `CodeEditor`.

use crate::complete::{self, Field, Variables};
use crate::state::{NexusState, RequestTab, SECRET_KEYS_ID};
use archipel_kit::behaviour::WidgetExt;
use archipel_kit::components::code::{Highlighter, Json, PlainText, TemplateHighlighter, Xml};
use archipel_kit::components::{
    Banner, Button, Change, Checkbox, CodeEditor, Dropdown, EmptyState, IconButton, KeyValueEntry,
    KeyValueTable, MenuItem, SectionHeader, SegmentedControl, Severity, TextField, Tooltip,
};
use archipel_kit::parts::Popover;
use archipel_kit::tokens::size;
use archipel_kit::{Glyph, Theme, egui};
use egui::{Id, Ui};
use nexus_http_core::assertions::{Assertion, Operator, Source};
use nexus_http_core::model::Id as RequestId;
use nexus_http_core::model::{
    ApiKeyLocation, Auth, Body, KeyValue, Method, MultipartField, MultipartValue, OAuth2Config,
    OAuth2Grant, Protocol, Request,
};
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::Arc;

pub const URL_FIELD_ID: &str = "nexus.url";

/// Actions the request view asks the app to perform.
#[derive(Default)]
pub struct RequestActions {
    pub send: bool,
    pub connect: bool,
    pub disconnect: bool,
    pub send_message: bool,
    pub cancel: bool,
    pub generate_code: bool,
    pub fetch_token: bool,
    pub load_grpc_schema: bool,
}

pub fn show(ui: &mut Ui, state: &mut NexusState, id: &RequestId, actions: &mut RequestActions) {
    let secrets: Arc<HashSet<String>> = ui
        .data(|d| d.get_temp(Id::new(SECRET_KEYS_ID)))
        .unwrap_or_default();
    let vars = Variables::new(&variables_for(state, id), secrets);
    let sending = state.sending_id() == Some(id);
    let connected = state.stream_connected(id);
    let mut gql = match state.schemas.get(id) {
        Some(s) => GqlContext {
            schema: s.schema.clone(),
            loading: s.loading(),
            error: s.error.clone(),
            fetch: false,
        },
        None => GqlContext::default(),
    };
    let grpc_schema = state
        .grpc_schemas
        .get(id)
        .map(|s| (s.methods.clone(), s.loading(), s.error.clone()));
    let tab = &mut state.request_tab;
    let mut current_tab = *tab;
    let Some(request) = state.request_mut(id) else {
        return;
    };
    let mut changed = false;

    // ---------------------------------------------------------------- URL bar
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        ui.add_space(size::CONTENT_INSET);
        changed |= method_picker(ui, &mut request.method, &mut request.protocol);
        let send_w = if request.protocol.is_http() {
            80.0
        } else {
            104.0
        };
        let url_w = ui.available_width()
            - send_w
            - size::CONTROL_HEIGHT
            - 2.0 * ui.spacing().item_spacing.x
            - size::CONTENT_INSET;
        let hint = match request.protocol {
            Protocol::Http => "Enter a URL, paste a curl command, or use {{base_url}}",
            Protocol::WebSocket => "ws://host/socket or wss://…",
            Protocol::Sse => "https://host/events",
            Protocol::Grpc => "grpc://host:50051 or grpcs://host:443",
        };
        let mut url = Field::new(&mut request.url, Id::new(URL_FIELD_ID), "URL");
        url.placeholder = hint;
        url.width = Some(url_w.max(120.0));
        url.monospace = true;
        let edit = complete::show(ui, &vars, url);
        if edit.response.changed() {
            changed = true;
            sync_params_from_url(request);
        }
        if edit.submitted {
            actions.send = true;
        }
        if ui
            .add(IconButton::new(Glyph::Code, "Generate code").with(Tooltip::new("Generate code")))
            .clicked()
        {
            actions.generate_code = true;
        }
        if !request.protocol.is_http() {
            if connected {
                if ui
                    .add(Button::secondary("Disconnect").min_width(send_w))
                    .clicked()
                {
                    actions.disconnect = true;
                }
            } else if ui
                .add(
                    Button::primary("Connect")
                        .min_width(send_w)
                        .with(Tooltip::new("Connect").shortcut("Ctrl+Enter")),
                )
                .clicked()
            {
                actions.connect = true;
            }
        } else if sending {
            if ui
                .add(Button::secondary("Cancel").min_width(send_w))
                .clicked()
            {
                actions.cancel = true;
            }
        } else if ui
            .add(
                Button::primary("Send")
                    .min_width(send_w)
                    .with(Tooltip::new("Send").shortcut("Ctrl+Enter")),
            )
            .clicked()
        {
            actions.send = true;
        }
    });
    if request.protocol == Protocol::Grpc {
        changed |= grpc_bar(ui, request, &vars, grpc_schema.as_ref(), actions);
    }
    ui.add_space(8.0);

    // ---------------------------------------------------------------- tabs
    ui.horizontal(|ui| {
        ui.add_space(size::CONTENT_INSET);
        let counts = TabCounts::of(request);
        let options = RequestTab::ALL.map(|t| {
            let label = match t {
                RequestTab::Body
                    if matches!(request.protocol, Protocol::WebSocket | Protocol::Grpc) =>
                {
                    "Message".to_owned()
                }
                RequestTab::Body => match &request.body {
                    Body::None => "Body".to_owned(),
                    other => other.label().to_owned(),
                },
                RequestTab::Auth => match &request.auth {
                    Auth::None | Auth::Inherit => "Auth".to_owned(),
                    other => auth_label(other).to_owned(),
                },
                other => other.label().to_owned(),
            };
            let label = match counts.badge(t) {
                Some(n) => format!("{label} ({n})"),
                None => label,
            };
            (t, label)
        });
        ui.add(SegmentedControl::new(&mut current_tab, options));
    });
    ui.add_space(8.0);

    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(size::CONTENT_INSET as i8, 0))
        .show(ui, |ui| {
            ui.set_min_height(ui.available_height());
            match current_tab {
                RequestTab::Params => {
                    ui.add(SectionHeader::new("Query parameters"));
                    egui::ScrollArea::vertical()
                        .id_salt("params")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            changed |= key_values(
                                ui,
                                &vars,
                                "params",
                                "Add parameter",
                                &mut request.params,
                            );
                        });
                }
                RequestTab::Headers => {
                    ui.add(SectionHeader::new("Headers"));
                    egui::ScrollArea::vertical()
                        .id_salt("headers")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            changed |= key_values(
                                ui,
                                &vars,
                                "headers",
                                "Add header",
                                &mut request.headers,
                            );
                        });
                }
                RequestTab::Body
                    if matches!(request.protocol, Protocol::WebSocket | Protocol::Grpc) =>
                {
                    changed |= message_tab(ui, &mut request.body, &vars, connected, actions);
                }
                RequestTab::Body => changed |= body_tab(ui, &mut request.body, &vars, &mut gql),
                RequestTab::Auth => changed |= auth_tab(ui, &mut request.auth, &vars, actions),
                RequestTab::Tests => changed |= tests_tab(ui, &mut request.assertions),
                RequestTab::Settings => changed |= settings_tab(ui, request),
            }
        });

    state.request_tab = current_tab;
    if gql.fetch {
        state.fetch_schema(id);
    }
    if changed {
        state.touch();
    }
}

/// Variables visible to request `id`, for highlighting and autocomplete.
pub fn variables_for(state: &NexusState, id: &RequestId) -> Map<String, Value> {
    let ws = &state.workspace;
    let folders = ws.find_request(id).map(|(f, _)| f).unwrap_or_default();
    nexus_http_core::template::merged_variables(
        &ws.base_environment,
        ws.active_environment(),
        &folders,
    )
}

/// GraphQL schema state handed to the body tab.
#[derive(Default)]
struct GqlContext {
    schema: Option<std::sync::Arc<nexus_http_core::graphql::Schema>>,
    loading: bool,
    error: Option<String>,
    fetch: bool,
}

struct TabCounts {
    params: usize,
    headers: usize,
    tests: usize,
}

impl TabCounts {
    fn of(r: &Request) -> Self {
        Self {
            params: r.params.iter().filter(|p| p.enabled).count(),
            headers: r.headers.iter().filter(|p| p.enabled).count(),
            tests: r.assertions.iter().filter(|a| a.enabled).count(),
        }
    }

    fn badge(&self, tab: RequestTab) -> Option<String> {
        let n = match tab {
            RequestTab::Params => self.params,
            RequestTab::Headers => self.headers,
            RequestTab::Tests => self.tests,
            _ => 0,
        };
        (n > 0).then(|| n.to_string())
    }
}

type GrpcSchemaView = (Vec<nexus_http_core::grpc::MethodInfo>, bool, Option<String>);

/// The gRPC row under the URL bar: schema source (`.proto` file or reflection)
/// and the method to call.
fn grpc_bar(
    ui: &mut Ui,
    request: &mut Request,
    vars: &Variables,
    schema: Option<&GrpcSchemaView>,
    actions: &mut RequestActions,
) -> bool {
    let colors = Theme::colors(ui.ctx());
    let mut changed = false;
    let loading = schema.is_some_and(|s| s.1);
    let methods = schema.map(|s| s.0.as_slice()).unwrap_or(&[]);
    let open_id = Id::new("nexus.grpc.methods");
    let mut open: bool = ui.data(|d| d.get_temp(open_id)).unwrap_or(false);
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        ui.add_space(size::CONTENT_INSET);
        let half = (ui.available_width() - 120.0 - size::CONTENT_INSET) / 2.0;
        let mut proto = Field::new(&mut request.grpc.proto_file, Id::new("nexus.grpc.proto"), "Proto file");
        proto.placeholder = ".proto file (empty: server reflection)";
        proto.width = Some(half);
        changed |= complete::show(ui, vars, proto).response.changed();
        let mut method = Field::new(&mut request.grpc.method, Id::new("nexus.grpc.method"), "gRPC method");
        method.placeholder = "package.Service/Method";
        method.width = Some(half);
        changed |= complete::show(ui, vars, method).response.changed();
        let label = if loading { "Loading…" } else { "Methods" };
        let button = ui.add(Button::secondary(label).min_width(100.0));
        if button.clicked() {
            if !loading {
                actions.load_grpc_schema = true;
            }
            open = !open && !methods.is_empty();
        }
        let mut close = false;
        if !methods.is_empty() {
            Popover::below(open_id.with("popover"), &button)
                .width(320.0)
                .show(ui.ctx(), &mut open, |ui| {
                    for m in methods {
                        let text = format!("{}  ·  {}", m.name, m.kind());
                        let item = MenuItem::new(text).checked(request.grpc.method == m.name);
                        if ui.add(item).clicked() {
                            request.grpc.method = m.name.clone();
                            if matches!(&request.body, Body::None)
                                || matches!(&request.body, Body::Json { text } if text.trim().is_empty())
                            {
                                request.body = Body::Json {
                                    text: m.template.clone(),
                                };
                            }
                            changed = true;
                            close = true;
                        }
                    }
                });
        }
        open &= !close;
    });
    ui.data_mut(|d| d.insert_temp(open_id, open));
    if let Some((_, _, Some(error))) = schema {
        ui.horizontal(|ui| {
            ui.add_space(size::CONTENT_INSET);
            ui.label(egui::RichText::new(error).color(colors.error));
        });
    }
    changed
}

/// The URL bar's picker: HTTP methods and the other protocols in one list.
fn method_picker(ui: &mut Ui, method: &mut Method, protocol: &mut Protocol) -> bool {
    const OTHERS: [(&str, Protocol); 3] = [
        ("WebSocket", Protocol::WebSocket),
        ("SSE", Protocol::Sse),
        ("gRPC", Protocol::Grpc),
    ];
    let mut verb = match *protocol {
        Protocol::Http => method.as_str().to_owned(),
        other => OTHERS
            .iter()
            .find(|(_, p)| *p == other)
            .map_or_else(String::new, |(l, _)| (*l).to_owned()),
    };
    let before = verb.clone();
    let mut options: Vec<String> = Method::COMMON
        .iter()
        .map(|m| m.as_str().to_owned())
        .collect();
    if protocol.is_http() && !options.contains(&verb) {
        options.push(verb.clone());
    }
    options.extend(OTHERS.iter().map(|(l, _)| (*l).to_owned()));
    ui.add(
        Dropdown::new("nexus.method", &mut verb, options)
            .label("Request method")
            .width(112.0),
    );
    if verb == before {
        return false;
    }
    match OTHERS.iter().find(|(l, _)| *l == verb) {
        Some((_, p)) => {
            *protocol = *p;
            *method = Method::Get;
        }
        None => {
            *protocol = Protocol::Http;
            *method = Method::from(verb);
        }
    }
    true
}

/// Moves `?a=b` typed into the URL into the params table.
fn sync_params_from_url(request: &mut Request) {
    let Some((base, query)) = request.url.split_once('?') else {
        return;
    };
    if query.contains("{{") && !query.contains("}}") {
        return; // still typing a template
    }
    let base = base.to_owned();
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        request.params.push(KeyValue::new(name, value));
    }
    request.url = base;
}

// ------------------------------------------------------------- key/value tables

/// A kit `KeyValueTable` over `items`, restricted to those `member` accepts.
///
/// `model::KeyValue` lives in the headless core, which does not depend on the kit, so the
/// kit's `KeyValueRow` cannot be implemented for it here; the table edits `KeyValueEntry`
/// copies and the result is written back row by row (fields the table does not show, such as
/// a description, stay untouched).
#[allow(clippy::too_many_arguments)]
fn edit_rows<T>(
    ui: &mut Ui,
    vars: &Variables,
    salt: &str,
    key_hint: &str,
    value_hint: &str,
    items: &mut Vec<T>,
    member: impl Fn(&T) -> bool,
    read: impl Fn(&T) -> KeyValueEntry,
    write: impl Fn(&mut T, KeyValueEntry),
    make: impl Fn(KeyValueEntry) -> T,
) -> bool {
    let mut idx: Vec<usize> = (0..items.len()).filter(|&i| member(&items[i])).collect();
    let mut entries: Vec<KeyValueEntry> = idx.iter().map(|&i| read(&items[i])).collect();
    let out = KeyValueTable::new(salt, &mut entries)
        .key_placeholder(key_hint)
        .value_placeholder(value_hint)
        .value_cell(|ui, cell| {
            let mut field = Field::new(cell.text, Id::new((salt, cell.name)), cell.name);
            field.placeholder = cell.placeholder;
            field.width = Some(cell.width);
            complete::show(ui, vars, field).response
        })
        .show(ui);
    if !out.changed() {
        return false;
    }
    if let Some(removed) = out.changes.iter().find_map(|c| match c {
        Change::Removed(row) => Some(*row),
        _ => None,
    }) {
        let at = idx.remove(removed);
        items.remove(at);
        for i in &mut idx {
            if *i > at {
                *i -= 1;
            }
        }
    }
    for (row, entry) in entries.into_iter().enumerate() {
        match idx.get(row) {
            Some(&i) => write(&mut items[i], entry),
            None => items.push(make(entry)),
        }
    }
    true
}

fn key_values(
    ui: &mut Ui,
    vars: &Variables,
    salt: &str,
    key_hint: &str,
    rows: &mut Vec<KeyValue>,
) -> bool {
    edit_rows(
        ui,
        vars,
        salt,
        key_hint,
        "value",
        rows,
        |_| true,
        |kv| KeyValueEntry {
            enabled: kv.enabled,
            key: kv.name.clone(),
            value: kv.value.clone(),
        },
        |kv, e| {
            kv.enabled = e.enabled;
            kv.name = e.key;
            kv.value = e.value;
        },
        |e| KeyValue {
            enabled: e.enabled,
            ..KeyValue::new(e.key, e.value)
        },
    )
}

// -------------------------------------------------------------------- body

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyKind {
    None,
    Json,
    Xml,
    Text,
    Form,
    Multipart,
    Binary,
    GraphQl,
}

impl BodyKind {
    const ALL: [BodyKind; 8] = [
        BodyKind::Json,
        BodyKind::Xml,
        BodyKind::Text,
        BodyKind::Form,
        BodyKind::Multipart,
        BodyKind::GraphQl,
        BodyKind::Binary,
        BodyKind::None,
    ];

    fn of(body: &Body) -> Self {
        match body {
            Body::None => BodyKind::None,
            Body::Json { .. } => BodyKind::Json,
            Body::Xml { .. } => BodyKind::Xml,
            Body::Text { .. } => BodyKind::Text,
            Body::FormUrlEncoded { .. } => BodyKind::Form,
            Body::Multipart { .. } => BodyKind::Multipart,
            Body::Binary { .. } => BodyKind::Binary,
            Body::GraphQl { .. } => BodyKind::GraphQl,
        }
    }

    fn label(self) -> &'static str {
        match self {
            BodyKind::None => "No Body",
            BodyKind::Json => "JSON",
            BodyKind::Xml => "XML",
            BodyKind::Text => "Plain Text",
            BodyKind::Form => "Form URL Encoded",
            BodyKind::Multipart => "Multipart Form",
            BodyKind::Binary => "Binary File",
            BodyKind::GraphQl => "GraphQL",
        }
    }

    /// Converts the body, carrying text over where it makes sense.
    fn convert(self, body: &Body) -> Body {
        let text = match body {
            Body::Json { text } | Body::Xml { text } | Body::Text { text, .. } => text.clone(),
            _ => String::new(),
        };
        let fields = match body {
            Body::FormUrlEncoded { fields } => fields.clone(),
            Body::Multipart { fields } => fields
                .iter()
                .filter_map(|f| match &f.value {
                    MultipartValue::Text { value } => Some(KeyValue {
                        enabled: f.enabled,
                        ..KeyValue::new(&f.name, value)
                    }),
                    MultipartValue::File { .. } => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        match self {
            BodyKind::None => Body::None,
            BodyKind::Json => Body::Json { text },
            BodyKind::Xml => Body::Xml { text },
            BodyKind::Text => Body::Text {
                text,
                content_type: None,
            },
            BodyKind::Form => Body::FormUrlEncoded { fields },
            BodyKind::Multipart => Body::Multipart {
                fields: fields
                    .into_iter()
                    .map(|kv| MultipartField {
                        name: kv.name,
                        value: MultipartValue::Text { value: kv.value },
                        enabled: kv.enabled,
                    })
                    .collect(),
            },
            BodyKind::Binary => Body::Binary {
                path: String::new(),
            },
            BodyKind::GraphQl => Body::GraphQl {
                query: if text.is_empty() {
                    "query {\n  \n}".into()
                } else {
                    text
                },
                variables: String::new(),
                operation_name: None,
            },
        }
    }
}

impl std::fmt::Display for BodyKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// A code editor for a request body: `{{variables}}` are highlighted over the language.
fn editor(
    ui: &mut Ui,
    text: &mut String,
    lang: &dyn Fn(&mut Ui, &mut String, &Variables, &str, f32) -> bool,
    vars: &Variables,
    name: &str,
) -> bool {
    let height = (ui.available_height() - 4.0).max(120.0);
    lang(ui, text, vars, name, height)
}

fn code_editor(
    ui: &mut Ui,
    text: &mut String,
    vars: &Variables,
    name: &str,
    height: f32,
    inner: impl Highlighter,
) -> bool {
    let highlighter = TemplateHighlighter::new(|n: &str| vars.token(n)).inner(inner);
    ui.add(
        CodeEditor::new(text)
            .label(name)
            .highlighter(&highlighter)
            .line_numbers(true)
            .id(ui.id().with(name))
            .height(height),
    )
    .changed()
}

fn json_editor(ui: &mut Ui, text: &mut String, vars: &Variables, name: &str, h: f32) -> bool {
    code_editor(ui, text, vars, name, h, Json)
}

fn xml_editor(ui: &mut Ui, text: &mut String, vars: &Variables, name: &str, h: f32) -> bool {
    code_editor(ui, text, vars, name, h, Xml)
}

fn plain_editor(ui: &mut Ui, text: &mut String, vars: &Variables, name: &str, h: f32) -> bool {
    code_editor(ui, text, vars, name, h, PlainText)
}

fn body_tab(ui: &mut Ui, body: &mut Body, vars: &Variables, gql: &mut GqlContext) -> bool {
    let mut changed = false;
    let mut kind = BodyKind::of(body);
    ui.horizontal(|ui| {
        let before = kind;
        ui.add(
            Dropdown::new("nexus.body-kind", &mut kind, BodyKind::ALL)
                .label("Body type")
                .width(170.0),
        );
        if kind != before {
            *body = kind.convert(body);
            changed = true;
        }
        if let Body::Json { text } = body
            && ui
                .add(Button::ghost("Beautify").with(Tooltip::new("Re-indent the JSON")))
                .clicked()
            && let Ok(v) = serde_json::from_str::<Value>(text)
            && let Ok(pretty) = serde_json::to_string_pretty(&v)
        {
            *text = pretty;
            changed = true;
        }
    });
    ui.add_space(8.0);
    match body {
        Body::None => {
            EmptyState::new("No body")
                .hint("Pick a body type above to send a payload.")
                .show(ui);
        }
        Body::Json { text } => {
            if !text.trim().is_empty()
                && !nexus_http_core::template::has_templates(text)
                && let Err(e) = serde_json::from_str::<Value>(text)
            {
                Banner::new(Severity::Warning, format!("Invalid JSON: {e}")).show(ui);
                ui.add_space(4.0);
            }
            changed |= editor(ui, text, &json_editor, vars, "JSON body");
        }
        Body::Xml { text } => changed |= editor(ui, text, &xml_editor, vars, "XML body"),
        Body::Text { text, content_type } => {
            let mut ct = content_type.clone().unwrap_or_default();
            if ui
                .add(
                    TextField::new(&mut ct)
                        .label("Content-Type")
                        .placeholder("text/plain")
                        .width(260.0),
                )
                .changed()
            {
                *content_type = (!ct.trim().is_empty()).then_some(ct);
                changed = true;
            }
            ui.add_space(4.0);
            changed |= editor(ui, text, &plain_editor, vars, "Text body");
        }
        Body::FormUrlEncoded { fields } => {
            egui::ScrollArea::vertical()
                .id_salt("form")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    changed |= key_values(ui, vars, "form", "Add field", fields);
                });
        }
        Body::Multipart { fields } => {
            egui::ScrollArea::vertical()
                .id_salt("multipart")
                .auto_shrink([false, false])
                .show(ui, |ui| changed |= multipart(ui, fields, vars));
        }
        Body::Binary { path } => {
            let mut field = Field::new(
                path,
                ui.id().with("binpath"),
                "File sent as the request body",
            );
            field.placeholder = "path/to/file.bin (absolute or relative to the project)";
            field.labelled = true;
            field.monospace = true;
            changed |= complete::show(ui, vars, field).response.changed();
        }
        Body::GraphQl {
            query,
            variables,
            operation_name: _,
        } => {
            changed |= crate::graphql_ui::body(
                ui,
                query,
                variables,
                vars,
                gql.schema.as_deref(),
                gql.loading,
                gql.error.as_deref(),
                &mut gql.fetch,
            );
        }
    }
    changed
}

/// WebSocket message composer.
fn message_tab(
    ui: &mut Ui,
    body: &mut Body,
    vars: &Variables,
    connected: bool,
    actions: &mut RequestActions,
) -> bool {
    let mut changed = false;
    if !matches!(body, Body::Json { .. } | Body::Text { .. }) {
        *body = Body::Json {
            text: String::new(),
        };
        changed = true;
    }
    ui.horizontal(|ui| {
        let mut is_json = matches!(body, Body::Json { .. });
        let before = is_json;
        ui.add(SegmentedControl::new(
            &mut is_json,
            [(true, "JSON"), (false, "Text")],
        ));
        if is_json != before {
            *body = if is_json {
                BodyKind::Json.convert(body)
            } else {
                BodyKind::Text.convert(body)
            };
            changed = true;
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            let tip = if connected {
                "Send message"
            } else {
                "Connect first"
            };
            if ui
                .add(
                    Button::primary("Send message")
                        .enabled(connected)
                        .with(Tooltip::new(tip).shortcut("Ctrl+Enter")),
                )
                .clicked()
            {
                actions.send_message = true;
            }
        });
    });
    ui.add_space(8.0);
    match body {
        Body::Json { text } => changed |= editor(ui, text, &json_editor, vars, "JSON message"),
        Body::Text { text, .. } => changed |= editor(ui, text, &plain_editor, vars, "Text message"),
        _ => {}
    }
    changed
}

fn multipart(ui: &mut Ui, fields: &mut Vec<MultipartField>, vars: &Variables) -> bool {
    let mut changed = false;
    ui.add(SectionHeader::new("Text fields"));
    changed |= edit_rows(
        ui,
        vars,
        "mp-text",
        "Add text field",
        "value",
        fields,
        |f| matches!(f.value, MultipartValue::Text { .. }),
        |f| KeyValueEntry {
            enabled: f.enabled,
            key: f.name.clone(),
            value: match &f.value {
                MultipartValue::Text { value } => value.clone(),
                MultipartValue::File { .. } => String::new(),
            },
        },
        |f, e| {
            f.enabled = e.enabled;
            f.name = e.key;
            f.value = MultipartValue::Text { value: e.value };
        },
        |e| MultipartField {
            name: e.key,
            value: MultipartValue::Text { value: e.value },
            enabled: e.enabled,
        },
    );
    ui.add(SectionHeader::new("Files"));
    changed |= edit_rows(
        ui,
        vars,
        "mp-file",
        "Add file field",
        "path/to/file",
        fields,
        |f| matches!(f.value, MultipartValue::File { .. }),
        |f| KeyValueEntry {
            enabled: f.enabled,
            key: f.name.clone(),
            value: match &f.value {
                MultipartValue::File { path, .. } => path.clone(),
                MultipartValue::Text { .. } => String::new(),
            },
        },
        |f, e| {
            f.enabled = e.enabled;
            f.name = e.key;
            if let MultipartValue::File { path, .. } = &mut f.value {
                *path = e.value;
            }
        },
        |e| MultipartField {
            name: e.key,
            value: MultipartValue::File {
                path: e.value,
                content_type: None,
            },
            enabled: e.enabled,
        },
    );
    changed
}

// -------------------------------------------------------------------- auth

/// A scheme label and a constructor for its default configuration.
type AuthChoice = (&'static str, fn() -> Auth);

const AUTH_KINDS: [AuthChoice; 8] = [
    ("No Auth", || Auth::None),
    ("Inherit", || Auth::Inherit),
    ("Basic", || Auth::Basic {
        username: String::new(),
        password: String::new(),
    }),
    ("Bearer", || Auth::Bearer {
        token: String::new(),
        prefix: "Bearer".into(),
    }),
    ("API Key", || Auth::ApiKey {
        key: "X-API-Key".into(),
        value: String::new(),
        location: ApiKeyLocation::Header,
    }),
    ("Digest", || Auth::Digest {
        username: String::new(),
        password: String::new(),
    }),
    ("OAuth 2.0", || {
        Auth::OAuth2(Box::new(OAuth2Config {
            header_prefix: "Bearer".into(),
            use_pkce: true,
            redirect_url: "http://127.0.0.1:8765/callback".into(),
            ..Default::default()
        }))
    }),
    ("AWS SigV4", || Auth::AwsSigV4 {
        access_key_id: String::new(),
        secret_access_key: String::new(),
        session_token: String::new(),
        region: "us-east-1".into(),
        service: String::new(),
    }),
];

fn auth_label(auth: &Auth) -> &'static str {
    let current = std::mem::discriminant(auth);
    AUTH_KINDS
        .iter()
        .find(|(_, make)| std::mem::discriminant(&make()) == current)
        .map_or("Auth", |(label, _)| label)
}

/// A labelled form field of the auth tab.
fn form_field(
    ui: &mut Ui,
    vars: &Variables,
    label: &str,
    value: &mut String,
    hint: &str,
    secret: bool,
) -> bool {
    let mut field = Field::new(value, ui.id().with(label), label);
    field.placeholder = hint;
    field.labelled = true;
    field.secret = secret;
    field.width = Some(ui.available_width().min(480.0));
    complete::show(ui, vars, field).response.changed()
}

fn auth_tab(ui: &mut Ui, auth: &mut Auth, vars: &Variables, actions: &mut RequestActions) -> bool {
    let colors = Theme::colors(ui.ctx());
    let mut changed = false;
    let mut label = auth_label(auth).to_owned();
    let before = label.clone();
    ui.add(
        Dropdown::new(
            "nexus.auth-kind",
            &mut label,
            AUTH_KINDS.iter().map(|(l, _)| (*l).to_owned()),
        )
        .label("Auth type")
        .width(170.0),
    );
    if label != before
        && let Some((_, make)) = AUTH_KINDS.iter().find(|(l, _)| *l == label)
    {
        *auth = make();
        changed = true;
    }
    ui.add_space(12.0);
    egui::ScrollArea::vertical()
        .id_salt("auth")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 10.0;
            match auth {
                Auth::None => {
                    EmptyState::new("No authentication")
                        .hint("Pick a scheme above; folders can share auth with Inherit.")
                        .show(ui);
                }
                Auth::Inherit => {
                    EmptyState::new("Inherited")
                        .hint("This request uses the auth of its nearest folder that defines one.")
                        .show(ui);
                }
                Auth::Basic { username, password } | Auth::Digest { username, password } => {
                    changed |= form_field(ui, vars, "Username", username, "username", false);
                    changed |= form_field(ui, vars, "Password", password, "password", true);
                }
                Auth::Bearer { token, prefix } => {
                    changed |= form_field(ui, vars, "Token", token, "token or {{token}}", false);
                    changed |= form_field(ui, vars, "Prefix", prefix, "Bearer", false);
                }
                Auth::ApiKey {
                    key,
                    value,
                    location,
                } => {
                    changed |= form_field(ui, vars, "Key", key, "X-API-Key", false);
                    changed |= form_field(ui, vars, "Value", value, "value", false);
                    ui.label("Add to");
                    changed |= ui
                        .add(SegmentedControl::new(
                            location,
                            [
                                (ApiKeyLocation::Header, "Header"),
                                (ApiKeyLocation::Query, "Query"),
                                (ApiKeyLocation::Cookie, "Cookie"),
                            ],
                        ))
                        .changed();
                }
                Auth::AwsSigV4 {
                    access_key_id,
                    secret_access_key,
                    session_token,
                    region,
                    service,
                } => {
                    changed |= form_field(ui, vars, "Access key ID", access_key_id, "AKIA…", false);
                    changed |= form_field(
                        ui,
                        vars,
                        "Secret access key",
                        secret_access_key,
                        "secret",
                        true,
                    );
                    changed |=
                        form_field(ui, vars, "Session token", session_token, "optional", true);
                    changed |= form_field(ui, vars, "Region", region, "us-east-1", false);
                    changed |=
                        form_field(ui, vars, "Service", service, "execute-api, s3, …", false);
                }
                Auth::OAuth2(cfg) => {
                    let cfg: &mut OAuth2Config = cfg;
                    const GRANTS: [(OAuth2Grant, &str); 4] = [
                        (OAuth2Grant::AuthorizationCode, "Authorization Code"),
                        (OAuth2Grant::ClientCredentials, "Client Credentials"),
                        (OAuth2Grant::Password, "Password"),
                        (OAuth2Grant::Implicit, "Implicit"),
                    ];
                    let mut grant = GRANTS
                        .iter()
                        .find(|(g, _)| *g == cfg.grant)
                        .map_or("", |(_, l)| l)
                        .to_owned();
                    let before = grant.clone();
                    ui.label("Grant type");
                    ui.add(
                        Dropdown::new(
                            "nexus.oauth-grant",
                            &mut grant,
                            GRANTS.iter().map(|(_, l)| (*l).to_owned()),
                        )
                        .label("Grant type")
                        .width(200.0),
                    );
                    if grant != before
                        && let Some((g, _)) = GRANTS.iter().find(|(_, l)| *l == grant)
                    {
                        cfg.grant = *g;
                        changed = true;
                    }
                    let grant = cfg.grant;
                    let redirects = matches!(
                        grant,
                        OAuth2Grant::AuthorizationCode | OAuth2Grant::Implicit
                    );
                    if redirects {
                        changed |= form_field(
                            ui,
                            vars,
                            "Authorization URL",
                            &mut cfg.authorization_url,
                            "https://…/authorize",
                            false,
                        );
                    }
                    if grant != OAuth2Grant::Implicit {
                        changed |= form_field(
                            ui,
                            vars,
                            "Access token URL",
                            &mut cfg.access_token_url,
                            "https://…/token",
                            false,
                        );
                    }
                    changed |= form_field(
                        ui,
                        vars,
                        "Client ID",
                        &mut cfg.client_id,
                        "client id",
                        false,
                    );
                    if grant != OAuth2Grant::Implicit {
                        changed |= form_field(
                            ui,
                            vars,
                            "Client secret",
                            &mut cfg.client_secret,
                            "client secret",
                            true,
                        );
                    }
                    if grant == OAuth2Grant::Password {
                        changed |=
                            form_field(ui, vars, "Username", &mut cfg.username, "username", false);
                        changed |=
                            form_field(ui, vars, "Password", &mut cfg.password, "password", true);
                    }
                    if redirects {
                        changed |= form_field(
                            ui,
                            vars,
                            "Redirect URL",
                            &mut cfg.redirect_url,
                            "http://127.0.0.1:8765/callback",
                            false,
                        );
                    }
                    changed |= form_field(ui, vars, "Scope", &mut cfg.scope, "read write", false);
                    changed |=
                        form_field(ui, vars, "Audience", &mut cfg.audience, "optional", false);
                    if grant == OAuth2Grant::AuthorizationCode {
                        changed |= ui
                            .add(Checkbox::new(&mut cfg.use_pkce, "Use PKCE"))
                            .changed();
                    }
                    changed |= ui
                        .add(Checkbox::new(
                            &mut cfg.credentials_in_header,
                            "Send client credentials as Basic auth",
                        ))
                        .changed();
                    ui.horizontal(|ui| {
                        if ui.add(Button::primary("Fetch Token")).clicked() {
                            actions.fetch_token = true;
                        }
                        match &cfg.token {
                            Some(token) => {
                                let short: String = token.access_token.chars().take(28).collect();
                                ui.label(
                                    egui::RichText::new(format!("Token {short}…"))
                                        .monospace()
                                        .color(colors.success),
                                );
                                if ui.add(Button::ghost("Clear")).clicked() {
                                    cfg.token = None;
                                    changed = true;
                                }
                            }
                            None => {
                                ui.label(
                                    egui::RichText::new("No token yet")
                                        .color(colors.text_secondary),
                                );
                            }
                        }
                    });
                }
            }
        });
    changed
}

// -------------------------------------------------------------------- tests & settings

const SOURCES: [&str; 6] = [
    "Status",
    "Header",
    "JSONPath",
    "Body",
    "Time (ms)",
    "Size (bytes)",
];

fn source_label(source: &Source) -> &'static str {
    match source {
        Source::Status => SOURCES[0],
        Source::Header(_) => SOURCES[1],
        Source::JsonPath(_) => SOURCES[2],
        Source::Body => SOURCES[3],
        Source::ResponseTimeMs => SOURCES[4],
        Source::BodySize => SOURCES[5],
    }
}

fn make_source(label: &str) -> Source {
    match label {
        "Header" => Source::Header("content-type".into()),
        "JSONPath" => Source::JsonPath("$.".into()),
        "Body" => Source::Body,
        "Time (ms)" => Source::ResponseTimeMs,
        "Size (bytes)" => Source::BodySize,
        _ => Source::Status,
    }
}

fn tests_tab(ui: &mut Ui, assertions: &mut Vec<Assertion>) -> bool {
    let mut changed = false;
    ui.add(SectionHeader::new(
        "Assertions run after every response and in the collection runner",
    ));
    let mut remove = None;
    egui::ScrollArea::vertical()
        .id_salt("tests")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (i, a) in assertions.iter_mut().enumerate() {
                let row = i + 1;
                ui.horizontal(|ui| {
                    changed |= ui
                        .add(
                            Checkbox::new(&mut a.enabled, format!("Enabled, assertion {row}"))
                                .label_visible(false),
                        )
                        .changed();
                    let mut source = source_label(&a.source).to_owned();
                    let before = source.clone();
                    ui.add(
                        Dropdown::new(("src", i), &mut source, SOURCES.map(str::to_owned))
                            .label(format!("Source, assertion {row}"))
                            .width(110.0),
                    );
                    if source != before {
                        a.source = make_source(&source);
                        changed = true;
                    }
                    if let Source::Header(arg) | Source::JsonPath(arg) = &mut a.source {
                        changed |= ui
                            .add(
                                TextField::new(arg)
                                    .id(ui.id().with(("arg", i)))
                                    .monospace(true)
                                    .placeholder("name or path")
                                    .width(160.0),
                            )
                            .changed();
                    }
                    let mut op = a.operator.label().to_owned();
                    let before = op.clone();
                    ui.add(
                        Dropdown::new(
                            ("op", i),
                            &mut op,
                            Operator::ALL.map(|o| o.label().to_owned()),
                        )
                        .label(format!("Operator, assertion {row}"))
                        .width(150.0),
                    );
                    if op != before
                        && let Some(o) = Operator::ALL.iter().find(|o| o.label() == op)
                    {
                        a.operator = *o;
                        changed = true;
                    }
                    if a.operator.needs_value() {
                        changed |= ui
                            .add(
                                TextField::new(&mut a.expected)
                                    .id(ui.id().with(("exp", i)))
                                    .placeholder("expected")
                                    .width(160.0),
                            )
                            .changed();
                    }
                    if ui
                        .add(
                            IconButton::new(Glyph::Delete, format!("Delete assertion {row}"))
                                .with(Tooltip::new("Delete")),
                        )
                        .clicked()
                    {
                        remove = Some(i);
                    }
                });
            }
            ui.add_space(6.0);
            if ui
                .add(Button::secondary("Add assertion").icon(Glyph::Plus))
                .clicked()
            {
                assertions.push(Assertion::new(Source::Status, Operator::Equals, "200"));
                changed = true;
            }
        });
    if let Some(i) = remove {
        assertions.remove(i);
        changed = true;
    }
    changed
}

fn tri_state(ui: &mut Ui, label: &str, value: &mut Option<bool>) -> bool {
    const CHOICES: [(&str, Option<bool>); 3] = [
        ("Workspace default", None),
        ("On", Some(true)),
        ("Off", Some(false)),
    ];
    let mut text = CHOICES
        .iter()
        .find(|(_, v)| v == value)
        .map_or("", |(l, _)| l)
        .to_owned();
    let before = text.clone();
    ui.label(label);
    ui.add(
        Dropdown::new(label, &mut text, CHOICES.map(|(l, _)| l.to_owned()))
            .label(label)
            .width(200.0),
    );
    if text == before {
        return false;
    }
    if let Some((_, v)) = CHOICES.iter().find(|(l, _)| *l == text) {
        *value = *v;
    }
    true
}

fn settings_tab(ui: &mut Ui, request: &mut Request) -> bool {
    let mut changed = false;
    let s = &mut request.settings;
    ui.spacing_mut().item_spacing.y = 10.0;
    changed |= tri_state(ui, "Follow redirects", &mut s.follow_redirects);
    changed |= tri_state(ui, "Verify TLS certificates", &mut s.verify_tls);
    let mut text = s.timeout_ms.map(|t| t.to_string()).unwrap_or_default();
    if ui
        .add(
            TextField::new(&mut text)
                .label("Timeout (ms)")
                .placeholder("workspace default")
                .width(200.0),
        )
        .changed()
    {
        s.timeout_ms = text.trim().parse().ok();
        changed = true;
    }
    let mut cookies = !s.disable_cookies;
    if ui
        .add(Checkbox::new(&mut cookies, "Send and store cookies"))
        .changed()
    {
        s.disable_cookies = !cookies;
        changed = true;
    }
    ui.add(SectionHeader::new("Description"));
    changed |= ui
        .add(
            CodeEditor::new(&mut request.description)
                .label("Description")
                .placeholder("Notes for your team (Markdown)")
                .highlighter(&PlainText)
                .wrap(true)
                .height(140.0),
        )
        .changed();
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_typed_query_into_params() {
        let mut r = Request::new("r", Method::Get, "https://x/a?b=1&c");
        sync_params_from_url(&mut r);
        assert_eq!(r.url, "https://x/a");
        assert_eq!(
            r.params,
            vec![KeyValue::new("b", "1"), KeyValue::new("c", "")]
        );
        let mut typing = Request::new("r", Method::Get, "https://x/a?b={{ va");
        sync_params_from_url(&mut typing);
        assert!(typing.params.is_empty());
    }

    #[test]
    fn converting_body_kinds_keeps_content() {
        let json = Body::Json { text: "{}".into() };
        assert_eq!(
            BodyKind::Text.convert(&json),
            Body::Text {
                text: "{}".into(),
                content_type: None
            }
        );
        let form = Body::FormUrlEncoded {
            fields: vec![KeyValue::new("a", "1")],
        };
        let Body::Multipart { fields } = BodyKind::Multipart.convert(&form) else {
            panic!()
        };
        assert_eq!(fields[0].name, "a");
        assert_eq!(
            BodyKind::of(&BodyKind::GraphQl.convert(&Body::None)),
            BodyKind::GraphQl
        );
    }
}
