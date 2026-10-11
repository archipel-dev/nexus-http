//! Request editor: an island titled with the request's name, its sections as content tabs
//! (Params, Body, Auth, Headers, Tests, Settings), the URL bar and the selected section.
//! Built only from kit components; the editing rules are `nexus_http_core::editing`.

use crate::state::{NexusState, RequestTab};
use crate::views::template::Templates;
use archipel_kit::behaviour::WidgetExt;
use archipel_kit::components::{
    Button, Checkbox, CodeEditor, Dropdown, EmptyState, FieldLabel, Form, FormUi, IconButton,
    KeyValueTable, Panel, Radio, TextField, Tooltip,
};
use archipel_kit::parts::Tone;
use archipel_kit::tokens::size;
use archipel_kit::{Glyph, Theme};
use egui::{RichText, Ui};
use nexus_http_core::assertions::{Assertion, Operator, Source};
use nexus_http_core::editing::{
    self, AuthKind, BodyKind, OAUTH2_GRANTS, SourceKind, Target, set_auth_kind, set_body_kind,
    set_source_kind, set_target,
};
use nexus_http_core::model::{
    ApiKeyLocation, Auth, Body, Id, KeyValue, MultipartField, MultipartValue, OAuth2Config,
    OAuth2Grant, Protocol, Request,
};

pub const URL_FIELD_ID: &str = "nexus.url";

/// Width of the method picker.
const TARGET_WIDTH: f32 = 96.0;
/// Width of the Send / Connect / Cancel button.
const SEND_WIDTH: f32 = 84.0;
/// Width of the dropdowns of an assertion row.
const ASSERTION_PICKER_WIDTH: f32 = 120.0;
/// Width of short argument fields (JSONPath, header name, expected value).
const ARGUMENT_WIDTH: f32 = 150.0;
/// Width of a form field in the Auth and Settings sections.
const FORM_FIELD_WIDTH: f32 = 360.0;
/// Width of the body type picker.
const KIND_WIDTH: f32 = 170.0;
/// Width of the text/file picker of a multipart row.
const PART_KIND_WIDTH: f32 = 84.0;

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

/// GraphQL schema state handed to the body section.
#[derive(Default)]
struct GqlContext {
    schema: Option<std::sync::Arc<nexus_http_core::graphql::Schema>>,
    loading: bool,
    error: Option<String>,
    fetch: bool,
}

type GrpcSchemaView = (Vec<nexus_http_core::grpc::MethodInfo>, bool, Option<String>);

pub fn show(ui: &mut Ui, state: &mut NexusState, id: &Id, actions: &mut RequestActions) {
    let templates = Templates::for_request(ui, state, id);
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
    let mut tab = RequestTab::ALL
        .iter()
        .position(|t| *t == state.request_tab)
        .unwrap_or(0);
    let Some(request) = state.request_mut(id) else {
        return;
    };
    let labels: Vec<String> = RequestTab::ALL
        .iter()
        .map(|t| tab_label(*t, request))
        .collect();
    let title = request.name.clone();
    let mut changed = false;
    let mut generate_code = false;
    // The section shown is the one selected when the frame began; a click on another tab
    // shows it on the next frame, which the click requests.
    let section = RequestTab::ALL[tab.min(RequestTab::ALL.len() - 1)];

    Panel::new(title)
        .tabs(&labels, &mut tab)
        .hover_actions(true)
        .actions(|ui| {
            let code =
                IconButton::new(Glyph::Code, "Generate code").with(Tooltip::new("Generate code"));
            generate_code |= ui.add(code).clicked();
        })
        .padded(true)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = size::SECTION_SPACING;
            changed |= url_bar(ui, request, &templates, sending, connected, actions);
            if request.protocol == Protocol::Grpc {
                changed |= grpc_bar(ui, request, &templates, grpc_schema.as_ref(), actions);
            }
            egui::ScrollArea::vertical()
                .id_salt(("request-section", section.label()))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    changed |= match section {
                        RequestTab::Params => rows_table(
                            ui,
                            "params",
                            &mut request.params,
                            "Add parameter",
                            &templates,
                        ),
                        RequestTab::Headers => rows_table(
                            ui,
                            "headers",
                            &mut request.headers,
                            "Add header",
                            &templates,
                        ),
                        RequestTab::Body if is_message(request.protocol) => {
                            message_section(ui, &mut request.body, &templates, connected, actions)
                        }
                        RequestTab::Body => {
                            body_section(ui, &mut request.body, &templates, &mut gql)
                        }
                        RequestTab::Auth => {
                            auth_section(ui, &mut request.auth, &templates, actions)
                        }
                        RequestTab::Tests => tests_section(ui, &mut request.assertions),
                        RequestTab::Settings => settings_section(ui, request),
                    };
                });
        });

    actions.generate_code |= generate_code;
    let picked = RequestTab::ALL[tab.min(RequestTab::ALL.len() - 1)];
    if picked != section {
        ui.ctx().request_repaint();
    }
    state.request_tab = picked;
    if gql.fetch {
        state.fetch_schema(id);
    }
    if changed {
        state.touch();
    }
}

