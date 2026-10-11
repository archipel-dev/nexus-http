//! `{{variable}}` fields: kit `TextField` with the template highlighter (resolved,
//! unresolved and secret variables of the active environment) and the autocomplete list.

use archipel_kit::components::code::{TemplateHighlighter, TemplateToken};
use archipel_kit::components::{MenuItem, TextField};
use archipel_kit::egui::{Id, Key, Modifiers, Response, Ui, WidgetInfo};
use archipel_kit::parts::Popover;
use nexus_http_core::template;
use serde_json::{Map, Value};
use std::collections::HashSet;
use std::sync::Arc;

/// Most suggestions shown at once.
const MAX_ITEMS: usize = 12;

/// One suggestion: a variable path and a short preview of its value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub label: String,
    pub detail: String,
}

/// The variables visible to a request, for highlighting and completion.
#[derive(Debug, Clone, Default)]
pub struct Variables {
    items: Vec<Item>,
    secrets: Arc<HashSet<String>>,
}

impl Variables {
    /// `secrets` are the names whose values must never be previewed.
    pub fn new(vars: &Map<String, Value>, secrets: Arc<HashSet<String>>) -> Self {
        let items = template::variable_paths(vars)
            .into_iter()
            .map(|(label, value)| Item {
                detail: if secrets.contains(&label) {
                    "••••••".into()
                } else {
                    preview(&value)
                },
                label,
            })
            .collect();
        Self { items, secrets }
    }

    /// What `{{name}}` resolves to.
    pub fn token(&self, name: &str) -> TemplateToken {
        let name = name.trim();
        let name = name.strip_prefix("_.").unwrap_or(name);
        if self.secrets.contains(name) {
            TemplateToken::Secret
        } else if self.items.iter().any(|i| i.label == name) {
            TemplateToken::Resolved
        } else {
            TemplateToken::Unresolved
        }
    }

    /// Candidates for the typed `partial` name: prefix matches first, then substring
    /// matches, at most [`MAX_ITEMS`].
    pub fn suggestions(&self, partial: &str) -> Vec<Item> {
        let needle = partial.trim_start_matches("_.").to_lowercase();
        let mut scored: Vec<(u8, &Item)> = self
            .items
            .iter()
            .filter_map(|i| rank(&i.label, &needle).map(|r| (r, i)))
            .collect();
        scored.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.label.cmp(&b.1.label)));
        scored
            .into_iter()
            .take(MAX_ITEMS)
            .map(|(_, i)| i.clone())
            .collect()
    }
}

fn rank(label: &str, needle: &str) -> Option<u8> {
    let l = label.to_lowercase();
    if needle.is_empty() || l.starts_with(needle) {
        Some(0)
    } else if l.contains(needle) {
        Some(1)
    } else {
        None
    }
}

fn preview(value: &Value) -> String {
    let text = match value {
        Value::Object(m) => format!("{{…}} {} keys", m.len()),
        Value::Array(a) => format!("[…] {} items", a.len()),
        other => template::value_to_text(other),
    };
    if text.chars().count() > 40 {
        format!("{}…", text.chars().take(40).collect::<String>())
    } else {
        text
    }
}

/// A single-line template field.
pub struct Field<'a> {
    pub text: &'a mut String,
    pub id: Id,
    /// The accessible name ("URL", "Value, row 2").
    pub name: &'a str,
    pub placeholder: &'a str,
    pub width: Option<f32>,
    /// Masks the value unless it is a `{{variable}}` reference.
    pub secret: bool,
    pub monospace: bool,
    /// Shows `name` as a visible label above the input.
    pub labelled: bool,
}

impl<'a> Field<'a> {
    pub fn new(text: &'a mut String, id: Id, name: &'a str) -> Self {
        Self {
            text,
            id,
            name,
            placeholder: "",
            width: None,
            secret: false,
            monospace: false,
            labelled: false,
        }
    }
}

/// What happened to a [`Field`].
pub struct Edit {
    pub response: Response,
    /// Enter was pressed with no suggestion list open.
    pub submitted: bool,
}

#[derive(Clone, Default)]
struct Popup {
    open: bool,
    selected: usize,
    matches: Vec<Item>,
}

