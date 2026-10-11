//! UI-independent controller: the open workspace, selection, tree editing,
//! background sending and persistence. Views read and mutate this state; all
//! policy lives here so it is unit-testable without rendering.

use nexus_http_core::assertions::{self, AssertionResult};
use nexus_http_core::cookies::CookieJar;
use nexus_http_core::history::History;
use nexus_http_core::model::{Folder, Id, Item, Method, Request, Workspace};
use nexus_http_core::storage::Store;
use nexus_http_core::{Client, Response, execute};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

/// Save at most this often while the user types.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(600);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RequestTab {
    #[default]
    Params,
    Body,
    Auth,
    Headers,
    Tests,
    Settings,
}

impl RequestTab {
    pub const ALL: [RequestTab; 6] = [
        RequestTab::Params,
        RequestTab::Body,
        RequestTab::Auth,
        RequestTab::Headers,
        RequestTab::Tests,
        RequestTab::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            RequestTab::Params => "Params",
            RequestTab::Body => "Body",
            RequestTab::Auth => "Auth",
            RequestTab::Headers => "Headers",
            RequestTab::Tests => "Tests",
            RequestTab::Settings => "Settings",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ResponseTab {
    #[default]
    Preview,
    Raw,
    Headers,
    Cookies,
    Timeline,
    Tests,
}

impl ResponseTab {
    pub const ALL: [ResponseTab; 6] = [
        ResponseTab::Preview,
        ResponseTab::Raw,
        ResponseTab::Headers,
        ResponseTab::Cookies,
        ResponseTab::Timeline,
        ResponseTab::Tests,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ResponseTab::Preview => "Preview",
            ResponseTab::Raw => "Raw",
            ResponseTab::Headers => "Headers",
            ResponseTab::Cookies => "Cookies",
            ResponseTab::Timeline => "Timeline",
            ResponseTab::Tests => "Tests",
        }
    }
}

/// Result of the latest send of a request.
#[derive(Debug, Clone)]
pub enum Outcome {
    Response(Box<Response>, Vec<AssertionResult>),
    Error(String),
}

/// A streaming connection (WebSocket / SSE) and the events it produced.
#[derive(Default)]
pub struct StreamState {
    pub session: Option<nexus_http_core::stream::StreamSession>,
    pub events: Vec<nexus_http_core::stream::StreamEvent>,
}

impl StreamState {
    pub fn connected(&self) -> bool {
        self.session.as_ref().is_some_and(|s| !s.is_finished())
    }
}

/// A fetched (or loading) GraphQL schema for a request.
#[derive(Default)]
pub struct SchemaState {
    pub schema: Option<Arc<nexus_http_core::graphql::Schema>>,
    pub error: Option<String>,
    rx: Option<Receiver<Result<nexus_http_core::graphql::Schema, String>>>,
}

impl SchemaState {
    pub fn loading(&self) -> bool {
        self.rx.is_some()
    }
}

/// The methods of a gRPC request's schema (loaded from a `.proto` file or by
/// reflection).
#[derive(Default)]
pub struct GrpcSchemaState {
    pub methods: Vec<nexus_http_core::grpc::MethodInfo>,
    pub error: Option<String>,
    rx: Option<Receiver<Result<Vec<nexus_http_core::grpc::MethodInfo>, String>>>,
}

impl GrpcSchemaState {
    pub fn loading(&self) -> bool {
        self.rx.is_some()
    }
}

/// Events kept per stream before the oldest are dropped.
const MAX_STREAM_EVENTS: usize = 5_000;

struct InFlight {
    id: Id,
    cancel: Arc<AtomicBool>,
    started: Instant,
    rx: Receiver<(Result<Response, String>, CookieJar)>,
}

pub struct NexusState {
    store: Option<Store>,
    base_dir: PathBuf,
    pub workspace: Workspace,
    pub jar: CookieJar,
    pub history: History,
    pub selected: Option<Id>,
    /// Most recently used requests first, for the request switcher.
    pub recent: Vec<Id>,
    pub request_tab: RequestTab,
    pub response_tab: ResponseTab,
    pub outcomes: std::collections::HashMap<Id, Outcome>,
    pub filter: String,
    /// Folder ids collapsed in the sidebar.
    pub collapsed: std::collections::HashSet<Id>,
    /// Transient message for the status line.
    pub notice: Option<(String, Instant)>,
    inflight: Option<InFlight>,
    pub streams: std::collections::HashMap<Id, StreamState>,
    pub schemas: std::collections::HashMap<Id, SchemaState>,
    pub grpc_schemas: std::collections::HashMap<Id, GrpcSchemaState>,
    dirty_since: Option<Instant>,
    history_dirty: bool,
}

impl NexusState {
    /// Opens the first workspace stored in `dir/.nexus`, or creates one.
    pub fn open(dir: &Path) -> Self {
        let store = Store::in_dir(dir);
        let mut notice = None;
        let workspace = match store.list() {
            Ok(list) => match list.first() {
                Some(summary) => store.load(&summary.id).unwrap_or_else(|e| {
                    notice = Some(format!("Could not load workspace: {e}"));
                    Workspace::new("My Workspace")
                }),
                None => Workspace::new("My Workspace"),
            },
            Err(e) => {
                notice = Some(format!("Could not read {}: {e}", store.root().display()));
                Workspace::new("My Workspace")
            }
        };
        let jar = store.load_cookies(&workspace.id).unwrap_or_default();
        let history = store.load_history(&workspace.id).unwrap_or_default();
        let mut state = Self::with_workspace(workspace, Some(store), dir.to_owned());
        state.jar = jar;
        state.history = history;
        state.notice = notice.map(|n| (n, Instant::now()));
        state
    }