fn is_message(protocol: Protocol) -> bool {
    matches!(protocol, Protocol::WebSocket | Protocol::Grpc)
}

/// A section tab's label: what the section holds, and how many rows are on.
fn tab_label(tab: RequestTab, request: &Request) -> String {
    let count = |n: usize| {
        if n > 0 {
            format!("{} {n}", tab.label())
        } else {
            tab.label().to_owned()
        }
    };
    match tab {
        RequestTab::Body if is_message(request.protocol) => "Message".to_owned(),
        RequestTab::Body => match &request.body {
            Body::None => "Body".to_owned(),
            other => BodyKind::of(other).label().to_owned(),
        },
        RequestTab::Auth => match AuthKind::of(&request.auth) {
            AuthKind::None | AuthKind::Inherit => "Auth".to_owned(),
            kind => kind.label().to_owned(),
        },
        RequestTab::Params => count(request.params.iter().filter(|p| p.enabled).count()),
        RequestTab::Headers => count(request.headers.iter().filter(|p| p.enabled).count()),
        RequestTab::Tests => count(request.assertions.iter().filter(|a| a.enabled).count()),
        RequestTab::Settings => tab.label().to_owned(),
    }
}

/// The tone of a target's name: the method's color, or the protocol's.
pub fn target_tone(target: &Target) -> Tone {
    match target {
        Target::Http(m) => Tone::method(&m.to_string()),
        Target::WebSocket => Tone::Accent,
        Target::Sse => Tone::Link,
        Target::Grpc => Tone::Success,
    }
}

/// Method or protocol, URL with `{{ variables }}`, and Send.
fn url_bar(
    ui: &mut Ui,
    request: &mut Request,
    templates: &Templates,
    sending: bool,
    connected: bool,
    actions: &mut RequestActions,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        let mut target = Target::of(request);
        ui.add(
            Dropdown::new("nexus.target", &mut target, Target::all())
                .separators([Target::protocols_start()])
                .tone(target_tone)
                .label("Method")
                .width(TARGET_WIDTH),
        );
        changed |= set_target(request, target);

        let url_width = (ui.available_width() - SEND_WIDTH - size::TOOLBAR_GAP).max(0.0);
        let out = templates
            .field(&mut request.url)
            .id(egui::Id::new(URL_FIELD_ID))
            .name("URL")
            .placeholder(editing::url_hint(request.protocol))
            .width(url_width)
            .show(ui);
        if out.response.changed() {
            changed = true;
            editing::take_query_into_params(request);
        }
        if out.submitted {
            actions.send = true;
        }

        let send = |label: &str| {
            Button::primary(label)
                .min_width(SEND_WIDTH)
                .with(Tooltip::new(label).shortcut("Ctrl+Enter"))
        };
        if !request.protocol.is_http() {
            if connected {
                actions.disconnect |= ui
                    .add(Button::secondary("Disconnect").min_width(SEND_WIDTH))
                    .clicked();
            } else {
                actions.connect |= ui.add(send("Connect")).clicked();
            }
        } else if sending {
            actions.cancel |= ui
                .add(Button::secondary("Cancel").min_width(SEND_WIDTH))
                .clicked();
        } else {
            actions.send |= ui.add(send("Send")).clicked();
        }
    });
    changed
}

