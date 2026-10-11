//! Git-friendly storage under a `.nexus/` directory (in a project or the user
//! data dir). Every request, folder and environment is its own pretty-printed
//! JSON file so edits produce small, reviewable diffs; the tree order lives in
//! `workspace.json`. Secrets, cookies and response history are machine-local and
//! kept under `local/`, which a generated `.gitignore` excludes.
//!
//! ```text
//! .nexus/
//!   .gitignore                      local/
//!   workspaces/<workspace-id>/
//!     workspace.json                name, settings, tree order, environment order
//!     items/<item-id>.json          one folder (without children) or request
//!     environments/base.json
//!     environments/<env-id>.json
//!   local/<workspace-id>/
//!     secrets.json  cookies.json  history.json
//! ```

use crate::cookies::CookieJar;
use crate::history::History;
use crate::model::{Environment, Folder, Id, Item, Request, Workspace, WorkspaceSettings};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const DIR_NAME: &str = ".nexus";

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("{path}: {reason}")]
    Io { path: PathBuf, reason: String },
    #[error("{path} is not valid: {reason}")]
    Parse { path: PathBuf, reason: String },
    #[error("invalid id `{0}`; ids may contain letters, digits, `-`, `_` and `.`")]
    InvalidId(String),
    #[error("workspace {0} does not exist")]
    Missing(Id),
}

#[derive(Debug, Serialize, Deserialize)]
struct WorkspaceFile {
    format: u32,
    id: Id,
    name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    active_environment: Option<Id>,
    #[serde(default)]
    environments: Vec<Id>,
    #[serde(default)]
    settings: WorkspaceSettings,
    #[serde(default)]
    tree: Vec<TreeNode>,
}

#[derive(Debug, Serialize, Deserialize)]
struct TreeNode {
    id: Id,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    children: Vec<TreeNode>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ItemFile {
    Folder(Folder),
    Request(Request),
}

const FORMAT: u32 = 1;

/// A `.nexus` directory.
#[derive(Debug, Clone)]
pub struct Store {
    root: PathBuf,
}

/// Summary of a stored workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceSummary {
    pub id: Id,
    pub name: String,
}

