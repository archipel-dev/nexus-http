//! Request editor: URL bar with method picker and Send, and the Params, Body,
//! Auth, Headers, Tests and Settings tabs.

use crate::state::{NexusState, RequestTab};
use crate::theme::{Palette, semibold};
use crate::widgets::{
    self, Icon, Syntax, caption, code_editor, field_frame, icon_button, kv_table, primary_button,
    tab_strip, template_field,
};
use egui::{CornerRadius, RichText, Sense, Ui, vec2};
use nexus_http_core::assertions::{Assertion, Operator, Source};
use nexus_http_core::model::{
    ApiKeyLocation, Auth, Body, Id, KeyValue, Method, MultipartField, MultipartValue, OAuth2Config,
    OAuth2Grant, Protocol, Request,
};
use serde_json::{Map, Value};

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

pub fn show(ui: &mut Ui, state: &mut NexusState, id: &Id, actions: &mut RequestActions) {
    let pal = Palette::of(ui);
    let vars = variables_for(state, id);
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
        ui.add_space(12.0);
        let bar_width = ui.available_width() - 12.0;
        let frame = field_frame(&pal).inner_margin(egui::Margin::symmetric(4, 3));
        frame.show(ui, |ui| {
            ui.set_width(bar_width - 10.0);
            ui.horizontal(|ui| {
                changed |= method_picker(ui, &mut request.method, &mut request.protocol);
                let (sep, _) = ui.allocate_exact_size(vec2(1.0, 22.0), Sense::hover());
                ui.painter().rect_filled(sep, 0.0, pal.border);
                let send_w = if request.protocol.is_http() {
                    86.0
                } else {
                    112.0
                };
                let url_w = ui.available_width() - send_w - 34.0;
                let hint = match request.protocol {
                    Protocol::Http => "Enter a URL, paste a curl command, or use {{ base_url }}",
                    Protocol::WebSocket => "ws://host/socket or wss://…",
                    Protocol::Sse => "https://host/events",
                    Protocol::Grpc => "grpc://host:50051 or grpcs://host:443",
                };
                let resp = template_field(
                    ui,
                    &mut request.url,
                    hint,
                    &vars,
                    url_w,
                    egui::Id::new(URL_FIELD_ID),
                );
                if resp.changed() {
                    changed = true;
                    sync_params_from_url(request);
                }
                if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    actions.send = true;
                }
                if icon_button(ui, Icon::Code, pal.weak, "Generate code").clicked() {
                    actions.generate_code = true;
                }
                if !request.protocol.is_http() {
                    if connected {
                        if widgets::secondary_button(ui, "Disconnect").clicked() {
                            actions.disconnect = true;
                        }
                    } else if primary_button(ui, "Connect", true)
                        .on_hover_text("Connect (Ctrl+Enter)")
                        .clicked()
                    {
                        actions.connect = true;
                    }
                } else if sending {
                    if widgets::secondary_button(ui, "Cancel").clicked() {
                        actions.cancel = true;
                    }
                } else if primary_button(ui, "Send", true)
                    .on_hover_text("Send (Ctrl+Enter)")
                    .clicked()
                {
                    actions.send = true;
                }
            });
        });
    });
    if request.protocol == Protocol::Grpc {
        changed |= grpc_bar(ui, request, &vars, grpc_schema.as_ref(), actions);
    }
    ui.add_space(4.0);

    // ---------------------------------------------------------------- tabs
    ui.horizontal(|ui| {
        ui.add_space(8.0);
        let counts = TabCounts::of(request);
        tab_strip(
            ui,
            &mut current_tab,
            &RequestTab::ALL,
            |t| match t {
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
                    Auth::Basic { .. } => "Basic".to_owned(),
                    Auth::Bearer { .. } => "Bearer".to_owned(),
                    Auth::ApiKey { .. } => "API Key".to_owned(),
                    Auth::Digest { .. } => "Digest".to_owned(),
                    Auth::OAuth2(_) => "OAuth 2.0".to_owned(),
                    Auth::AwsSigV4 { .. } => "AWS SigV4".to_owned(),
                },
                other => other.label().to_owned(),
            },
            |t| counts.badge(t),
        );
    });
    let line_y = ui.cursor().top();
    ui.painter()
        .hline(ui.max_rect().x_range(), line_y, pal.hairline());
    ui.add_space(8.0);

    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 0))
        .show(ui, |ui| {
            ui.set_min_height(ui.available_height());
            match current_tab {
                RequestTab::Params => {
                    caption(ui, "Query parameters");
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .id_salt("params")
                        .show(ui, |ui| {
                            if kv_table(ui, &mut request.params, "params", "Add parameter", &vars) {
                                changed = true;
                            }
                        });
                }
                RequestTab::Headers => {
                    caption(ui, "Headers");
                    ui.add_space(4.0);
                    egui::ScrollArea::vertical()
                        .id_salt("headers")
                        .show(ui, |ui| {
                            if kv_table(ui, &mut request.headers, "headers", "Add header", &vars) {
                                changed = true;
                            }
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
pub fn variables_for(state: &NexusState, id: &Id) -> Map<String, Value> {
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
    vars: &Map<String, Value>,
    schema: Option<&GrpcSchemaView>,
    actions: &mut RequestActions,
) -> bool {
    let pal = Palette::of(ui);
    let mut changed = false;
    let loading = schema.is_some_and(|s| s.1);
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(12.0);
        let half = (ui.available_width() - 150.0) / 2.0;
        changed |= template_field(
            ui,
            &mut request.grpc.proto_file,
            ".proto file (empty: server reflection)",
            vars,
            half,
            egui::Id::new("nexus.grpc.proto"),
        )
        .changed();
        changed |= template_field(
            ui,
            &mut request.grpc.method,
            "package.Service/Method",
            vars,
            half,
            egui::Id::new("nexus.grpc.method"),
        )
        .changed();
        let methods = schema.map(|s| s.0.as_slice()).unwrap_or(&[]);
        let label = if loading { "Loading…" } else { "Methods" };
        let button = widgets::secondary_button(ui, label);
        if button.clicked() && !loading {
            actions.load_grpc_schema = true;
        }
        if !methods.is_empty() {
            egui::Popup::menu(&button)
                .id(egui::Id::new("nexus.grpc.methods"))
                .width(320.0)
                .show(|ui| {
                    for m in methods {
                        let text = format!("{}  ·  {}", m.name, m.kind());
                        if ui
                            .selectable_label(request.grpc.method == m.name, text)
                            .clicked()
                        {
                            request.grpc.method = m.name.clone();
                            if matches!(&request.body, Body::None)
                                || matches!(&request.body, Body::Json { text } if text.trim().is_empty())
                            {
                                request.body = Body::Json {
                                    text: m.template.clone(),
                                };
                            }
                            changed = true;
                        }
                    }
                });
        }
    });
    if let Some((_, _, Some(error))) = schema {
        ui.horizontal(|ui| {
            ui.add_space(14.0);
            ui.label(RichText::new(error).size(12.0).color(pal.error));
        });
    }
    changed
}

fn method_picker(ui: &mut Ui, method: &mut Method, protocol: &mut Protocol) -> bool {
    let pal = Palette::of(ui);
    let font = semibold(ui, 13.0);
    let (text, color) = match protocol {
        Protocol::Http => (method.as_str().to_owned(), pal.method(method)),
        Protocol::WebSocket => ("WS".to_owned(), pal.syntax_number),
        Protocol::Sse => ("SSE".to_owned(), pal.info),
        Protocol::Grpc => ("gRPC".to_owned(), pal.success),
    };
    let galley = ui.painter().layout_no_wrap(text, font.clone(), color);
    let (rect, resp) =
        ui.allocate_exact_size(vec2(galley.size().x.max(44.0) + 30.0, 26.0), Sense::click());
    if resp.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(5), pal.hover);
    }
    ui.painter().galley(
        egui::pos2(rect.left() + 10.0, rect.center().y - galley.size().y / 2.0),
        galley,
        pal.text,
    );
    let chevron = egui::Rect::from_center_size(
        egui::pos2(rect.right() - 11.0, rect.center().y),
        vec2(10.0, 10.0),
    );
    widgets::paint_icon(ui.painter(), chevron, Icon::Chevron(true), pal.weak);
    let mut changed = false;
    egui::Popup::menu(&resp).width(170.0).show(|ui| {
        for m in Method::COMMON {
            let label = RichText::new(m.as_str())
                .font(semibold(ui, 12.5))
                .color(pal.method(&m));
            if ui
                .selectable_label(protocol.is_http() && *method == m, label)
                .clicked()
            {
                *method = m;
                *protocol = Protocol::Http;
                changed = true;
            }
        }
        ui.separator();
        for (p, label, color) in [
            (Protocol::WebSocket, "WebSocket", pal.syntax_number),
            (Protocol::Sse, "Server-Sent Events", pal.info),
            (Protocol::Grpc, "gRPC", pal.success),
        ] {
            let text = RichText::new(label).font(semibold(ui, 12.5)).color(color);
            if ui.selectable_label(*protocol == p, text).clicked() {
                *protocol = p;
                *method = Method::Get;
                changed = true;
            }
        }
    });
    changed
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

fn body_tab(ui: &mut Ui, body: &mut Body, vars: &Map<String, Value>, gql: &mut GqlContext) -> bool {
    let pal = Palette::of(ui);
    let mut changed = false;
    let current = BodyKind::of(body);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for kind in BodyKind::ALL {
            let selected = kind == current;
            let text = RichText::new(kind.label()).size(12.0).color(if selected {
                pal.strong
            } else {
                pal.weak
            });
            let resp = ui
                .add(egui::Button::selectable(selected, text).corner_radius(CornerRadius::same(5)));
            if resp.clicked() && !selected {
                *body = kind.convert(body);
                changed = true;
            }
        }
        if let Body::Json { text } = body {
            ui.add_space(8.0);
            if ui
                .add(
                    egui::Button::new(RichText::new("Beautify").size(12.0).color(pal.accent))
                        .frame(false),
                )
                .on_hover_text("Re-indent the JSON")
                .clicked()
                && let Ok(v) = serde_json::from_str::<Value>(text)
                && let Ok(pretty) = serde_json::to_string_pretty(&v)
            {
                *text = pretty;
                changed = true;
            }
        }
    });
    ui.add_space(8.0);
    match body {
        Body::None => {
            widgets::empty_state(ui, "No body", "Pick a body type above to send a payload.")
        }
        Body::Json { text } => {
            if !text.trim().is_empty()
                && !nexus_http_core::template::has_templates(text)
                && let Err(e) = serde_json::from_str::<Value>(text)
            {
                ui.label(
                    RichText::new(format!("Invalid JSON: {e}"))
                        .size(11.5)
                        .color(pal.warning),
                );
            }
            changed |= editor_box(ui, text, Syntax::Json, vars, "json");
        }
        Body::Xml { text } => changed |= editor_box(ui, text, Syntax::Plain, vars, "xml"),
        Body::Text { text, content_type } => {
            ui.horizontal(|ui| {
                caption(ui, "Content-Type");
                let mut ct = content_type.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut ct)
                            .hint_text("text/plain")
                            .desired_width(220.0),
                    )
                    .changed()
                {
                    *content_type = (!ct.trim().is_empty()).then_some(ct);
                    changed = true;
                }
            });
            ui.add_space(4.0);
            changed |= editor_box(ui, text, Syntax::Plain, vars, "text");
        }
        Body::FormUrlEncoded { fields } => {
            changed |= kv_table(ui, fields, "form", "Add field", vars);
        }
        Body::Multipart { fields } => changed |= multipart(ui, fields, vars),
        Body::Binary { path } => {
            caption(
                ui,
                "File sent as the request body (absolute or relative to the project)",
            );
            ui.add_space(4.0);
            field_frame(&pal).show(ui, |ui| {
                changed |= template_field(
                    ui,
                    path,
                    "path/to/file.bin",
                    vars,
                    ui.available_width(),
                    ui.id().with("binpath"),
                )
                .changed();
            });
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
    vars: &Map<String, Value>,
    connected: bool,
    actions: &mut RequestActions,
) -> bool {
    let pal = Palette::of(ui);
    let mut changed = false;
    if !matches!(body, Body::Json { .. } | Body::Text { .. }) {
        *body = Body::Json {
            text: String::new(),
        };
        changed = true;
    }
    ui.horizontal(|ui| {
        let is_json = matches!(body, Body::Json { .. });
        for (label, json) in [("JSON", true), ("Text", false)] {
            let text = RichText::new(label).size(12.0).color(if is_json == json {
                pal.strong
            } else {
                pal.weak
            });
            if ui
                .add(
                    egui::Button::selectable(is_json == json, text)
                        .corner_radius(CornerRadius::same(5)),
                )
                .clicked()
                && is_json != json
            {
                *body = if json {
                    BodyKind::Json.convert(body)
                } else {
                    BodyKind::Text.convert(body)
                };
                changed = true;
            }
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if primary_button(ui, "Send message", connected)
                .on_hover_text(if connected {
                    "Send (Ctrl+Enter)"
                } else {
                    "Connect first"
                })
                .clicked()
            {
                actions.send_message = true;
            }
        });
    });
    ui.add_space(8.0);
    match body {
        Body::Json { text } => changed |= editor_box(ui, text, Syntax::Json, vars, "wsjson"),
        Body::Text { text, .. } => changed |= editor_box(ui, text, Syntax::Plain, vars, "wstext"),
        _ => {}
    }
    changed
}