/// The gRPC row under the URL bar: the schema source and the method to call.
fn grpc_bar(
    ui: &mut Ui,
    request: &mut Request,
    templates: &Templates,
    schema: Option<&GrpcSchemaView>,
    actions: &mut RequestActions,
) -> bool {
    let mut changed = false;
    let loading = schema.is_some_and(|s| s.1);
    let methods = schema.map(|s| s.0.as_slice()).unwrap_or(&[]);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        let half = (ui.available_width() - SEND_WIDTH - 2.0 * size::TOOLBAR_GAP) / 2.0;
        changed |= ui
            .add(
                templates
                    .field(&mut request.grpc.proto_file)
                    .name(".proto file")
                    .placeholder(".proto file (empty: server reflection)")
                    .width(half),
            )
            .changed();
        if methods.is_empty() {
            changed |= ui
                .add(
                    templates
                        .field(&mut request.grpc.method)
                        .name("gRPC method")
                        .placeholder("package.Service/Method")
                        .width(half),
                )
                .changed();
        } else {
            let mut picked = request.grpc.method.clone();
            let names: Vec<String> = methods.iter().map(|m| m.name.clone()).collect();
            ui.add(
                Dropdown::new("nexus.grpc.method", &mut picked, names)
                    .label("gRPC method")
                    .width(half),
            );
            if picked != request.grpc.method
                && let Some(method) = methods.iter().find(|m| m.name == picked)
            {
                editing::pick_grpc_method(request, &method.name, &method.template);
                changed = true;
            }
        }
        let label = if loading { "Loading…" } else { "Methods" };
        let load = Button::secondary(label)
            .min_width(SEND_WIDTH)
            .enabled(!loading)
            .with(Tooltip::new(
                "Ask the server (reflection) or the .proto file for its methods",
            ));
        actions.load_grpc_schema |= ui.add(load).clicked();
    });
    if let Some((_, _, Some(error))) = schema {
        problem(ui, error);
    }
    changed
}

/// A line of text in the warning color: a problem with the input above it.
pub fn problem(ui: &mut Ui, text: &str) {
    let colors = Theme::colors(ui.ctx());
    ui.label(RichText::new(text).color(Tone::Warning.color(&colors)));
}

/// Name/value rows with `{{ variables }}` in the values: params, headers, form fields.
fn rows_table(
    ui: &mut Ui,
    id: &str,
    rows: &mut Vec<KeyValue>,
    placeholder: &str,
    templates: &Templates,
) -> bool {
    let out = KeyValueTable::with_fields(
        id,
        rows,
        |r| &mut r.enabled,
        |r| &mut r.name,
        |r| &mut r.value,
    )
    .key_placeholder(placeholder)
    .value_placeholder("value")
    .value_cell(|ui, cell| {
        ui.add(
            templates
                .field(cell.text)
                .placeholder(cell.placeholder)
                .name(cell.name)
                .width(cell.width)
                .enabled(!cell.read_only),
        )
    })
    .show(ui);
    !out.changes.is_empty()
}

/// The editor of a body or message, filling the section.
fn code_editor(ui: &mut Ui, text: &mut String, templates: &Templates, json: bool) -> bool {
    let editor = CodeEditor::new(text)
        .label("Body")
        .line_numbers(true)
        .height(ui.available_height().max(size::CONTROL_HEIGHT * 6.0));
    let editor = if json {
        editor.highlighter(&templates.json)
    } else {
        editor.highlighter(&templates.plain)
    };
    ui.add(editor).changed()
}

