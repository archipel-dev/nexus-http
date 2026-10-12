//! `{{ variables }}` in fields: highlighting, autocomplete and secrets, the same in every
//! field of a request. The completion rules live in `nexus_http_core::completion`.

use crate::state::{NexusState, SECRET_KEYS_ID};
use archipel_kit::components::code::{Json, PlainText, TemplateHighlighter, TemplateToken};
use archipel_kit::components::{Completer, Completion, Completions, TextField};
use nexus_http_core::model::Id;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::Arc;

type Resolver = Box<dyn Fn(&str) -> TemplateToken>;

/// The variables a request sees, ready for its fields.
pub struct Templates {
    vars: Map<String, Value>,
    /// Plain text with `{{ variables }}`: URLs, header values, auth fields.
    pub plain: TemplateHighlighter<Resolver, PlainText>,
    /// JSON with `{{ variables }}`: bodies and messages.
    pub json: TemplateHighlighter<Resolver, Json>,
}

impl Templates {
    /// The variables of request `id`: base and active environment, then its folders.
    pub fn for_request(ui: &egui::Ui, state: &NexusState, id: &Id) -> Self {
        let ws = &state.workspace;
        let folders = ws.find_request(id).map(|(f, _)| f).unwrap_or_default();
        let vars = nexus_http_core::template::merged_variables(
            &ws.base_environment,
            ws.active_environment(),
            &folders,
        );
        Self::new(ui, vars)
    }

    pub fn new(ui: &egui::Ui, vars: Map<String, Value>) -> Self {
        let paths: Arc<HashSet<String>> = Arc::new(
            nexus_http_core::template::variable_paths(&vars)
                .into_iter()
                .map(|(path, _)| path)
                .collect(),
        );
        let secrets: Arc<HashSet<String>> = ui
            .data(|d| d.get_temp(egui::Id::new(SECRET_KEYS_ID)))
            .unwrap_or_default();
        let resolver = || -> Resolver {
            let (paths, secrets) = (paths.clone(), secrets.clone());
            Box::new(move |name: &str| {
                let name = name.strip_prefix("_.").unwrap_or(name);
                if secrets.contains(name) {
                    TemplateToken::Secret
                } else if paths.contains(name) {
                    TemplateToken::Resolved
                } else {
                    TemplateToken::Unresolved
                }
            })
        };
        Self {
            vars,
            plain: TemplateHighlighter::new(resolver()),
            json: TemplateHighlighter::new(resolver()).inner(Json),
        }
    }

    pub fn vars(&self) -> &Map<String, Value> {
        &self.vars
    }

    /// A single-line field with highlighting and autocomplete.
    pub fn field<'t>(&'t self, text: &'t mut String) -> TextField<'t> {
        TextField::new(text)
            .monospace(true)
            .highlighter(&self.plain)
            .completer(self)
    }
}

impl Completer for Templates {
    fn complete(&self, text: &str, caret: usize) -> Option<Completions> {
        let (start, end, items) = nexus_http_core::completion::complete(text, caret, &self.vars)?;
        Some(Completions {
            start,
            end,
            items: items
                .into_iter()
                .map(|item| Completion::new(item.label, item.insert).detail(item.detail))
                .collect(),
        })
    }
}