fn editor_box(
    ui: &mut Ui,
    text: &mut String,
    syntax: Syntax,
    vars: &Map<String, Value>,
    salt: &str,
) -> bool {
    let pal = Palette::of(ui);
    field_frame(&pal)
        .inner_margin(egui::Margin::same(8))
        .show(ui, |ui| {
            ui.set_min_height(ui.available_height() - 16.0);
            code_editor(ui, text, syntax, vars, ui.id().with(salt)).changed()
        })
        .inner
}

fn multipart(ui: &mut Ui, fields: &mut Vec<MultipartField>, vars: &Map<String, Value>) -> bool {
    let pal = Palette::of(ui);
    let mut changed = false;
    let mut remove = None;
    for (i, field) in fields.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= ui.checkbox(&mut field.enabled, "").changed();
            let w = (ui.available_width() - 120.0) / 2.0;
            changed |= template_field(
                ui,
                &mut field.name,
                "name",
                vars,
                w,
                ui.id().with(("mpn", i)),
            )
            .changed();
            match &mut field.value {
                MultipartValue::Text { value } => {
                    changed |=
                        template_field(ui, value, "value", vars, w, ui.id().with(("mpv", i)))
                            .changed();
                }
                MultipartValue::File { path, .. } => {
                    ui.label(RichText::new("file").size(11.5).color(pal.weak));
                    changed |= template_field(
                        ui,
                        path,
                        "path/to/file",
                        vars,
                        w - 20.0,
                        ui.id().with(("mpf", i)),
                    )
                    .changed();
                }
            }
            let is_file = matches!(field.value, MultipartValue::File { .. });
            if ui
                .small_button(if is_file { "Text" } else { "File" })
                .on_hover_text("Switch between text and file")
                .clicked()
            {
                field.value = if is_file {
                    MultipartValue::Text {
                        value: String::new(),
                    }
                } else {
                    MultipartValue::File {
                        path: String::new(),
                        content_type: None,
                    }
                };
                changed = true;
            }
            if icon_button(ui, Icon::Trash, pal.weak, "Delete").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        fields.remove(i);
        changed = true;
    }
    ui.horizontal(|ui| {
        if ui.button("+ Text field").clicked() {
            fields.push(MultipartField {
                name: String::new(),
                value: MultipartValue::Text {
                    value: String::new(),
                },
                enabled: true,
            });
            changed = true;
        }
        if ui.button("+ File").clicked() {
            fields.push(MultipartField {
                name: String::new(),
                value: MultipartValue::File {
                    path: String::new(),
                    content_type: None,
                },
                enabled: true,
            });
            changed = true;
        }
    });
    changed
}