fn body_section(ui: &mut Ui, body: &mut Body, templates: &Templates, gql: &mut GqlContext) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        let mut kind = BodyKind::of(body);
        ui.add(
            Dropdown::new("nexus.body.kind", &mut kind, BodyKind::ALL)
                .separators([BodyKind::ALL.len() - 1])
                .label("Body type")
                .width(KIND_WIDTH),
        );
        changed |= set_body_kind(body, kind);
        if let Body::Json { text } = body {
            let beautify = Button::ghost("Beautify").with(Tooltip::new("Re-indent the JSON"));
            if ui.add(beautify).clicked()
                && let Some(pretty) = editing::beautify_json(text)
            {
                *text = pretty;
                changed = true;
            }
        }
    });
    match body {
        Body::None => {
            EmptyState::new("No body")
                .icon(Glyph::FileText)
                .hint("Pick a body type to send a payload.")
                .show(ui);
        }
        Body::Json { text } => {
            if let Some(message) = editing::json_problem(text) {
                problem(ui, &message);
            }
            changed |= code_editor(ui, text, templates, true);
        }
        Body::Xml { text } => changed |= code_editor(ui, text, templates, false),
        Body::Text { text, content_type } => {
            Form::new().show(ui, |form| {
                form.row("Content-Type:", |ui| {
                    let mut value = content_type.clone().unwrap_or_default();
                    let edit = ui.add(
                        TextField::new(&mut value)
                            .name("Content-Type")
                            .placeholder("text/plain")
                            .width(FORM_FIELD_WIDTH),
                    );
                    if edit.changed() {
                        *content_type = (!value.trim().is_empty()).then_some(value);
                        changed = true;
                    }
                });
            });
            changed |= code_editor(ui, text, templates, false);
        }
        Body::FormUrlEncoded { fields } => {
            changed |= rows_table(ui, "form", fields, "Add field", templates);
        }
        Body::Multipart { fields } => changed |= multipart(ui, fields, templates),
        Body::Binary { path } => {
            Form::new().show(ui, |form| {
                form.row("File:", |ui| {
                    changed |= ui
                        .add(
                            templates.field(path).name("File").placeholder(
                                "path/to/file.bin (absolute or relative to the project)",
                            ),
                        )
                        .changed();
                });
            });
        }
        Body::GraphQl {
            query, variables, ..
        } => {
            changed |= crate::graphql_ui::body(
                ui,
                query,
                variables,
                templates,
                gql.schema.as_deref(),
                gql.loading,
                gql.error.as_deref(),
                &mut gql.fetch,
            );
        }
    }
    changed
}

/// The message composer of a WebSocket or gRPC request.
fn message_section(
    ui: &mut Ui,
    body: &mut Body,
    templates: &Templates,
    connected: bool,
    actions: &mut RequestActions,
) -> bool {
    let mut changed = editing::ensure_message_body(body);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        let mut kind = BodyKind::of(body);
        ui.add(
            Dropdown::new("nexus.message.kind", &mut kind, BodyKind::MESSAGE)
                .label("Message type")
                .width(KIND_WIDTH),
        );
        changed |= set_body_kind(body, kind);
        let tip = if connected {
            Tooltip::new("Send message").shortcut("Ctrl+Enter")
        } else {
            Tooltip::new("Connect first")
        };
        let send = Button::primary("Send message").enabled(connected).with(tip);
        actions.send_message |= ui.add(send).clicked();
    });
    match body {
        Body::Json { text } => changed |= code_editor(ui, text, templates, true),
        Body::Text { text, .. } => changed |= code_editor(ui, text, templates, false),
        _ => {}
    }
    changed
}

/// Text or file, as a multipart row's picker shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartKind {
    Text,
    File,
}

impl std::fmt::Display for PartKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            PartKind::Text => "Text",
            PartKind::File => "File",
        })
    }
}

