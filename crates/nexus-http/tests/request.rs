//! Behavior and audit tests for the request editor.

use archipel_kit::testing::{self, Queryable, Role};
use archipel_kit::{Theme, egui};
use nexus_http::state::{NexusState, RequestTab};
use nexus_http::views::request::{self, RequestActions};
use nexus_http_core::model::{
    Auth, Body, Environment, Id, Item, KeyValue, Method, MultipartField, MultipartValue, Protocol,
    Request, Workspace,
};

struct Fixture {
    state: NexusState,
    id: Id,
    actions: RequestActions,
}

fn fixture() -> Fixture {
    let mut workspace = Workspace::new("Test");
    let mut env = Environment::new("Local");
    env.data
        .insert("base_url".into(), "http://localhost:8080".into());
    env.data.insert("user_id".into(), "7".into());
    env.data.insert("token".into(), "secret".into());
    env.secrets = vec!["token".into()];
    workspace.base_environment = env;
    let mut request = Request::new("Items", Method::Get, "{{base_url}}/items");
    request.params = vec![KeyValue::new("page", "1")];
    request.headers = vec![KeyValue::new("accept", "application/json")];
    let id = request.id.clone();
    workspace.items.push(Item::Request(request));
    let mut state = NexusState::in_memory(workspace);
    state.selected = Some(id.clone());
    Fixture {
        state,
        id,
        actions: RequestActions::default(),
    }
}

fn harness(f: Fixture, theme: Theme) -> testing::Harness<'static, Fixture> {
    let mut h = testing::harness_state(theme, f, |ui, f| {
        request::show(ui, &mut f.state, &f.id, &mut f.actions);
    });
    h.set_size(egui::vec2(900.0, 640.0));
    h.run_ok();
    h
}

fn req(h: &testing::Harness<'_, Fixture>) -> Request {
    let f = h.state();
    f.state.workspace.find_request(&f.id).unwrap().1.clone()
}

fn set(f: &mut Fixture, edit: impl FnOnce(&mut Request)) {
    edit(f.state.request_mut(&f.id.clone()).unwrap());
}

#[test]
fn audit_every_tab_in_both_themes() {
    for theme in [Theme::dark(), Theme::light()] {
        for tab in RequestTab::ALL {
            let mut f = fixture();
            f.state.request_tab = tab;
            let mut h = harness(f, theme.clone());
            let audit = testing::audit(&mut h);
            assert!(audit.is_clean(), "{tab:?}: {:?}", audit.findings);
        }
    }
}

#[test]
fn audit_every_body_kind_and_auth() {
    let bodies = [
        Body::None,
        Body::Json {
            text: "{\"a\": 1}".into(),
        },
        Body::Xml {
            text: "<a/>".into(),
        },
        Body::Text {
            text: "hi".into(),
            content_type: None,
        },
        Body::FormUrlEncoded {
            fields: vec![KeyValue::new("a", "1")],
        },
        Body::Multipart {
            fields: vec![
                MultipartField {
                    name: "t".into(),
                    value: MultipartValue::Text { value: "v".into() },
                    enabled: true,
                },
                MultipartField {
                    name: "f".into(),
                    value: MultipartValue::File {
                        path: "a.bin".into(),
                        content_type: None,
                    },
                    enabled: true,
                },
            ],
        },
        Body::Binary {
            path: "a.bin".into(),
        },
        Body::GraphQl {
            query: "query { me }".into(),
            variables: "{}".into(),
            operation_name: None,
        },
    ];
    for body in bodies {
        let mut f = fixture();
        f.state.request_tab = RequestTab::Body;
        set(&mut f, |r| r.body = body.clone());
        let mut h = harness(f, Theme::dark());
        testing::audit(&mut h).assert_clean();
    }
}

#[test]
fn audit_other_protocols() {
    for protocol in [Protocol::WebSocket, Protocol::Sse, Protocol::Grpc] {
        for tab in [RequestTab::Body, RequestTab::Params] {
            let mut f = fixture();
            f.state.request_tab = tab;
            set(&mut f, |r| r.protocol = protocol);
            let mut h = harness(f, Theme::dark());
            testing::audit(&mut h).assert_clean();
        }
    }
}

