//! Nexus: the standalone desktop API client of the Archipel suite.
//!
//! ```bash
//! cargo run -p nexus -- [DIR]       # open DIR/.nexus
//! cargo run -p nexus -- --demo      # seeded demo + local API
//! ```
//!
//! Demo options (environment): `NEXUS_LIGHT=1` light theme, `NEXUS_SEND=1` send
//! the selected request on start, `NEXUS_SELECT=<request name>`,
//! `NEXUS_REQUEST_TAB=<tab>`, `NEXUS_RESPONSE_TAB=<tab>`,
//! `NEXUS_OVERLAY=env|switcher|import|export|code`, `NEXUS_RUN=<top-level folder or empty>`
//! opens the collection runner and starts a run.
//!
//! `NEXUS_PERF=<out.json>` measures start, frame time, idle CPU and memory and exits
//! (see `perf.rs` and `docs/performance.md`; use `cargo xtask perf`).

mod perf;

use eframe::egui;
use nexus_http::NexusApp;
use nexus_http::state::{NexusState, RequestTab, ResponseTab};
use nexus_http::views::overlays::Overlay;
use nexus_http_core::assertions::{Assertion, Operator, Source};
use nexus_http_core::model::*;
use serde_json::json;
use std::path::PathBuf;

fn main() -> eframe::Result {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let demo = args.iter().any(|a| a == "--demo");
    let mut app = if demo {
        let port = demo_server();
        let ws_port = demo_websocket_server();
        let sse_port = demo_sse_server();
        let dir = std::env::temp_dir().join(format!("nexus-demo-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::write(dir.join("invoice.pdf"), b"%PDF-1.4\n% demo invoice\n");
        let mut state = NexusState::open(&dir);
        state.replace_workspace(demo_workspace(port, ws_port, sse_port));
        NexusApp::with_state(state)
    } else {
        let dir = args
            .first()
            .map(PathBuf::from)
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default());
        NexusApp::open(&dir)
    };
    apply_demo_env(&mut app);
    let light = std::env::var_os("NEXUS_LIGHT").is_some();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Nexus")
            .with_inner_size([1280.0, 800.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Nexus",
        options,
        Box::new(move |cc| {
            NexusApp::install(&cc.egui_ctx, !light);
            Ok(Box::new(Host {
                app,
                sent: false,
                messages: 0,
                last_message: 0.0,
                perf: perf::Perf::from_env(),
            }))
        }),
    )
}

struct Host {
    app: NexusApp,
    sent: bool,
    /// Demo: WebSocket messages still to send once connected.
    messages: u32,
    last_message: f64,
    perf: Option<perf::Perf>,
}

impl eframe::App for Host {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if let Some(perf) = &mut self.perf {
            perf.update(ctx, &mut self.app);
        }
        if !self.sent {
            self.sent = true;
            if std::env::var_os("NEXUS_SEND").is_some() {
                self.app.send_selected();
            }
            if std::env::var_os("NEXUS_SCHEMA").is_some()
                && let Some(id) = self.app.state.selected.clone()
            {
                self.app.state.fetch_schema(&id);
            }
            if let Ok(n) = std::env::var("NEXUS_MESSAGES") {
                self.messages = n.parse().unwrap_or(0);
            }
            if let Ok(scope) = std::env::var("NEXUS_RUN") {
                let folder = self
                    .app
                    .state
                    .workspace
                    .items
                    .iter()
                    .find(|i| i.name() == scope)
                    .map(|i| i.id().clone());
                self.app.open_runner(folder);
                self.app.start_run();
            }
        }
        let now = ctx.input(|i| i.time);
        if self.messages > 0
            && now - self.last_message > 0.6
            && let Some(id) = self.app.state.selected.clone()
            && self.app.state.stream_connected(&id)
        {
            self.messages -= 1;
            self.last_message = now;
            self.app.state.send_stream_message(&id);
            ctx.request_repaint();
        }
        self.app.window(ctx);
    }
}

fn apply_demo_env(app: &mut NexusApp) {
    if let Ok(name) = std::env::var("NEXUS_SELECT")
        && let Some(id) = app
            .state
            .workspace
            .requests()
            .iter()
            .find(|(_, r)| r.name == name)
            .map(|(_, r)| r.id.clone())
    {
        app.state.select(id);
    }
    if let Ok(tab) = std::env::var("NEXUS_REQUEST_TAB")
        && let Some(t) = RequestTab::ALL
            .iter()
            .find(|t| t.label().eq_ignore_ascii_case(&tab))
    {
        app.state.request_tab = *t;
    }
    if let Ok(tab) = std::env::var("NEXUS_RESPONSE_TAB")
        && let Some(t) = ResponseTab::ALL
            .iter()
            .find(|t| t.label().eq_ignore_ascii_case(&tab))
    {
        app.state.response_tab = *t;
    }
    let overlay = match std::env::var("NEXUS_OVERLAY").as_deref() {
        Ok("env") => Overlay::Environments,
        Ok("switcher") => Overlay::Switcher,
        Ok("import") => Overlay::Import,
        Ok("export") => Overlay::Export,
        Ok("code") => Overlay::Code,
        _ => Overlay::None,
    };
    if overlay != Overlay::None {
        app.open_overlay(overlay);
    }
}

fn demo_workspace(port: u16, ws_port: u16, sse_port: u16) -> Workspace {
    let mut ws = Workspace::new("Acme Store API");
    ws.base_environment = ws
        .base_environment
        .clone()
        .with("base_url", format!("http://127.0.0.1:{port}"))
        .with("api_version", "v1")
        .with("ws_url", format!("ws://127.0.0.1:{ws_port}"))
        .with("events_url", format!("http://127.0.0.1:{sse_port}"));
    let mut local = Environment::new("Local")
        .with("user", "ada@acme.test")
        .with("password", "demo");
    local.color = Some("#4E9D6C".into());
    local.secrets.push("password".into());
    let mut staging = Environment::new("Staging")
        .with("base_url", "https://staging.acme.test")
        .with("user", "qa@acme.test")
        .with("password", "");
    staging.color = Some("#D59637".into());
    ws.active_environment = Some(local.id.clone());
    ws.environments = vec![local, staging];

    let mut login = Request::new(
        "Login",
        Method::Post,
        "{{ base_url }}/{{ api_version }}/auth/login",
    )
    .with_body(Body::Json {
        text: "{\n  \"email\": \"{{ user }}\",\n  \"password\": \"{{ password }}\"\n}".into(),
    });
    login
        .assertions
        .push(Assertion::new(Source::Status, Operator::Equals, "200"));
    login.assertions.push(Assertion::new(
        Source::JsonPath("$.token".into()),
        Operator::Exists,
        "",
    ));
    let login_id = login.id.clone();
    let me = Request::new(
        "Current user",
        Method::Get,
        "{{ base_url }}/{{ api_version }}/me",
    )
    .with_auth(Auth::Bearer {
        token: format!("{{% response 'body', '{login_id}', '$.token' %}}"),
        prefix: "Bearer".into(),
    });
    let mut auth = Folder::new("Auth");
    auth.children = vec![Item::Request(login), Item::Request(me)];

    let mut products = Folder::new("Products");
    products
        .headers
        .push(KeyValue::new("Accept", "application/json"));
    let mut list = Request::new(
        "List products",
        Method::Get,
        "{{ base_url }}/{{ api_version }}/products",
    )
    .param("category", "keyboards")
    .param("limit", "20")
    .header("X-Request-Id", "{% uuid %}");
    list.params.push(KeyValue {
        enabled: false,
        ..KeyValue::new("sort", "-price")
    });
    list.assertions
        .push(Assertion::new(Source::Status, Operator::Equals, "200"));
    list.assertions.push(Assertion::new(
        Source::JsonPath("$.items[0].sku".into()),
        Operator::Equals,
        "KB-001",
    ));
    list.assertions.push(Assertion::new(
        Source::ResponseTimeMs,
        Operator::LessThan,
        "500",
    ));
    let mut get = Request::new(
        "Get product",
        Method::Get,
        "{{ base_url }}/{{ api_version }}/products/KB-001",
    );
    get.assertions
        .push(Assertion::new(Source::Status, Operator::Equals, "200"));
    get.assertions.push(Assertion::new(
        Source::JsonPath("$.price".into()),
        Operator::LessThan,
        "100",
    ));
    let create = Request::new("Create product", Method::Post, "{{ base_url }}/{{ api_version }}/products")
        .with_body(Body::Json {
            text: "{\n  \"sku\": \"KB-042\",\n  \"name\": \"Split Keyboard\",\n  \"price\": 189.0,\n  \"tags\": [\"ergonomic\", \"wireless\"],\n  \"in_stock\": true\n}".into(),
        })
        .with_auth(Auth::Inherit);
    let update = Request::new(
        "Update price",
        Method::Patch,
        "{{ base_url }}/{{ api_version }}/products/KB-042",
    )
    .with_body(Body::Json {
        text: "{ \"price\": 169.0 }".into(),
    });
    let delete = Request::new(
        "Delete product",
        Method::Delete,
        "{{ base_url }}/{{ api_version }}/products/KB-042",
    );
    products.children = [list, get, create, update, delete]
        .into_iter()
        .map(Item::Request)
        .collect();

    let mut orders = Folder::new("Orders");
    orders.children.push(Item::Request(
        Request::new(
            "Upload invoice",
            Method::Post,
            "{{ base_url }}/{{ api_version }}/orders/1001/invoice",
        )
        .with_body(Body::Multipart {
            fields: vec![
                MultipartField {
                    name: "note".into(),
                    value: MultipartValue::Text { value: "Q3".into() },
                    enabled: true,
                },
                MultipartField {
                    name: "file".into(),
                    value: MultipartValue::File {
                        path: "invoice.pdf".into(),
                        content_type: None,
                    },
                    enabled: true,
                },
            ],
        }),
    ));
    orders.children.push(Item::Request(
        Request::new(
            "Search orders",
            Method::Get,
            "{{ base_url }}/{{ api_version }}/orders",
        )
        .param("status", "open"),
    ));

    let graphql = Request::new("Inventory (GraphQL)", Method::Post, "{{ base_url }}/graphql").with_body(Body::GraphQl {
        query: "query Inventory($first: Int!) {\n  products(first: $first) {\n    sku\n    name\n    stock\n  }\n}".into(),
        variables: "{ \"first\": 3 }".into(),
        operation_name: None,
    });
    let health = Request::new("Health check", Method::Get, "{{ base_url }}/health");
    let mut live = Folder::new("Live");
    let mut prices = Request::new(
        "Price ticker",
        Method::Get,
        "{{ events_url }}/v1/prices/stream",
    );
    prices.protocol = Protocol::Sse;
    let mut orders_ws = Request::new("Order updates", Method::Get, "{{ ws_url }}/orders")
        .header("Authorization", "Bearer demo-token")
        .with_body(Body::Json {
            text: "{\n  \"type\": \"subscribe\",\n  \"channel\": \"orders\",\n  \"at\": \"{% now 'iso' %}\"\n}".into(),
        });
    orders_ws.protocol = Protocol::WebSocket;
    live.children = vec![Item::Request(prices), Item::Request(orders_ws)];
    ws.items = vec![
        Item::Folder(auth),
        Item::Folder(products),
        Item::Folder(orders),
        Item::Folder(live),
        Item::Request(graphql),
        Item::Request(health),
    ];
    ws
}

/// A small local API so the demo shows real responses.
fn demo_server() -> u16 {
    let server = tiny_http::Server::http("127.0.0.1:0").expect("bind demo server");
    let port = server.server_addr().to_ip().map(|a| a.port()).unwrap_or(0);
    std::thread::spawn(move || {
        for mut request in server.incoming_requests() {
            let mut body = String::new();
            let _ = std::io::Read::read_to_string(request.as_reader(), &mut body);
            let path = request.url().split('?').next().unwrap_or("").to_owned();
            let (status, value) = match (request.method().as_str(), path.as_str()) {
                ("POST", "/v1/auth/login") => (
                    200,
                    json!({"token": "demo-session-token", "expires_in": 3600, "user": {"id": 7, "email": "ada@acme.test"}}),
                ),
                ("GET", "/v1/me") => (
                    200,
                    json!({"id": 7, "email": "ada@acme.test", "roles": ["admin", "buyer"]}),
                ),
                ("GET", "/v1/products") => (
                    200,
                    json!({
                        "items": [
                            {"sku": "KB-001", "name": "Mechanical Keyboard", "price": 129.0, "currency": "EUR", "in_stock": true, "tags": ["tactile", "rgb"]},
                            {"sku": "KB-007", "name": "Low-profile Keyboard", "price": 99.5, "currency": "EUR", "in_stock": false, "tags": ["slim"]},
                            {"sku": "KB-013", "name": "Travel Keyboard", "price": 74.0, "currency": "EUR", "in_stock": true, "tags": []}
                        ],
                        "page": {"limit": 20, "next": null, "total": 3}
                    }),
                ),
                ("GET", p) if p.starts_with("/v1/products/") => (
                    200,
                    json!({"sku": "KB-001", "name": "Mechanical Keyboard", "price": 129.0}),
                ),
                ("POST", "/v1/products") => (201, serde_json::from_str(&body).unwrap_or(json!({}))),
                ("POST", p) if p.ends_with("/invoice") => (201, json!({"received": true})),
                ("PATCH", _) => (200, json!({"sku": "KB-042", "price": 169.0})),
                ("DELETE", _) => (204, json!(null)),
                ("POST", "/graphql") if body.contains("__schema") => (200, demo_schema()),
                ("POST", "/graphql") => (
                    200,
                    json!({"data": {"products": [{"sku": "KB-001", "name": "Mechanical Keyboard", "stock": 12}]}}),
                ),
                ("GET", "/health") => (200, json!({"status": "ok"})),
                _ => (404, json!({"error": "not_found", "path": path})),
            };
            let text = if value.is_null() {
                String::new()
            } else {
                value.to_string()
            };
            let response = tiny_http::Response::from_string(text)
                .with_status_code(status)
                .with_header(
                    tiny_http::Header::from_bytes(
                        &b"Content-Type"[..],
                        &b"application/json; charset=utf-8"[..],
                    )
                    .expect("static header"),
                )
                .with_header(
                    tiny_http::Header::from_bytes(
                        &b"Set-Cookie"[..],
                        &b"session=7f3a9c; Path=/; HttpOnly"[..],
                    )
                    .expect("static header"),
                )
                .with_header(
                    tiny_http::Header::from_bytes(&b"X-Request-Id"[..], &b"c0ffee-42"[..])
                        .expect("static header"),
                );
            let _ = request.respond(response);
        }
    });
    port
}

/// Server-Sent Events over a raw socket so every event is flushed at once.
fn demo_sse_server() -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind demo sse");
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let mut head = Vec::new();
                let mut byte = [0u8; 1];
                while !head.ends_with(b"\r\n\r\n")
                    && stream.read(&mut byte).map(|n| n == 1).unwrap_or(false)
                {
                    head.push(byte[0]);
                }
                let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\n\r\n: price feed\n\n");
                let mut ticker = Ticker {
                    n: 0,
                    pending: Vec::new(),
                };
                let mut buf = [0u8; 512];
                loop {
                    let n = ticker.read(&mut buf).unwrap_or(0);
                    if n == 0 || stream.write_all(&buf[..n]).is_err() || stream.flush().is_err() {
                        return;
                    }
                }
            });
        }
    });
    port
}