/// Multipart rows: on, name, text or file, value or path, remove.
fn multipart(ui: &mut Ui, fields: &mut Vec<MultipartField>, templates: &Templates) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (i, field) in fields.iter_mut().enumerate() {
        ui.push_id(("multipart", i), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
                let n = i + 1;
                changed |= ui
                    .add(
                        Checkbox::new(&mut field.enabled, format!("Enabled, row {n}"))
                            .label_visible(false),
                    )
                    .changed();
                let fixed = PART_KIND_WIDTH + size::ICON_BUTTON + 4.0 * size::TOOLBAR_GAP;
                let width = ((ui.available_width() - fixed) / 2.0).max(size::CONTROL_HEIGHT);
                changed |= ui
                    .add(
                        templates
                            .field(&mut field.name)
                            .name(format!("Name, row {n}"))
                            .placeholder("name")
                            .width(width),
                    )
                    .changed();
                let mut kind = match field.value {
                    MultipartValue::File { .. } => PartKind::File,
                    MultipartValue::Text { .. } => PartKind::Text,
                };
                ui.add(
                    Dropdown::new(
                        ("nexus.multipart.kind", i),
                        &mut kind,
                        [PartKind::Text, PartKind::File],
                    )
                    .label(format!("Kind, row {n}"))
                    .width(PART_KIND_WIDTH),
                );
                changed |= editing::set_multipart_file(field, kind == PartKind::File);
                let (value, hint) = match &mut field.value {
                    MultipartValue::Text { value } => (value, "value"),
                    MultipartValue::File { path, .. } => (path, "path/to/file"),
                };
                changed |= ui
                    .add(
                        templates
                            .field(value)
                            .name(format!("Value, row {n}"))
                            .placeholder(hint)
                            .width(width),
                    )
                    .changed();
                if ui
                    .add(IconButton::new(Glyph::Close, format!("Remove, row {n}")).small(true))
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        });
    }
    if let Some(i) = remove {
        fields.remove(i);
        changed = true;
    }
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
        for (label, file) in [("Add text field", false), ("Add file", true)] {
            if ui.add(Button::secondary(label).icon(Glyph::Plus)).clicked() {
                fields.push(editing::new_multipart_field(file));
                changed = true;
            }
        }
    });
    changed
}

/// A labelled template field in a form. Secrets are masked unless they hold a variable.
fn form_field(
    form: &mut FormUi<'_>,
    templates: &Templates,
    label: &str,
    value: &mut String,
    hint: &str,
    secret: bool,
) -> bool {
    let masked = secret && !value.contains("{{");
    form.row(&format!("{label}:"), |ui| {
        let field = templates
            .field(value)
            .name(label)
            .placeholder(hint)
            .width(FORM_FIELD_WIDTH)
            .password(masked);
        ui.add(field).changed()
    })
}

fn auth_section(
    ui: &mut Ui,
    auth: &mut Auth,
    templates: &Templates,
    actions: &mut RequestActions,
) -> bool {
    let mut changed = false;
    Form::new().show(ui, |form| {
        form.row("Type:", |ui| {
            let mut kind = AuthKind::of(auth);
            ui.add(
                Dropdown::new("nexus.auth.kind", &mut kind, AuthKind::ALL)
                    .separators([2])
                    .label("Auth type")
                    .width(KIND_WIDTH),
            );
            changed |= set_auth_kind(auth, kind);
        });
        match auth {
            Auth::None => {}
            Auth::Inherit => {
                form.ui()
                    .label("This request uses the auth of its nearest folder that defines one.");
            }
            Auth::Basic { username, password } | Auth::Digest { username, password } => {
                changed |= form_field(form, templates, "Username", username, "username", false);
                changed |= form_field(form, templates, "Password", password, "password", true);
            }
            Auth::Bearer { token, prefix } => {
                changed |= form_field(
                    form,
                    templates,
                    "Token",
                    token,
                    "token or {{ token }}",
                    true,
                );
                changed |= form_field(form, templates, "Prefix", prefix, "Bearer", false);
            }
            Auth::ApiKey {
                key,
                value,
                location,
            } => {
                changed |= form_field(form, templates, "Key", key, "X-API-Key", false);
                changed |= form_field(form, templates, "Value", value, "value", true);
                form.radios("Add to", |ui| {
                    for (label, place) in [
                        ("Header", ApiKeyLocation::Header),
                        ("Query", ApiKeyLocation::Query),
                        ("Cookie", ApiKeyLocation::Cookie),
                    ] {
                        changed |= ui.add(Radio::new(location, place, label)).changed();
                    }
                });
            }
            Auth::AwsSigV4 {
                access_key_id,
                secret_access_key,
                session_token,
                region,
                service,
            } => {
                for (label, value, hint, secret) in [
                    ("Access key ID", access_key_id, "AKIA…", false),
                    ("Secret access key", secret_access_key, "secret", true),
                    ("Session token", session_token, "optional", true),
                    ("Region", region, "us-east-1", false),
                    ("Service", service, "execute-api, s3, …", false),
                ] {
                    changed |= form_field(form, templates, label, value, hint, secret);
                }
            }
            Auth::OAuth2(cfg) => changed |= oauth2(form, cfg, templates, actions),
        }
    });
    changed
}