// -------------------------------------------------------------------- auth

/// A scheme button label and a constructor for its default configuration.
type AuthChoice = (&'static str, fn() -> Auth);

fn auth_tab(
    ui: &mut Ui,
    auth: &mut Auth,
    vars: &Map<String, Value>,
    actions: &mut RequestActions,
) -> bool {
    let pal = Palette::of(ui);
    let mut changed = false;
    let kinds: [AuthChoice; 8] = [
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
    let current = std::mem::discriminant(auth);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        for (label, make) in &kinds {
            let candidate = make();
            let selected = std::mem::discriminant(&candidate) == current;
            let text = RichText::new(*label).size(12.0).color(if selected {
                pal.strong
            } else {
                pal.weak
            });
            if ui
                .add(egui::Button::selectable(selected, text).corner_radius(CornerRadius::same(5)))
                .clicked()
                && !selected
            {
                *auth = candidate;
                changed = true;
            }
        }
    });
    ui.add_space(12.0);
    let grid = |ui: &mut Ui, f: &mut dyn FnMut(&mut Ui) -> bool| -> bool {
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            f(ui)
        })
        .inner
    };
    let field_w = (ui.available_width() - 190.0).clamp(160.0, 520.0);
    let row = |ui: &mut Ui, label: &str, value: &mut String, hint: &str, secret: bool| -> bool {
        ui.horizontal(|ui| {
            widgets::form_label(ui, label, 150.0);
            field_frame(&pal)
                .show(ui, |ui| {
                    if secret && !value.contains("{{") {
                        ui.add(
                            egui::TextEdit::singleline(value)
                                .password(true)
                                .frame(false)
                                .desired_width(field_w)
                                .hint_text(hint),
                        )
                    } else {
                        template_field(ui, value, hint, vars, field_w, ui.id().with(label))
                    }
                })
                .inner
                .changed()
        })
        .inner
    };
    match auth {
        Auth::None => widgets::empty_state(
            ui,
            "No authentication",
            "Pick a scheme above; folders can share auth with Inherit.",
        ),
        Auth::Inherit => widgets::empty_state(
            ui,
            "Inherited",
            "This request uses the auth of its nearest folder that defines one.",
        ),
        Auth::Basic { username, password } | Auth::Digest { username, password } => {
            changed |= grid(ui, &mut |ui| {
                row(ui, "Username", username, "username", false)
                    | row(ui, "Password", password, "password", true)
            });
        }
        Auth::Bearer { token, prefix } => {
            changed |= grid(ui, &mut |ui| {
                row(ui, "Token", token, "token or {{ token }}", false)
                    | row(ui, "Prefix", prefix, "Bearer", false)
            });
        }
        Auth::ApiKey {
            key,
            value,
            location,
        } => {
            changed |= grid(ui, &mut |ui| {
                let a = row(ui, "Key", key, "X-API-Key", false);
                let b = row(ui, "Value", value, "value", false);
                let mut c = false;
                ui.horizontal(|ui| {
                    widgets::form_label(ui, "Add to", 150.0);
                    for (l, loc) in [
                        ("Header", ApiKeyLocation::Header),
                        ("Query", ApiKeyLocation::Query),
                        ("Cookie", ApiKeyLocation::Cookie),
                    ] {
                        c |= ui.radio_value(location, loc, l).changed();
                    }
                });
                a | b | c
            });
        }
        Auth::AwsSigV4 {
            access_key_id,
            secret_access_key,
            session_token,
            region,
            service,
        } => {
            changed |= grid(ui, &mut |ui| {
                row(ui, "Access key ID", access_key_id, "AKIA…", false)
                    | row(ui, "Secret access key", secret_access_key, "secret", true)
                    | row(ui, "Session token", session_token, "optional", true)
                    | row(ui, "Region", region, "us-east-1", false)
                    | row(ui, "Service", service, "execute-api, s3, …", false)
            });
        }
        Auth::OAuth2(cfg) => {
            let cfg: &mut OAuth2Config = cfg;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Grant type").color(pal.weak));
                ui.add_space(50.0);
                for (l, g) in [
                    ("Authorization Code", OAuth2Grant::AuthorizationCode),
                    ("Client Credentials", OAuth2Grant::ClientCredentials),
                    ("Password", OAuth2Grant::Password),
                    ("Implicit", OAuth2Grant::Implicit),
                ] {
                    changed |= ui.radio_value(&mut cfg.grant, g, l).changed();
                }
            });
            ui.add_space(8.0);
            let grant = cfg.grant;
            changed |= grid(ui, &mut |ui| {
                let mut c = false;
                if matches!(
                    grant,
                    OAuth2Grant::AuthorizationCode | OAuth2Grant::Implicit
                ) {
                    c |= row(
                        ui,
                        "Authorization URL",
                        &mut cfg.authorization_url,
                        "https://…/authorize",
                        false,
                    );
                }
                if grant != OAuth2Grant::Implicit {
                    c |= row(
                        ui,
                        "Access token URL",
                        &mut cfg.access_token_url,
                        "https://…/token",
                        false,
                    );
                }
                c |= row(ui, "Client ID", &mut cfg.client_id, "client id", false);
                if grant != OAuth2Grant::Implicit {
                    c |= row(
                        ui,
                        "Client secret",
                        &mut cfg.client_secret,
                        "client secret",
                        true,
                    );
                }
                if grant == OAuth2Grant::Password {
                    c |= row(ui, "Username", &mut cfg.username, "username", false);
                    c |= row(ui, "Password", &mut cfg.password, "password", true);
                }
                if matches!(
                    grant,
                    OAuth2Grant::AuthorizationCode | OAuth2Grant::Implicit
                ) {
                    c |= row(
                        ui,
                        "Redirect URL",
                        &mut cfg.redirect_url,
                        "http://127.0.0.1:8765/callback",
                        false,
                    );
                }
                c |= row(ui, "Scope", &mut cfg.scope, "read write", false);
                c |= row(ui, "Audience", &mut cfg.audience, "optional", false);
                c
            });
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if grant == OAuth2Grant::AuthorizationCode {
                    changed |= ui.checkbox(&mut cfg.use_pkce, "Use PKCE").changed();
                }
                changed |= ui
                    .checkbox(
                        &mut cfg.credentials_in_header,
                        "Send client credentials as Basic auth",
                    )
                    .changed();
            });
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if primary_button(ui, "Fetch Token", true).clicked() {
                    actions.fetch_token = true;
                }
                match &cfg.token {
                    Some(token) => {
                        let short: String = token.access_token.chars().take(28).collect();
                        ui.label(
                            RichText::new(format!("Token {short}…"))
                                .font(crate::theme::mono(12.0))
                                .color(pal.success),
                        );
                        if ui.small_button("Clear").clicked() {
                            cfg.token = None;
                            changed = true;
                        }
                    }
                    None => {
                        ui.label(RichText::new("No token yet").color(pal.weak));
                    }
                }
            });
        }
    }
    changed
}