/// Endless SSE body: a price tick every 400 ms.
struct Ticker {
    n: u32,
    pending: Vec<u8>,
}

impl std::io::Read for Ticker {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.pending.is_empty() {
            if self.n > 0 {
                std::thread::sleep(std::time::Duration::from_millis(400));
            }
            self.n += 1;
            let skus = ["KB-001", "KB-007", "KB-013"];
            let sku = skus[(self.n as usize) % 3];
            let price = 99.0 + (self.n * 7 % 40) as f64 + 0.5;
            let event = if self.n.is_multiple_of(5) {
                format!(
                    "event: stock\nid: {}\ndata: {{\"sku\":\"{sku}\",\"stock\":{}}}\n\n",
                    self.n,
                    self.n % 13
                )
            } else {
                format!(
                    "event: price\nid: {}\ndata: {{\"sku\":\"{sku}\",\"price\":{price:.1},\"currency\":\"EUR\"}}\n\n",
                    self.n
                )
            };
            self.pending = event.into_bytes();
        }
        let n = buf.len().min(self.pending.len());
        buf[..n].copy_from_slice(&self.pending[..n]);
        self.pending.drain(..n);
        Ok(n)
    }
}

/// WebSocket server that acknowledges messages and pushes order updates.
fn demo_websocket_server() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind demo websocket");
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || {
                let Ok(mut ws) = tungstenite::accept(stream) else {
                    return;
                };
                let _ = ws
                    .get_ref()
                    .set_read_timeout(Some(std::time::Duration::from_millis(700)));
                let mut order = 1000;
                loop {
                    match ws.read() {
                        Ok(tungstenite::Message::Text(t)) => {
                            let reply = format!(
                                "{{\"type\":\"ack\",\"received\":{}}}",
                                if t.trim_start().starts_with('{') {
                                    t.to_string()
                                } else {
                                    format!("{:?}", t.as_str())
                                }
                            );
                            if ws.send(tungstenite::Message::text(reply)).is_err() {
                                return;
                            }
                        }
                        Ok(tungstenite::Message::Close(_)) => return,
                        Ok(_) => {}
                        Err(tungstenite::Error::Io(e))
                            if matches!(
                                e.kind(),
                                std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                            ) =>
                        {
                            order += 1;
                            let status = ["paid", "packed", "shipped"][order % 3];
                            let msg = format!(
                                "{{\"type\":\"order.updated\",\"order\":{order},\"status\":\"{status}\"}}"
                            );
                            if ws.send(tungstenite::Message::text(msg)).is_err() {
                                return;
                            }
                        }
                        Err(_) => return,
                    }
                }
            });
        }
    });
    port
}