/// Shows the field with highlighting and the suggestion list under the caret.
pub fn show(ui: &mut Ui, vars: &Variables, field: Field<'_>) -> Edit {
    let Field {
        text,
        id,
        name,
        placeholder,
        width,
        secret,
        monospace,
        labelled,
    } = field;

    if secret && !text.contains("{{") {
        let mut input = TextField::new(&mut *text)
            .id(id)
            .placeholder(placeholder)
            .password(true);
        if labelled {
            input = input.label(name);
        }
        if let Some(w) = width {
            input = input.width(w);
        }
        let out = input.show(ui);
        if !labelled {
            name_text_edit(&out.response, name, placeholder, text);
        }
        return Edit {
            response: out.response,
            submitted: out.submitted,
        };
    }

    let key = id.with("completion");
    let mut popup: Popup = ui.data(|d| d.get_temp(key)).unwrap_or_default();
    // While the list is open it owns the navigation keys.
    let listing = popup.open && !popup.matches.is_empty();
    let (mut down, mut up, mut accept) = (false, false, false);
    if listing {
        ui.input_mut(|i| {
            down = i.consume_key(Modifiers::NONE, Key::ArrowDown);
            up = i.consume_key(Modifiers::NONE, Key::ArrowUp);
            accept = i.consume_key(Modifiers::NONE, Key::Enter)
                || i.consume_key(Modifiers::NONE, Key::Tab);
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                popup.open = false;
            }
        });
    }

    let highlighter = TemplateHighlighter::new(|n: &str| vars.token(n));
    let mut input = TextField::new(&mut *text)
        .id(id)
        .placeholder(placeholder)
        .monospace(monospace)
        .highlighter(&highlighter);
    if labelled {
        input = input.label(name);
    }
    if let Some(w) = width {
        input = input.width(w);
    }
    let out = input.show(ui);
    if !labelled {
        name_text_edit(&out.response, name, placeholder, text);
    }

    popup.matches = match &out.template {
        Some(token) if out.response.has_focus() || popup.open => vars.suggestions(&token.partial),
        _ => Vec::new(),
    };
    if out.response.changed() {
        popup.open = true;
        popup.selected = 0;
    }
    // Tabbing away closes the list; a click is the popover's (outside) or an item's (inside).
    let clicking = ui.input(|i| i.pointer.any_pressed() || i.pointer.any_released());
    if popup.matches.is_empty() || out.response.lost_focus() && !clicking {
        popup.open = false;
    }
    let last = popup.matches.len().saturating_sub(1);
    if down {
        popup.selected = (popup.selected + 1).min(last);
    }
    if up {
        popup.selected = popup.selected.saturating_sub(1);
    }
    popup.selected = popup.selected.min(last);

    let mut response = out.response.clone();
    let mut picked = None;
    if accept && let Some(item) = popup.matches.get(popup.selected) {
        picked = Some(item.label.clone());
    }
    if let Some(caret) = out.caret_rect {
        let (matches, selected) = (popup.matches.clone(), popup.selected);
        Popover::new(key.with("popover"), caret).width(300.0).show(
            ui.ctx(),
            &mut popup.open,
            |ui| {
                for (i, item) in matches.iter().enumerate() {
                    let row = MenuItem::new(item.label.as_str())
                        .shortcut(item.detail.as_str())
                        .highlighted(i == selected);
                    if ui.add(row).clicked() {
                        picked = Some(item.label.clone());
                    }
                }
            },
        );
    }
    if let Some(label) = picked {
        // `insert_completion` edits through the same text the field was given.
        if out.insert_completion(ui.ctx(), text, &label) {
            response.mark_changed();
        }
        popup.open = false;
    }
    ui.data_mut(|d| d.insert_temp(key, popup));
    Edit {
        response,
        submitted: out.submitted && !accept,
    }
}

fn name_text_edit(response: &Response, name: &str, placeholder: &str, text: &str) {
    if name.is_empty() {
        return;
    }
    response.widget_info(|| {
        let mut info = WidgetInfo::text_edit(true, text, text, placeholder);
        info.label = Some(name.to_owned());
        info
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn vars() -> Variables {
        let map = json!({"host": "x", "api": {"host": "y"}, "token": "t"})
            .as_object()
            .unwrap()
            .clone();
        Variables::new(&map, Arc::new(HashSet::from(["token".to_owned()])))
    }

    #[test]
    fn ranks_prefix_before_substring() {
        let found = vars().suggestions("ho");
        assert_eq!(found[0].label, "host");
        assert!(found.iter().any(|i| i.label == "api.host"));
        assert!(!found.iter().any(|i| i.label == "token"));
    }

    #[test]
    fn classifies_tokens_and_hides_secrets() {
        let v = vars();
        assert_eq!(v.token(" host "), TemplateToken::Resolved);
        assert_eq!(v.token("_.api.host"), TemplateToken::Resolved);
        assert_eq!(v.token("nope"), TemplateToken::Unresolved);
        assert_eq!(v.token("token"), TemplateToken::Secret);
        let secret = v.suggestions("tok");
        assert_eq!(secret[0].detail, "••••••");
    }
}