#[test]
fn typing_a_url_marks_it_dirty_and_enter_sends() {
    let mut h = harness(fixture(), Theme::dark());
    let url = h.get_by_label("URL");
    url.focus();
    url.type_text("/x");
    h.run_ok();
    assert_eq!(req(&h).url, "{{base_url}}/items/x");
    h.key_press(egui::Key::Enter);
    h.run_ok();
    assert!(h.state().actions.send);
}

#[test]
fn send_button_requests_a_send() {
    let mut h = harness(fixture(), Theme::dark());
    h.get_by_role_and_label(Role::Button, "Send").click();
    h.run_ok();
    assert!(h.state().actions.send);
}

#[test]
fn picking_a_method_changes_the_request() {
    let mut h = harness(fixture(), Theme::dark());
    h.set_size(egui::vec2(900.0, 700.0));
    h.run_ok();
    h.get_by_role(Role::ComboBox).click();
    h.run_ok();
    h.get_by_label("POST").click();
    h.run_ok();
    assert_eq!(req(&h).method, Method::Post);
}

#[test]
fn autocomplete_inserts_the_variable() {
    let mut f = fixture();
    set(&mut f, |r| r.url = String::new());
    let mut h = harness(f, Theme::dark());
    let url = h.get_by_label("URL");
    url.focus();
    url.type_text("{{us");
    h.run_ok();
    h.key_press(egui::Key::Enter);
    h.run_ok();
    assert_eq!(req(&h).url, "{{user_id}}");
    assert!(!h.state().actions.send, "Enter accepted the suggestion");
}

#[test]
fn params_add_toggle_and_delete() {
    let mut h = harness(fixture(), Theme::dark());
    let key = h.get_by_label("Key, new row");
    key.focus();
    key.type_text("size");
    h.run_ok();
    assert_eq!(req(&h).params.len(), 2);
    assert_eq!(req(&h).params[1].name, "size");
    h.get_by_label("Enabled, row 2").click();
    h.run_ok();
    assert!(!req(&h).params[1].enabled);
    h.get_by_label("Remove, row 1").click();
    h.run_ok();
    let params = req(&h).params;
    assert_eq!(params.len(), 1);
    assert_eq!(params[0].name, "size");
}

#[test]
fn headers_keep_their_description() {
    let mut f = fixture();
    f.state.request_tab = RequestTab::Headers;
    set(&mut f, |r| r.headers[0].description = "note".into());
    let mut h = harness(f, Theme::dark());
    let value = h.get_by_label("Value, row 1");
    value.focus();
    value.type_text("x");
    h.run_ok();
    let headers = req(&h).headers;
    assert_eq!(headers[0].value, "application/jsonx");
    assert_eq!(headers[0].description, "note");
}

#[test]
fn editing_a_json_body() {
    let mut f = fixture();
    f.state.request_tab = RequestTab::Body;
    set(&mut f, |r| {
        r.body = Body::Json {
            text: String::new(),
        }
    });
    let mut h = harness(f, Theme::dark());
    let editor = h.get_by_label("JSON body");
    editor.focus();
    editor.type_text("{\"a\": 1}");
    h.run_ok();
    let Body::Json { text } = req(&h).body else {
        panic!("not json")
    };
    assert!(text.starts_with("{\"a\": 1}"), "{text}");
}

#[test]
fn switching_the_auth_type_shows_its_form() {
    let mut f = fixture();
    f.state.request_tab = RequestTab::Auth;
    let mut h = harness(f, Theme::dark());
    h.set_size(egui::vec2(900.0, 700.0));
    h.run_ok();
    h.get_by_role(Role::ComboBox).click();
    h.run_ok();
    h.get_by_label("Bearer").click();
    h.run_ok();
    let token = h.get_by_label("Token");
    token.focus();
    token.type_text("abc");
    h.run_ok();
    match req(&h).auth {
        Auth::Bearer { token, .. } => assert_eq!(token, "abc"),
        other => panic!("{other:?}"),
    }
    testing::audit(&mut h).assert_clean();
}

#[test]
fn switching_tabs_updates_the_state() {
    let mut h = harness(fixture(), Theme::dark());
    h.get_by_label("Headers (1)").click();
    h.run_ok();
    assert_eq!(h.state().state.request_tab, RequestTab::Headers);
}