/// Introspection result of the demo GraphQL API.
fn demo_schema() -> serde_json::Value {
    let t = |kind: &str, name: &str| json!({"kind": kind, "name": name, "ofType": null});
    let nn = |inner: serde_json::Value| json!({"kind": "NON_NULL", "name": null, "ofType": inner});
    let list = |inner: serde_json::Value| json!({"kind": "LIST", "name": null, "ofType": inner});
    let field = |name: &str, ty: serde_json::Value, args: serde_json::Value, description: &str| json!({"name": name, "type": ty, "args": args, "description": description, "isDeprecated": false});
    let arg = |name: &str, ty: serde_json::Value| json!({"name": name, "type": ty});
    json!({"data": {"__schema": {
        "queryType": {"name": "Query"},
        "mutationType": {"name": "Mutation"},
        "subscriptionType": null,
        "types": [
            {"kind": "OBJECT", "name": "Query", "fields": [
                field("products", nn(list(nn(t("OBJECT", "Product")))), json!([arg("first", t("SCALAR", "Int")), arg("category", t("SCALAR", "String"))]), "Products, newest first"),
                field("product", t("OBJECT", "Product"), json!([arg("sku", nn(t("SCALAR", "String")))]), "One product by SKU"),
                field("orders", nn(list(nn(t("OBJECT", "Order")))), json!([arg("status", t("ENUM", "OrderStatus"))]), "Orders of the current user"),
                field("me", t("OBJECT", "User"), json!([]), "The signed-in user")
            ]},
            {"kind": "OBJECT", "name": "Mutation", "fields": [
                field("createProduct", t("OBJECT", "Product"), json!([arg("input", nn(t("INPUT_OBJECT", "ProductInput")))]), "Adds a product"),
                field("cancelOrder", t("OBJECT", "Order"), json!([arg("id", nn(t("SCALAR", "ID")))]), "Cancels an open order")
            ]},
            {"kind": "OBJECT", "name": "Product", "fields": [
                field("sku", nn(t("SCALAR", "String")), json!([]), "Stock keeping unit"),
                field("name", nn(t("SCALAR", "String")), json!([]), ""),
                field("price", t("SCALAR", "Float"), json!([]), "Gross price in EUR"),
                field("stock", t("SCALAR", "Int"), json!([]), "Units in the warehouse"),
                field("supplier", t("OBJECT", "Supplier"), json!([]), ""),
                field("tags", nn(list(nn(t("SCALAR", "String")))), json!([]), "")
            ]},
            {"kind": "OBJECT", "name": "Supplier", "fields": [
                field("name", t("SCALAR", "String"), json!([]), ""),
                field("country", t("SCALAR", "String"), json!([]), "ISO 3166 code")
            ]},
            {"kind": "OBJECT", "name": "Order", "fields": [
                field("id", nn(t("SCALAR", "ID")), json!([]), ""),
                field("status", t("ENUM", "OrderStatus"), json!([]), ""),
                field("items", nn(list(nn(t("OBJECT", "Product")))), json!([]), "")
            ]},
            {"kind": "OBJECT", "name": "User", "fields": [
                field("id", nn(t("SCALAR", "ID")), json!([]), ""),
                field("email", t("SCALAR", "String"), json!([]), ""),
                field("roles", nn(list(nn(t("SCALAR", "String")))), json!([]), "")
            ]},
            {"kind": "INPUT_OBJECT", "name": "ProductInput", "fields": null, "inputFields": [
                {"name": "sku", "type": nn(t("SCALAR", "String"))},
                {"name": "name", "type": nn(t("SCALAR", "String"))}
            ]},
            {"kind": "ENUM", "name": "OrderStatus", "fields": null, "enumValues": [{"name": "OPEN"}, {"name": "SHIPPED"}, {"name": "CANCELLED"}]}
        ]
    }}})
}