// -------------------------------------------------------------------- tests & settings

fn tests_tab(ui: &mut Ui, assertions: &mut Vec<Assertion>) -> bool {
    let pal = Palette::of(ui);
    let mut changed = false;
    caption(
        ui,
        "Assertions run after every response and in the collection runner",
    );
    ui.add_space(6.0);
    let mut remove = None;
    for (i, a) in assertions.iter_mut().enumerate() {
        ui.horizontal(|ui| {
            changed |= ui.checkbox(&mut a.enabled, "").changed();
            let source_label = match &a.source {
                Source::Status => "Status",
                Source::Header(_) => "Header",
                Source::JsonPath(_) => "JSONPath",
                Source::Body => "Body",
                Source::ResponseTimeMs => "Time (ms)",
                Source::BodySize => "Size (bytes)",
            };
            egui::ComboBox::from_id_salt(("src", i))
                .selected_text(source_label)
                .width(100.0)
                .show_ui(ui, |ui| {
                    for (label, make) in [
                        ("Status", Source::Status),
                        ("Header", Source::Header("content-type".into())),
                        ("JSONPath", Source::JsonPath("$.".into())),
                        ("Body", Source::Body),
                        ("Time (ms)", Source::ResponseTimeMs),
                        ("Size (bytes)", Source::BodySize),
                    ] {
                        if ui.selectable_label(source_label == label, label).clicked() {
                            a.source = make;
                            changed = true;
                        }
                    }
                });
            if let Source::Header(arg) | Source::JsonPath(arg) = &mut a.source {
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(arg)
                            .desired_width(140.0)
                            .font(crate::theme::mono(12.0)),
                    )
                    .changed();
            }
            egui::ComboBox::from_id_salt(("op", i))
                .selected_text(a.operator.label())
                .width(120.0)
                .show_ui(ui, |ui| {
                    for op in Operator::ALL {
                        changed |= ui
                            .selectable_value(&mut a.operator, op, op.label())
                            .changed();
                    }
                });
            if a.operator.needs_value() {
                changed |= ui
                    .add(
                        egui::TextEdit::singleline(&mut a.expected)
                            .desired_width(140.0)
                            .hint_text("expected"),
                    )
                    .changed();
            }
            if icon_button(ui, Icon::Trash, pal.weak, "Delete").clicked() {
                remove = Some(i);
            }
        });
    }
    if let Some(i) = remove {
        assertions.remove(i);
        changed = true;
    }
    ui.add_space(6.0);
    if ui.button("+ Add assertion").clicked() {
        assertions.push(Assertion::new(Source::Status, Operator::Equals, "200"));
        changed = true;
    }
    changed
}