fn oauth2(
    form: &mut FormUi<'_>,
    cfg: &mut OAuth2Config,
    templates: &Templates,
    actions: &mut RequestActions,
) -> bool {
    let mut changed = false;
    form.row("Grant type:", |ui| {
        let labels: Vec<&str> = OAUTH2_GRANTS.iter().map(|(_, l)| *l).collect();
        let current = OAUTH2_GRANTS
            .iter()
            .position(|(g, _)| *g == cfg.grant)
            .unwrap_or(0);
        let mut label = labels[current];
        ui.add(
            Dropdown::new("nexus.oauth.grant", &mut label, labels.clone())
                .label("Grant type")
                .width(FORM_FIELD_WIDTH),
        );
        if let Some((grant, _)) = OAUTH2_GRANTS.iter().find(|(_, l)| *l == label)
            && *grant != cfg.grant
        {
            cfg.grant = *grant;
            changed = true;
        }
    });
    let grant = cfg.grant;
    let browser = matches!(
        grant,
        OAuth2Grant::AuthorizationCode | OAuth2Grant::Implicit
    );
    let mut fields: Vec<(&str, &mut String, &str, bool)> = Vec::new();
    if browser {
        fields.push((
            "Authorization URL",
            &mut cfg.authorization_url,
            "https://…/authorize",
            false,
        ));
    }
    if grant != OAuth2Grant::Implicit {
        fields.push((
            "Access token URL",
            &mut cfg.access_token_url,
            "https://…/token",
            false,
        ));
    }
    fields.push(("Client ID", &mut cfg.client_id, "client id", false));
    if grant != OAuth2Grant::Implicit {
        fields.push((
            "Client secret",
            &mut cfg.client_secret,
            "client secret",
            true,
        ));
    }
    if grant == OAuth2Grant::Password {
        fields.push(("Username", &mut cfg.username, "username", false));
        fields.push(("Password", &mut cfg.password, "password", true));
    }
    if browser {
        fields.push((
            "Redirect URL",
            &mut cfg.redirect_url,
            "http://127.0.0.1:8765/callback",
            false,
        ));
    }
    fields.push(("Scope", &mut cfg.scope, "read write", false));
    fields.push(("Audience", &mut cfg.audience, "optional", false));
    for (label, value, hint, secret) in fields {
        changed |= form_field(form, templates, label, value, hint, secret);
    }
    if grant == OAuth2Grant::AuthorizationCode {
        changed |= form
            .add(
                Checkbox::new(&mut cfg.use_pkce, "Use PKCE")
                    .comment("Proof Key for Code Exchange, recommended for every client"),
            )
            .changed();
    }
    changed |= form
        .add(Checkbox::new(
            &mut cfg.credentials_in_header,
            "Send client credentials as Basic auth",
        ))
        .changed();
    form.fields(|ui| {
        actions.fetch_token |= ui.add(Button::primary("Fetch token")).clicked();
        match &cfg.token {
            Some(token) => {
                let short: String = token.access_token.chars().take(28).collect();
                let colors = Theme::colors(ui.ctx());
                ui.label(
                    RichText::new(format!("Token {short}…"))
                        .monospace()
                        .color(Tone::Success.color(&colors)),
                );
                if ui.add(Button::ghost("Clear")).clicked() {
                    cfg.token = None;
                    changed = true;
                }
            }
            None => {
                ui.add(FieldLabel::new("No token yet").enabled(false));
            }
        }
    });
    changed
}