impl Store {
    /// The store inside `dir` (`dir/.nexus`). Nothing is created until saving.
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            root: dir.join(DIR_NAME),
        }
    }

    /// A store rooted exactly at `root`.
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn workspace_dir(&self, id: &Id) -> Result<PathBuf, StorageError> {
        check_id(id)?;
        Ok(self.root.join("workspaces").join(id.as_str()))
    }

    fn local_dir(&self, id: &Id) -> Result<PathBuf, StorageError> {
        check_id(id)?;
        Ok(self.root.join("local").join(id.as_str()))
    }

    pub fn list(&self) -> Result<Vec<WorkspaceSummary>, StorageError> {
        let dir = self.root.join("workspaces");
        let entries = match fs::read_dir(&dir) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(e) => return Err(io(&dir, e)),
        };
        let mut out = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let file = entry.path().join("workspace.json");
            if file.is_file() {
                let ws: WorkspaceFile = read_json(&file)?;
                out.push(WorkspaceSummary {
                    id: ws.id,
                    name: ws.name,
                });
            }
        }
        out.sort_by_key(|w| w.name.to_lowercase());
        Ok(out)
    }

    pub fn load(&self, id: &Id) -> Result<Workspace, StorageError> {
        let dir = self.workspace_dir(id)?;
        let file_path = dir.join("workspace.json");
        if !file_path.is_file() {
            return Err(StorageError::Missing(id.clone()));
        }
        let file: WorkspaceFile = read_json(&file_path)?;
        let mut items: BTreeMap<Id, ItemFile> = BTreeMap::new();
        if let Ok(entries) = fs::read_dir(dir.join("items")) {
            for entry in entries.filter_map(Result::ok) {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "json") {
                    let item: ItemFile = read_json(&path)?;
                    let item_id = match &item {
                        ItemFile::Folder(f) => f.id.clone(),
                        ItemFile::Request(r) => r.id.clone(),
                    };
                    items.insert(item_id, item);
                }
            }
        }
        let mut tree = build_tree(&file.tree, &mut items);
        // Items present on disk but missing from the tree (e.g. added by a
        // teammate's merge) are appended rather than lost.
        for (_, orphan) in std::mem::take(&mut items) {
            tree.push(match orphan {
                ItemFile::Folder(f) => Item::Folder(f),
                ItemFile::Request(r) => Item::Request(r),
            });
        }

        let env_dir = dir.join("environments");
        let secrets: BTreeMap<Id, Map<String, Value>> =
            read_json_or_default(&self.local_dir(id)?.join("secrets.json"))?;
        let mut base: Environment = read_json(&env_dir.join("base.json"))?;
        restore_secrets(&mut base, &secrets);
        let mut environments = Vec::new();
        for env_id in &file.environments {
            check_id(env_id)?;
            let path = env_dir.join(format!("{env_id}.json"));
            if path.is_file() {
                let mut env: Environment = read_json(&path)?;
                restore_secrets(&mut env, &secrets);
                environments.push(env);
            }
        }
        Ok(Workspace {
            id: file.id,
            name: file.name,
            description: file.description,
            items: tree,
            base_environment: base,
            environments,
            active_environment: file.active_environment,
            settings: file.settings,
        })
    }

    pub fn save(&self, workspace: &Workspace) -> Result<(), StorageError> {
        let dir = self.workspace_dir(&workspace.id)?;
        self.write_gitignore()?;

        let mut tree = Vec::new();
        let mut item_files: Vec<(Id, ItemFile)> = Vec::new();
        flatten(&workspace.items, &mut tree, &mut item_files)?;
        let file = WorkspaceFile {
            format: FORMAT,
            id: workspace.id.clone(),
            name: workspace.name.clone(),
            description: workspace.description.clone(),
            active_environment: workspace.active_environment.clone(),
            environments: workspace
                .environments
                .iter()
                .map(|e| e.id.clone())
                .collect(),
            settings: workspace.settings.clone(),
            tree,
        };
        write_json(&dir.join("workspace.json"), &file)?;

        let items_dir = dir.join("items");
        let mut keep = BTreeSet::new();
        for (id, item) in &item_files {
            let name = format!("{id}.json");
            write_json(&items_dir.join(&name), item)?;
            keep.insert(name);
        }
        remove_stale(&items_dir, &keep)?;

        let env_dir = dir.join("environments");
        let mut secrets: BTreeMap<Id, Map<String, Value>> = BTreeMap::new();
        let base = split_secrets(&workspace.base_environment, &mut secrets);
        write_json(&env_dir.join("base.json"), &base)?;
        let mut keep = BTreeSet::from(["base.json".to_owned()]);
        for env in &workspace.environments {
            check_id(&env.id)?;
            let name = format!("{}.json", env.id);
            write_json(&env_dir.join(&name), &split_secrets(env, &mut secrets))?;
            keep.insert(name);
        }
        remove_stale(&env_dir, &keep)?;

        let secrets_path = self.local_dir(&workspace.id)?.join("secrets.json");
        if secrets.is_empty() {
            remove_if_exists(&secrets_path)?;
        } else {
            write_json(&secrets_path, &secrets)?;
        }
        Ok(())
    }

    pub fn delete(&self, id: &Id) -> Result<(), StorageError> {
        for dir in [self.workspace_dir(id)?, self.local_dir(id)?] {
            match fs::remove_dir_all(&dir) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(io(&dir, e)),
            }
        }
        Ok(())
    }

    pub fn load_cookies(&self, id: &Id) -> Result<CookieJar, StorageError> {
        read_json_or_default(&self.local_dir(id)?.join("cookies.json"))
    }

    pub fn save_cookies(&self, id: &Id, jar: &CookieJar) -> Result<(), StorageError> {
        self.write_gitignore()?;
        write_json(&self.local_dir(id)?.join("cookies.json"), jar)
    }

    pub fn load_history(&self, id: &Id) -> Result<History, StorageError> {
        read_json_or_default(&self.local_dir(id)?.join("history.json"))
    }

    pub fn save_history(&self, id: &Id, history: &History) -> Result<(), StorageError> {
        self.write_gitignore()?;
        write_json(&self.local_dir(id)?.join("history.json"), history)
    }

    fn write_gitignore(&self) -> Result<(), StorageError> {
        let path = self.root.join(".gitignore");
        if !path.exists() {
            write_atomic(
                &path,
                b"# Machine-local nexus-http data: secrets, cookies, response history.\nlocal/\n",
            )?;
        }
        Ok(())
    }
}