fn settings_tab(ui: &mut Ui, request: &mut Request) -> bool {
    let mut changed = false;
    let s = &mut request.settings;
    let tri = |ui: &mut Ui, label: &str, value: &mut Option<bool>| -> bool {
        let mut c = false;
        ui.horizontal(|ui| {
            widgets::form_label(ui, label, 180.0);
            for (l, v) in [
                ("Workspace default", None),
                ("On", Some(true)),
                ("Off", Some(false)),
            ] {
                c |= ui.radio_value(value, v, l).changed();
            }
        });
        c
    };
    changed |= tri(ui, "Follow redirects", &mut s.follow_redirects);
    changed |= tri(ui, "Verify TLS certificates", &mut s.verify_tls);
    ui.horizontal(|ui| {
        widgets::form_label(ui, "Timeout (ms)", 180.0);
        let mut text = s.timeout_ms.map(|t| t.to_string()).unwrap_or_default();
        if ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .hint_text("workspace default")
                    .desired_width(140.0),
            )
            .changed()
        {
            s.timeout_ms = text.trim().parse().ok();
            changed = true;
        }
    });
    ui.horizontal(|ui| {
        widgets::form_label(ui, "Cookies", 180.0);
        let mut enabled = !s.disable_cookies;
        if ui
            .checkbox(&mut enabled, "Send and store cookies")
            .changed()
        {
            s.disable_cookies = !enabled;
            changed = true;
        }
    });
    ui.add_space(12.0);
    caption(ui, "Description");
    changed |= ui
        .add(
            egui::TextEdit::multiline(&mut request.description)
                .desired_rows(4)
                .desired_width(f32::INFINITY)
                .hint_text("Notes for your team (Markdown)"),
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