    /// An in-memory state (no persistence), for tests and previews.
    pub fn in_memory(workspace: Workspace) -> Self {
        Self::with_workspace(workspace, None, PathBuf::from("."))
    }

    fn with_workspace(workspace: Workspace, store: Option<Store>, base_dir: PathBuf) -> Self {
        let selected = workspace.requests().first().map(|(_, r)| r.id.clone());
        Self {
            store,
            base_dir,
            workspace,
            jar: CookieJar::default(),
            history: History::default(),
            selected,
            recent: Vec::new(),
            request_tab: RequestTab::default(),
            response_tab: ResponseTab::default(),
            outcomes: Default::default(),
            filter: String::new(),
            collapsed: Default::default(),
            notice: None,
            inflight: None,
            streams: Default::default(),
            schemas: Default::default(),
            grpc_schemas: Default::default(),
            dirty_since: None,
            history_dirty: false,
        }
    }

    pub fn notify(&mut self, message: impl Into<String>) {
        self.notice = Some((message.into(), Instant::now()));
    }

    // ---------------------------------------------------------------- selection

    pub fn select(&mut self, id: Id) {
        self.recent.retain(|r| r != &id);
        self.recent.insert(0, id.clone());
        self.recent.truncate(50);
        self.selected = Some(id);
    }

    pub fn selected_request(&self) -> Option<&Request> {
        let id = self.selected.as_ref()?;
        match self.workspace.find_item(id)? {
            Item::Request(r) => Some(r),
            Item::Folder(_) => None,
        }
    }

    pub fn request_mut(&mut self, id: &Id) -> Option<&mut Request> {
        match self.workspace.find_item_mut(id)? {
            Item::Request(r) => Some(r),
            Item::Folder(_) => None,
        }
    }

    pub fn folder_mut(&mut self, id: &Id) -> Option<&mut Folder> {
        match self.workspace.find_item_mut(id)? {
            Item::Folder(f) => Some(f),
            Item::Request(_) => None,
        }
    }