fn check_id(id: &Id) -> Result<(), StorageError> {
    if id.is_valid() {
        Ok(())
    } else {
        Err(StorageError::InvalidId(id.0.clone()))
    }
}

fn build_tree(nodes: &[TreeNode], items: &mut BTreeMap<Id, ItemFile>) -> Vec<Item> {
    nodes
        .iter()
        .filter_map(|node| match items.remove(&node.id)? {
            ItemFile::Request(r) => Some(Item::Request(r)),
            ItemFile::Folder(mut f) => {
                f.children = build_tree(&node.children, items);
                Some(Item::Folder(f))
            }
        })
        .collect()
}

fn flatten(
    items: &[Item],
    tree: &mut Vec<TreeNode>,
    files: &mut Vec<(Id, ItemFile)>,
) -> Result<(), StorageError> {
    for item in items {
        check_id(item.id())?;
        match item {
            Item::Request(r) => {
                tree.push(TreeNode {
                    id: r.id.clone(),
                    children: Vec::new(),
                });
                files.push((r.id.clone(), ItemFile::Request(r.clone())));
            }
            Item::Folder(f) => {
                let mut children = Vec::new();
                flatten(&f.children, &mut children, files)?;
                tree.push(TreeNode {
                    id: f.id.clone(),
                    children,
                });
                let mut shallow = f.clone();
                shallow.children.clear();
                files.push((f.id.clone(), ItemFile::Folder(shallow)));
            }
        }
    }
    Ok(())
}

fn split_secrets(env: &Environment, secrets: &mut BTreeMap<Id, Map<String, Value>>) -> Environment {
    let mut public = env.clone();
    for key in &env.secrets {
        if let Some(value) = public.data.get_mut(key) {
            let secret = std::mem::replace(value, Value::String(String::new()));
            secrets
                .entry(env.id.clone())
                .or_default()
                .insert(key.clone(), secret);
        }
    }
    public
}

fn restore_secrets(env: &mut Environment, secrets: &BTreeMap<Id, Map<String, Value>>) {
    if let Some(values) = secrets.get(&env.id) {
        for (key, value) in values {
            env.data.insert(key.clone(), value.clone());
        }
    }
}

fn io(path: &Path, e: std::io::Error) -> StorageError {
    StorageError::Io {
        path: path.to_owned(),
        reason: e.to_string(),
    }
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Result<T, StorageError> {
    let text = fs::read_to_string(path).map_err(|e| io(path, e))?;
    serde_json::from_str(&text).map_err(|e| StorageError::Parse {
        path: path.to_owned(),
        reason: e.to_string(),
    })
}

fn read_json_or_default<T: DeserializeOwned + Default>(path: &Path) -> Result<T, StorageError> {
    if path.is_file() {
        read_json(path)
    } else {
        Ok(T::default())
    }
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), StorageError> {
    let mut text = serde_json::to_string_pretty(value).map_err(|e| StorageError::Parse {
        path: path.to_owned(),
        reason: e.to_string(),
    })?;
    text.push('\n');
    // Skip unchanged files so saving does not touch mtimes or git status.
    if fs::read(path).is_ok_and(|current| current == text.as_bytes()) {
        return Ok(());
    }
    write_atomic(path, text.as_bytes())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), StorageError> {
    let parent = path.parent().unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|e| io(parent, e))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, bytes).map_err(|e| io(&tmp, e))?;
    fs::rename(&tmp, path).map_err(|e| io(path, e))
}