fn tests_section(ui: &mut Ui, assertions: &mut Vec<Assertion>) -> bool {
    let mut changed = false;
    let mut remove = None;
    for (i, a) in assertions.iter_mut().enumerate() {
        ui.push_id(("assertion", i), |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = size::TOOLBAR_GAP;
                let n = i + 1;
                changed |= ui
                    .add(
                        Checkbox::new(&mut a.enabled, format!("Enabled, assertion {n}"))
                            .label_visible(false),
                    )
                    .changed();
                let mut kind = SourceKind::of(&a.source);
                ui.add(
                    Dropdown::new(("nexus.assert.source", i), &mut kind, SourceKind::ALL)
                        .label(format!("Source, assertion {n}"))
                        .width(ASSERTION_PICKER_WIDTH),
                );
                changed |= set_source_kind(&mut a.source, kind);
                if let Source::Header(arg) | Source::JsonPath(arg) = &mut a.source {
                    changed |= ui
                        .add(
                            TextField::new(arg)
                                .monospace(true)
                                .name(format!("Argument, assertion {n}"))
                                .width(ARGUMENT_WIDTH),
                        )
                        .changed();
                }
                let mut operator = a.operator;
                ui.add(
                    Dropdown::new(("nexus.assert.op", i), &mut operator, Operator::ALL)
                        .label(format!("Operator, assertion {n}"))
                        .width(ASSERTION_PICKER_WIDTH),
                );
                if operator != a.operator {
                    a.operator = operator;
                    changed = true;
                }
                if a.operator.needs_value() {
                    changed |= ui
                        .add(
                            TextField::new(&mut a.expected)
                                .name(format!("Expected, assertion {n}"))
                                .placeholder("expected")
                                .width(ARGUMENT_WIDTH),
                        )
                        .changed();
                }
                if ui
                    .add(IconButton::new(Glyph::Close, format!("Remove assertion {n}")).small(true))
                    .clicked()
                {
                    remove = Some(i);
                }
            });
        });
    }
    if let Some(i) = remove {
        assertions.remove(i);
        changed = true;
    }
    if assertions.is_empty() {
        EmptyState::new("No assertions")
            .icon(Glyph::Test)
            .hint("Assertions run after every response and in the collection runner.")
            .show(ui);
    }
    if ui
        .add(Button::secondary("Add assertion").icon(Glyph::Plus))
        .clicked()
    {
        assertions.push(editing::new_assertion());
        changed = true;
    }
    changed
}

fn settings_section(ui: &mut Ui, request: &mut Request) -> bool {
    let mut changed = false;
    let timeout_id = ui.id().with(("timeout", request.id.to_string()));
    Form::new().show(ui, |form| {
        let settings = &mut request.settings;
        for (label, value) in [
            ("Follow redirects", &mut settings.follow_redirects),
            ("Verify TLS certificates", &mut settings.verify_tls),
        ] {
            form.radios(label, |ui| {
                for (choice, v) in [
                    ("Workspace default", None),
                    ("On", Some(true)),
                    ("Off", Some(false)),
                ] {
                    changed |= ui.add(Radio::new(value, v, choice)).changed();
                }
            });
        }
        form.row("Timeout (ms):", |ui| {
            // The text stays as typed; the setting takes it once it is a number.
            let mut text = ui.data_mut(|d| {
                d.get_temp_mut_or_insert_with(timeout_id, || {
                    settings
                        .timeout_ms
                        .map(|t| t.to_string())
                        .unwrap_or_default()
                })
                .clone()
            });
            let edit = ui.add(
                TextField::new(&mut text)
                    .name("Timeout (ms)")
                    .placeholder("workspace default")
                    .width(ARGUMENT_WIDTH),
            );
            if edit.changed() {
                settings.timeout_ms = editing::parse_timeout_ms(&text);
                changed = true;
            }
            ui.data_mut(|d| d.insert_temp(timeout_id, text));
        });
        let mut cookies = !settings.disable_cookies;
        if form
            .add(Checkbox::new(&mut cookies, "Send and store cookies"))
            .changed()
        {
            settings.disable_cookies = !cookies;
            changed = true;
        }
        form.group("Description", |form| {
            changed |= form
                .ui()
                .add(
                    TextField::new(&mut request.description)
                        .multiline(4)
                        .name("Description")
                        .placeholder("Notes for your team (Markdown)"),
                )
                .changed();
        });
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_http_core::model::Method;

    #[test]
    fn tab_labels_count_enabled_rows_and_name_the_body() {
        let mut r = Request::new("r", Method::Post, "https://x");
        r.headers.push(KeyValue::new("a", "1"));
        r.body = Body::Json { text: "{}".into() };
        assert_eq!(tab_label(RequestTab::Headers, &r), "Headers 1");
        assert_eq!(tab_label(RequestTab::Params, &r), "Params");
        assert_eq!(tab_label(RequestTab::Body, &r), "JSON");
        r.protocol = Protocol::WebSocket;
        assert_eq!(tab_label(RequestTab::Body, &r), "Message");
    }
}