    /// Requests for the switcher: recent first, then tree order, filtered by a
    /// case-insensitive subsequence match on name, URL or folder path.
    pub fn switcher_matches(&self, query: &str) -> Vec<(String, Id)> {
        let mut all: Vec<(String, &Request)> = self
            .workspace
            .requests()
            .into_iter()
            .map(|(folders, r)| {
                let path = folders
                    .iter()
                    .map(|f| f.name.as_str())
                    .collect::<Vec<_>>()
                    .join(" / ");
                (path, r)
            })
            .collect();
        let rank = |id: &Id| {
            self.recent
                .iter()
                .position(|r| r == id)
                .unwrap_or(usize::MAX)
        };
        all.sort_by_key(|(_, r)| rank(&r.id));
        let query = query.to_lowercase();
        all.into_iter()
            .filter(|(path, r)| {
                query.is_empty()
                    || subsequence(
                        &format!("{} {} {path}", r.name, r.url).to_lowercase(),
                        &query,
                    )
            })
            .map(|(path, r)| (path, r.id.clone()))
            .collect()
    }

    // ---------------------------------------------------------------- tree editing

    /// Adds a request inside `parent` (or at the top level) and selects it.
    pub fn new_request(&mut self, parent: Option<&Id>) -> Id {
        let request = Request::new("New Request", Method::Get, "");
        let id = request.id.clone();
        self.insert(parent, Item::Request(request));
        self.select(id.clone());
        self.request_tab = RequestTab::Params;
        id
    }

    pub fn new_folder(&mut self, parent: Option<&Id>) -> Id {
        let folder = Folder::new("New Folder");
        let id = folder.id.clone();
        self.insert(parent, Item::Folder(folder));
        id
    }

    fn insert(&mut self, parent: Option<&Id>, item: Item) {
        match parent.and_then(|p| self.folder_mut(p)) {
            Some(folder) => folder.children.push(item),
            None => self.workspace.items.push(item),
        }
        if let Some(p) = parent {
            self.collapsed.remove(p);
        }
        self.touch();
    }

    pub fn rename(&mut self, id: &Id, name: &str) {
        let name = name.trim();
        if name.is_empty() {
            return;
        }
        match self.workspace.find_item_mut(id) {
            Some(Item::Request(r)) => r.name = name.to_owned(),
            Some(Item::Folder(f)) => f.name = name.to_owned(),
            None => return,
        }
        self.touch();
    }

    /// Duplicates an item (deeply, with fresh ids) right after the original.
    pub fn duplicate(&mut self, id: &Id) -> Option<Id> {
        let (siblings, index) = locate_mut(&mut self.workspace.items, id)?;
        let mut copy = siblings[index].clone();
        refresh_ids(&mut copy);
        match &mut copy {
            Item::Request(r) => r.name = format!("{} (copy)", r.name),
            Item::Folder(f) => f.name = format!("{} (copy)", f.name),
        }
        let new_id = copy.id().clone();
        siblings.insert(index + 1, copy);
        self.touch();
        Some(new_id)
    }

    pub fn delete(&mut self, id: &Id) {
        if let Some((siblings, index)) = locate_mut(&mut self.workspace.items, id) {
            let removed = siblings.remove(index);
            let mut gone = Vec::new();
            collect_ids(&removed, &mut gone);
            for g in &gone {
                self.history.clear(g);
                self.outcomes.remove(g);
                self.recent.retain(|r| r != g);
            }
            if self.selected.as_ref().is_some_and(|s| gone.contains(s)) {
                self.selected = self.workspace.requests().first().map(|(_, r)| r.id.clone());
            }
            self.history_dirty = true;
            self.touch();
        }
    }

    /// Moves `id` to `index` within `parent` (top level when `None`). Moving a
    /// folder into itself or a descendant is ignored.
    pub fn move_item(&mut self, id: &Id, parent: Option<&Id>, index: usize) -> bool {
        if parent == Some(id) {
            return false;
        }
        if let (Some(p), Some(Item::Folder(folder))) = (parent, self.workspace.find_item(id)) {
            let mut inner = Vec::new();
            collect_ids(&Item::Folder(folder.clone()), &mut inner);
            if inner.contains(p) {
                return false;
            }
        }
        let Some((siblings, from)) = locate_mut(&mut self.workspace.items, id) else {
            return false;
        };
        let same_parent_ptr = siblings as *const Vec<Item>;
        let item = siblings.remove(from);
        let target: &mut Vec<Item> = match parent {
            Some(p) => match self.workspace.find_item_mut(p) {
                Some(Item::Folder(f)) => &mut f.children,
                _ => &mut self.workspace.items,
            },
            None => &mut self.workspace.items,
        };
        // Removing shifts later indices left within the same list.
        let mut index = index;
        if std::ptr::eq(target as *const Vec<Item>, same_parent_ptr) && index > from {
            index -= 1;
        }
        let index = index.min(target.len());
        target.insert(index, item);
        self.touch();
        true
    }