fn remove_stale(dir: &Path, keep: &BTreeSet<String>) -> Result<(), StorageError> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(());
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.ends_with(".json") && !keep.contains(&name) {
            remove_if_exists(&entry.path())?;
        }
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<(), StorageError> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io(path, e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Body, Method};
    use serde_json::json;

    fn sample() -> Workspace {
        let mut ws = Workspace::new("Demo API");
        ws.base_environment = ws.base_environment.clone().with("host", "localhost");
        let mut prod = Environment::new("Production")
            .with("host", "api.prod")
            .with("token", "s3cr3t");
        prod.secrets.push("token".into());
        ws.active_environment = Some(prod.id.clone());
        ws.environments.push(prod);
        let mut folder = Folder::new("Users");
        folder.children.push(Item::Request(
            Request::new("Create", Method::Post, "{{host}}/users").with_body(Body::Json {
                text: "{\"name\":\"Ada\"}".into(),
            }),
        ));
        folder.children.push(Item::Request(Request::new(
            "List",
            Method::Get,
            "{{host}}/users",
        )));
        ws.items.push(Item::Folder(folder));
        ws.items.push(Item::Request(Request::new(
            "Health",
            Method::Get,
            "{{host}}/health",
        )));
        ws
    }

    #[test]
    fn round_trips_workspace_preserving_order() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_dir(dir.path());
        let ws = sample();
        store.save(&ws).unwrap();
        assert_eq!(store.load(&ws.id).unwrap(), ws);
        let list = store.list().unwrap();
        assert_eq!(
            list,
            vec![WorkspaceSummary {
                id: ws.id.clone(),
                name: "Demo API".into()
            }]
        );
        assert_eq!(
            fs::read_to_string(store.root().join(".gitignore"))
                .unwrap()
                .lines()
                .last(),
            Some("local/")
        );
    }

    #[test]
    fn keeps_secrets_out_of_shared_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_dir(dir.path());
        let ws = sample();
        store.save(&ws).unwrap();
        let env_id = &ws.environments[0].id;
        let shared = fs::read_to_string(
            store
                .root()
                .join("workspaces")
                .join(ws.id.as_str())
                .join("environments")
                .join(format!("{env_id}.json")),
        )
        .unwrap();
        assert!(!shared.contains("s3cr3t"));
        let local = fs::read_to_string(
            store
                .root()
                .join("local")
                .join(ws.id.as_str())
                .join("secrets.json"),
        )
        .unwrap();
        assert!(local.contains("s3cr3t"));
        assert_eq!(
            store.load(&ws.id).unwrap().environments[0].data["token"],
            json!("s3cr3t")
        );
    }

    #[test]
    fn removes_deleted_items_and_keeps_orphans() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_dir(dir.path());
        let mut ws = sample();
        store.save(&ws).unwrap();
        let health = ws.items.pop().unwrap();
        store.save(&ws).unwrap();
        let items_dir = store
            .root()
            .join("workspaces")
            .join(ws.id.as_str())
            .join("items");
        assert!(!items_dir.join(format!("{}.json", health.id())).exists());

        // An item file added outside the tree (e.g. by a git merge) still loads.
        let extra = Request::new("Extra", Method::Get, "x");
        fs::write(
            items_dir.join(format!("{}.json", extra.id)),
            serde_json::to_string(&ItemFile::Request(extra.clone())).unwrap(),
        )
        .unwrap();
        let loaded = store.load(&ws.id).unwrap();
        assert_eq!(loaded.items.last().unwrap().id(), &extra.id);
    }

    #[test]
    fn rejects_unsafe_ids_and_reports_parse_errors() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_dir(dir.path());
        let mut ws = sample();
        ws.id = Id::from("../escape");
        assert!(matches!(store.save(&ws), Err(StorageError::InvalidId(_))));
        assert!(matches!(
            store.load(&Id::from("missing")),
            Err(StorageError::Missing(_))
        ));

        let ws = sample();
        store.save(&ws).unwrap();
        let file = store
            .root()
            .join("workspaces")
            .join(ws.id.as_str())
            .join("workspace.json");
        fs::write(&file, "{ nope").unwrap();
        assert!(matches!(
            store.load(&ws.id),
            Err(StorageError::Parse { .. })
        ));
    }

    #[test]
    fn persists_cookies_and_history() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_dir(dir.path());
        let id = Id::from("wrk_1");
        let mut jar = CookieJar::default();
        jar.store(&url::Url::parse("http://h.test/").unwrap(), "a=1");
        store.save_cookies(&id, &jar).unwrap();
        assert_eq!(store.load_cookies(&id).unwrap(), jar);
        assert_eq!(store.load_history(&id).unwrap(), History::default());
    }
}