    // ---------------------------------------------------------------- sending

    pub fn is_sending(&self) -> bool {
        self.inflight.is_some()
    }

    pub fn sending_id(&self) -> Option<&Id> {
        self.inflight.as_ref().map(|f| &f.id)
    }

    pub fn sending_elapsed(&self) -> Option<Duration> {
        self.inflight.as_ref().map(|f| f.started.elapsed())
    }

    /// Sends request `id` on a background thread. A request already in
    /// flight is cancelled first.
    pub fn send(&mut self, id: &Id) {
        self.cancel();
        self.flush();
        let workspace = self.workspace.clone();
        let mut jar = self.jar.clone();
        let history = self.history.clone();
        let base_dir = self.base_dir.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = channel();
        let request_id = id.clone();
        let flag = cancel.clone();
        std::thread::spawn(move || {
            let client = Client::new(workspace.settings.clone());
            let result = execute(
                &workspace,
                &request_id,
                &base_dir,
                &client,
                Some(&mut jar),
                &history,
                &flag,
            )
            .map_err(|e| e.to_string());
            let _ = tx.send((result, jar));
        });
        self.inflight = Some(InFlight {
            id: id.clone(),
            cancel,
            started: Instant::now(),
            rx,
        });
    }

    /// Opens a WebSocket or SSE connection for request `id`.
    pub fn connect_stream(&mut self, id: &Id) {
        use nexus_http_core::exec::{PrepareOptions, prepare};
        use nexus_http_core::model::Protocol;
        self.flush();
        let Some((_, request)) = self.workspace.find_request(id) else {
            return;
        };
        let protocol = request.protocol;
        let mut prepared = match prepare(
            &self.workspace,
            id,
            &PrepareOptions {
                base_dir: &self.base_dir,
                responses: &self.history,
            },
        ) {
            Ok(p) => p,
            Err(e) => {
                let entry = self.streams.entry(id.clone()).or_default();
                entry.session = None;
                entry.events.push(nexus_http_core::stream::StreamEvent {
                    at_ms: 0.0,
                    direction: nexus_http_core::stream::Direction::Error,
                    label: "error".into(),
                    data: e.to_string(),
                    id: None,
                });
                return;
            }
        };
        let session = match protocol {
            Protocol::Grpc => {
                let (_, request) = self.workspace.find_request(id).expect("checked above");
                let (proto, method) =
                    (request.grpc.proto_file.clone(), request.grpc.method.clone());
                let body = self.rendered_message(id).unwrap_or_default();
                nexus_http_core::grpc::start(nexus_http_core::grpc::call_for(
                    &prepared,
                    &self.workspace.settings,
                    &proto,
                    &self.base_dir,
                    &method,
                    &body,
                ))
            }
            Protocol::WebSocket => {
                // The body is the message composer, not part of the handshake.
                prepared.body.clear();
                prepared
                    .headers
                    .retain(|(n, _)| !n.eq_ignore_ascii_case("content-type"));
                nexus_http_core::stream::connect_websocket(prepared, Some(self.jar.clone()))
            }
            _ => nexus_http_core::stream::connect_sse(
                prepared,
                self.workspace.settings.clone(),
                Some(self.jar.clone()),
            ),
        };
        let entry = self.streams.entry(id.clone()).or_default();
        entry.events.clear();
        entry.session = Some(session);
    }

    pub fn disconnect_stream(&mut self, id: &Id) {
        if let Some(stream) = self.streams.get(id)
            && let Some(session) = &stream.session
        {
            session.close();
        }
    }

    /// Sends the request's composed message over its WebSocket.
    pub fn send_stream_message(&mut self, id: &Id) -> bool {
        use nexus_http_core::model::Body;
        let Some((folders, request)) = self.workspace.find_request(id) else {
            return false;
        };
        let text = match &request.body {
            Body::Json { text } | Body::Xml { text } | Body::Text { text, .. } => text.clone(),
            _ => String::new(),
        };
        let ctx = nexus_http_core::template::Context::for_request(&self.workspace, &folders)
            .with_responses(&self.history);
        let rendered = match ctx.render(&text) {
            Ok(t) => t,
            Err(e) => {
                self.notify(format!("Message: {e}"));
                return false;
            }
        };
        self.streams
            .get(id)
            .and_then(|s| s.session.as_ref())
            .is_some_and(|s| s.send_text(rendered))
    }

    /// The request's message body with templates rendered.
    fn rendered_message(&mut self, id: &Id) -> Option<String> {
        use nexus_http_core::model::Body;
        let (folders, request) = self.workspace.find_request(id)?;
        let text = match &request.body {
            Body::Json { text } | Body::Xml { text } | Body::Text { text, .. } => text.clone(),
            _ => String::new(),
        };
        let ctx = nexus_http_core::template::Context::for_request(&self.workspace, &folders)
            .with_responses(&self.history);
        match ctx.render(&text) {
            Ok(t) => Some(t),
            Err(e) => {
                self.notify(format!("Message: {e}"));
                None
            }
        }
    }

    /// Loads the gRPC schema of request `id` (proto file or reflection) in the
    /// background.
    pub fn load_grpc_schema(&mut self, id: &Id) {
        use nexus_http_core::exec::{PrepareOptions, prepare};
        let prepared = match prepare(
            &self.workspace,
            id,
            &PrepareOptions {
                base_dir: &self.base_dir,
                responses: &self.history,
            },
        ) {
            Ok(p) => p,
            Err(e) => {
                let entry = self.grpc_schemas.entry(id.clone()).or_default();
                entry.error = Some(e.to_string());
                return;
            }
        };
        let proto = self
            .workspace
            .find_request(id)
            .map(|(_, r)| r.grpc.proto_file.clone())
            .unwrap_or_default();
        let target =
            nexus_http_core::grpc::Target::from_prepared(&prepared, &self.workspace.settings);
        let schema = nexus_http_core::grpc::schema_for(&proto, &self.base_dir);
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(
                nexus_http_core::grpc::load_methods(&schema, &target).map_err(|e| e.to_string()),
            );
        });
        let entry = self.grpc_schemas.entry(id.clone()).or_default();
        entry.rx = Some(rx);
        entry.error = None;
    }

    /// Fetches the GraphQL schema of request `id` by introspection.
    pub fn fetch_schema(&mut self, id: &Id) {
        let workspace = self.workspace.clone();
        let history = self.history.clone();
        let base_dir = self.base_dir.clone();
        let request_id = id.clone();
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let client = Client::new(workspace.settings.clone());
            let result = nexus_http_core::graphql::fetch_schema(
                &workspace,
                &request_id,
                &base_dir,
                &client,
                &history,
                &AtomicBool::new(false),
            );
            let _ = tx.send(result);
        });
        let entry = self.schemas.entry(id.clone()).or_default();
        entry.rx = Some(rx);
        entry.error = None;
    }

    /// Whether the open stream takes more messages (gRPC client streaming).
    pub fn stream_can_send(&self, id: &Id) -> bool {
        self.streams
            .get(id)
            .and_then(|s| s.session.as_ref())
            .is_some_and(|s| s.can_send())
    }

    pub fn stream_connected(&self, id: &Id) -> bool {
        self.streams.get(id).is_some_and(StreamState::connected)
    }

    pub fn cancel(&mut self) {
        if let Some(inflight) = self.inflight.take() {
            inflight.cancel.store(true, Ordering::Relaxed);
            self.outcomes
                .insert(inflight.id, Outcome::Error("Request cancelled".into()));
        }
    }

    /// Collects a finished send and performs debounced saves. Returns true
    /// while a request is in flight (the host should keep repainting).
    pub fn poll(&mut self) -> bool {
        if let Some(inflight) = &self.inflight {
            match inflight.rx.try_recv() {
                Ok((result, jar)) => {
                    let id = inflight.id.clone();
                    self.inflight = None;
                    self.jar = jar;
                    let outcome = match result {
                        Ok(response) => {
                            let checks = self
                                .workspace
                                .find_request(&id)
                                .map(|(_, r)| assertions::check_all(&r.assertions, &response))
                                .unwrap_or_default();
                            self.history.push(id.clone(), response.clone());
                            self.history_dirty = true;
                            Outcome::Response(Box::new(response), checks)
                        }
                        Err(e) => Outcome::Error(e),
                    };
                    self.outcomes.insert(id, outcome);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => {
                    let id = inflight.id.clone();
                    self.inflight = None;
                    self.outcomes.insert(
                        id,
                        Outcome::Error("request worker stopped unexpectedly".into()),
                    );
                }
            }
        }
        let mut streaming = false;
        for schema in self.schemas.values_mut() {
            if let Some(rx) = &schema.rx {
                match rx.try_recv() {
                    Ok(Ok(s)) => {
                        schema.schema = Some(Arc::new(s));
                        schema.error = None;
                        schema.rx = None;
                    }
                    Ok(Err(e)) => {
                        schema.error = Some(e);
                        schema.rx = None;
                    }
                    Err(TryRecvError::Empty) => streaming = true,
                    Err(TryRecvError::Disconnected) => schema.rx = None,
                }
            }
        }
        for schema in self.grpc_schemas.values_mut() {
            if let Some(rx) = &schema.rx {
                match rx.try_recv() {
                    Ok(Ok(methods)) => {
                        schema.methods = methods;
                        schema.error = None;
                        schema.rx = None;
                    }
                    Ok(Err(e)) => {
                        schema.error = Some(e);
                        schema.rx = None;
                    }
                    Err(TryRecvError::Empty) => streaming = true,
                    Err(TryRecvError::Disconnected) => schema.rx = None,
                }
            }
        }
        for stream in self.streams.values_mut() {
            if let Some(session) = &mut stream.session {
                stream.events.extend(session.poll());
                if stream.events.len() > MAX_STREAM_EVENTS {
                    let excess = stream.events.len() - MAX_STREAM_EVENTS;
                    stream.events.drain(..excess);
                }
                if session.is_finished() {
                    stream.session = None;
                } else {
                    streaming = true;
                }
            }
        }
        if self
            .dirty_since
            .is_some_and(|t| t.elapsed() >= SAVE_DEBOUNCE)
        {
            self.flush();
        }
        if self.history_dirty && self.inflight.is_none() {
            self.save_local();
        }
        self.inflight.is_some() || self.dirty_since.is_some() || streaming
    }

    /// The outcome to show for `id`: the latest send, else the latest stored response.
    pub fn outcome(&self, id: &Id) -> Option<Outcome> {
        self.outcomes.get(id).cloned().or_else(|| {
            self.history.latest(id).map(|r| {
                let checks = self
                    .workspace
                    .find_request(id)
                    .map(|(_, req)| assertions::check_all(&req.assertions, r))
                    .unwrap_or_default();
                Outcome::Response(Box::new(r.clone()), checks)
            })
        })
    }

    // ---------------------------------------------------------------- persistence

    /// Marks the workspace changed; it is saved after a short pause.
    pub fn touch(&mut self) {
        if self.dirty_since.is_none() {
            self.dirty_since = Some(Instant::now());
        }
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty_since.is_some()
    }

    /// Saves pending workspace changes now.
    pub fn flush(&mut self) {
        self.dirty_since = None;
        if let Some(store) = &self.store
            && let Err(e) = store.save(&self.workspace)
        {
            self.notify(format!("Saving failed: {e}"));
        }
    }

    fn save_local(&mut self) {
        self.history_dirty = false;
        if let Some(store) = &self.store {
            let id = self.workspace.id.clone();
            if let Err(e) = store
                .save_history(&id, &self.history)
                .and_then(|_| store.save_cookies(&id, &self.jar))
            {
                self.notify(format!("Saving history failed: {e}"));
            }
        }
    }

    /// History changed outside [`Self::poll`] (e.g. by the collection runner).
    pub fn mark_history_dirty(&mut self) {
        self.history_dirty = true;
    }

    /// Replaces the workspace (after an import) and saves it.
    pub fn replace_workspace(&mut self, workspace: Workspace) {
        self.flush();
        let store = self.store.clone();
        let base_dir = self.base_dir.clone();
        *self = Self::with_workspace(workspace, store, base_dir);
        self.touch();
        self.flush();
    }

    /// Merges imported items into the current workspace.
    pub fn merge_import(&mut self, imported: Workspace) {
        let first = imported.requests().first().map(|(_, r)| r.id.clone());
        self.workspace.items.extend(imported.items);
        for (k, v) in imported.base_environment.data {
            self.workspace.base_environment.data.entry(k).or_insert(v);
        }
        self.workspace.environments.extend(imported.environments);
        if let Some(id) = first {
            self.select(id);
        }
        self.touch();
    }

    pub fn store_root(&self) -> Option<&Path> {
        self.store.as_ref().map(Store::root)
    }

    pub fn base_dir(&self) -> &Path {
        &self.base_dir
    }
}

impl Drop for NexusState {
    fn drop(&mut self) {
        if self.dirty_since.is_some() {
            self.flush();
        }
        if self.history_dirty {
            self.save_local();
        }
        if let Some(inflight) = &self.inflight {
            inflight.cancel.store(true, Ordering::Relaxed);
        }
    }
}

fn subsequence(haystack: &str, needle: &str) -> bool {
    let mut chars = haystack.chars();
    needle
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|n| chars.any(|h| h == n))
}

/// The sibling list containing `id` and its index.
fn locate_mut<'a>(items: &'a mut Vec<Item>, id: &Id) -> Option<(&'a mut Vec<Item>, usize)> {
    if let Some(i) = items.iter().position(|item| item.id() == id) {
        return Some((items, i));
    }
    for item in items.iter_mut() {
        if let Item::Folder(f) = item
            && let Some(found) = locate_mut(&mut f.children, id)
        {
            return Some(found);
        }
    }
    None
}

fn refresh_ids(item: &mut Item) {
    match item {
        Item::Request(r) => r.id = Id::new("req"),
        Item::Folder(f) => {
            f.id = Id::new("fld");
            for child in &mut f.children {
                refresh_ids(child);
            }
        }
    }
}

fn collect_ids(item: &Item, out: &mut Vec<Id>) {
    out.push(item.id().clone());
    if let Item::Folder(f) = item {
        for child in &f.children {
            collect_ids(child, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[Item]) -> Vec<&str> {
        items.iter().map(Item::name).collect()
    }

    #[test]
    fn creates_renames_duplicates_and_deletes() {
        let mut s = NexusState::in_memory(Workspace::new("W"));
        let folder = s.new_folder(None);
        let req = s.new_request(Some(&folder));
        assert_eq!(s.selected.as_ref(), Some(&req));
        s.rename(&req, "  Login ");
        s.rename(&req, "   ");
        let copy = s.duplicate(&req).unwrap();
        let Some(Item::Folder(f)) = s.workspace.find_item(&folder) else {
            panic!()
        };
        assert_eq!(names(&f.children), ["Login", "Login (copy)"]);
        let folder_copy = s.duplicate(&folder).unwrap();
        assert_ne!(
            s.workspace.find_item(&folder_copy).map(Item::id),
            Some(&folder)
        );
        s.delete(&req);
        assert!(s.workspace.find_item(&req).is_none());
        assert!(s.workspace.find_item(&copy).is_some());
        assert!(s.is_dirty());
    }

    #[test]
    fn moves_items_and_refuses_cycles() {
        let mut s = NexusState::in_memory(Workspace::new("W"));
        let a = s.new_request(None);
        let b = s.new_request(None);
        s.rename(&a, "a");
        s.rename(&b, "b");
        let folder = s.new_folder(None);
        assert!(s.move_item(&b, None, 0));
        assert_eq!(names(&s.workspace.items), ["b", "a", "New Folder"]);
        assert!(s.move_item(&b, None, 3));
        assert_eq!(names(&s.workspace.items), ["a", "New Folder", "b"]);
        assert!(s.move_item(&a, Some(&folder), 0));
        assert_eq!(names(&s.workspace.items), ["New Folder", "b"]);
        let inner = s.new_folder(Some(&folder));
        assert!(!s.move_item(&folder, Some(&inner), 0));
        assert!(!s.move_item(&folder, Some(&folder), 0));
    }

    #[test]
    fn switcher_ranks_recent_and_filters() {
        let mut s = NexusState::in_memory(Workspace::new("W"));
        let a = s.new_request(None);
        s.rename(&a, "List users");
        let b = s.new_request(None);
        s.rename(&b, "Create order");
        s.select(a.clone());
        let all = s.switcher_matches("");
        assert_eq!(all[0].1, a);
        let filtered = s.switcher_matches("crord");
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].1, b);
    }

    #[test]
    fn persists_and_reopens() {
        let dir = tempfile::tempdir().unwrap();
        let id = {
            let mut s = NexusState::open(dir.path());
            let id = s.new_request(None);
            s.rename(&id, "Persisted");
            s.flush();
            id
        };
        let s = NexusState::open(dir.path());
        assert_eq!(
            s.selected_request().map(|r| r.name.as_str()),
            Some("Persisted")
        );
        assert_eq!(s.selected.as_ref(), Some(&id));
    }

    #[test]
    fn grpc_requests_load_methods_and_report_connection_errors() {
        use nexus_http_core::model::{Body, Method, Protocol};
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("echo.proto"),
            "syntax = \"proto3\";\npackage t;\nservice E { rpc Say(M) returns (M); }\nmessage M { string name = 1; }\n",
        )
        .unwrap();
        let mut s = NexusState::in_memory(Workspace::new("W"));
        s.base_dir = dir.path().to_path_buf();
        let id = s.new_request(None);
        {
            let r = s.request_mut(&id).unwrap();
            r.protocol = Protocol::Grpc;
            r.method = Method::Post;
            r.url = "grpc://127.0.0.1:9".into();
            r.grpc.proto_file = "echo.proto".into();
            r.grpc.method = "t.E/Say".into();
            r.body = Body::Json {
                text: "{\"name\": \"x\"}".into(),
            };
        }
        s.load_grpc_schema(&id);
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while s.grpc_schemas[&id].loading() && Instant::now() < deadline {
            s.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let methods = &s.grpc_schemas[&id].methods;
        assert_eq!(methods.len(), 1, "{:?}", s.grpc_schemas[&id].error);
        assert_eq!(methods[0].name, "t.E/Say");
        assert_eq!(methods[0].kind(), "unary");
        assert!(methods[0].template.contains("\"name\""));

        s.connect_stream(&id);
        while Instant::now() < deadline && !s.streams[&id].events.iter().any(|e| e.label == "error")
        {
            s.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let events = &s.streams[&id].events;
        assert!(
            events
                .iter()
                .any(|e| e.data.contains("cannot connect to http://127.0.0.1:9")),
            "{events:?}"
        );
    }

    #[test]
    fn grpc_schema_errors_are_reported() {
        use nexus_http_core::model::Protocol;
        let mut s = NexusState::in_memory(Workspace::new("W"));
        let id = s.new_request(None);
        {
            let r = s.request_mut(&id).unwrap();
            r.protocol = Protocol::Grpc;
            r.url = "grpc://127.0.0.1:9".into();
            r.grpc.proto_file = "missing.proto".into();
        }
        s.load_grpc_schema(&id);
        let deadline = Instant::now() + std::time::Duration::from_secs(10);
        while s.grpc_schemas[&id].loading() && Instant::now() < deadline {
            s.poll();
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        let error = s.grpc_schemas[&id].error.clone().unwrap();
        assert!(
            error.starts_with("cannot read the .proto schema"),
            "{error}"
        );
    }
}
