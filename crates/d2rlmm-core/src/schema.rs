//! Mod config: the schema a `mod.json` declares, and the values a user chose.
//!
//! Two things read this module: the installer (which hands the resolved values to
//! the mod's JavaScript as `config`) and the preferences dialog (which renders
//! widgets from the same schema). Keeping one implementation means the dialog can
//! never disagree with what the mod sees, which is the kind of bug that only shows
//! up as "my setting does nothing".
//!
//! # Bindings
//!
//! Fields can carry small expression trees — `["eq", ["value", "mode"], "custom"]` —
//! under `visible` and `overrideValue`. They are evaluated against the *current*
//! values, so they have to be recomputed after every change, and they can chain
//! (a field whose value drives another field's visibility that in turn gates a
//! third). [`ModConfig::recompute`] runs the resolution twice to a fixed point for
//! that reason; two passes covers every chain shape seen in the wild, and a third
//! would only matter for a cycle, which would never settle anyway.

use std::collections::HashMap;

use serde_json::{Map, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    Checkbox,
    Number,
    Text,
    Select,
    Color,
    Section,
}

impl FieldType {
    pub fn of(field: &Value) -> Option<FieldType> {
        match field.get("type").and_then(Value::as_str)? {
            "checkbox" => Some(FieldType::Checkbox),
            "number" => Some(FieldType::Number),
            "text" => Some(FieldType::Text),
            "select" => Some(FieldType::Select),
            "color" => Some(FieldType::Color),
            "section" => Some(FieldType::Section),
            _ => None,
        }
    }
}

pub fn field_id(field: &Value) -> Option<&str> {
    field.get("id").and_then(Value::as_str)
}

pub fn field_name(field: &Value) -> Option<&str> {
    field.get("name").and_then(Value::as_str)
}

pub fn field_description(field: &Value) -> Option<&str> {
    field.get("description").and_then(Value::as_str)
}

pub fn children(field: &Value) -> &[Value] {
    field
        .get("children")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
}

/// Every field in the tree, depth first, sections included.
pub fn flatten<'a>(fields: &'a [Value], out: &mut Vec<&'a Value>) {
    for field in fields {
        out.push(field);
        let kids = children(field);
        if !kids.is_empty() {
            flatten(kids, out);
        }
    }
}

/// Every leaf field (no sections), depth first.
pub fn leaves<'a>(fields: &'a [Value], out: &mut Vec<&'a Value>) {
    for field in fields {
        let kids = children(field);
        if kids.is_empty() {
            out.push(field);
        } else {
            leaves(kids, out);
        }
    }
}

/// A `select` field's options, as `(label, value, description)`.
pub fn options(field: &Value) -> Vec<(String, Value, Option<String>)> {
    field
        .get("options")
        .and_then(Value::as_array)
        .map(|opts| {
            opts.iter()
                .filter_map(|opt| {
                    let value = opt.get("value").cloned()?;
                    let label = opt
                        .get("label")
                        .and_then(Value::as_str)
                        .unwrap_or_else(|| value.as_str().unwrap_or(""))
                        .to_string();
                    let description = opt
                        .get("description")
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    Some((label, value, description))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Whether a JSON value counts as true in a binding context.
///
/// JS truthiness, not Rust's: an empty array or object is truthy there, and mods
/// are written against JS.
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().map(|f| f != 0.0).unwrap_or(true),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// Evaluates a binding expression against the current values.
pub fn evaluate(
    expr: &Value,
    values: &Map<String, Value>,
    expanded: &HashMap<String, bool>,
) -> Value {
    let Value::Array(parts) = expr else {
        return expr.clone();
    };
    let Some(op) = parts.first().and_then(Value::as_str) else {
        return expr.clone();
    };

    let arg = |i: usize| -> Value {
        parts
            .get(i)
            .map(|a| evaluate(a, values, expanded))
            .unwrap_or(Value::Null)
    };

    match op {
        "value" => {
            let id = parts.get(1).and_then(Value::as_str).unwrap_or_default();
            values.get(id).cloned().unwrap_or(Value::Null)
        }
        "expanded" => {
            let id = parts.get(1).and_then(Value::as_str).unwrap_or_default();
            Value::Bool(expanded.get(id).copied().unwrap_or(false))
        }
        "not" => Value::Bool(!truthy(&arg(1))),
        "and" => Value::Bool((1..parts.len()).all(|i| truthy(&arg(i)))),
        "or" => Value::Bool((1..parts.len()).any(|i| truthy(&arg(i)))),
        "eq" => Value::Bool(arg(1) == arg(2)),
        "neq" => Value::Bool(arg(1) != arg(2)),
        "lt" | "lte" | "gt" | "gte" => {
            let (Some(a), Some(b)) = (number(&arg(1)), number(&arg(2))) else {
                return Value::Bool(false);
            };
            let result = match op {
                "lt" => a < b,
                "lte" => a <= b,
                "gt" => a > b,
                _ => a >= b,
            };
            Value::Bool(result)
        }
        "in" => {
            let needle = arg(1);
            match arg(2) {
                Value::Array(haystack) => Value::Bool(haystack.contains(&needle)),
                _ => Value::Bool(false),
            }
        }
        "if" => {
            if truthy(&arg(1)) {
                arg(2)
            } else {
                arg(3)
            }
        }
        // An unknown operator is a mod bug. Failing to a literal is friendlier than
        // propagating something the UI cannot render.
        _ => expr.clone(),
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        Value::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
        _ => None,
    }
}

/// The configuration of one mod: schema in, resolved values out.
#[derive(Debug, Clone)]
pub struct ModConfig {
    fields: Vec<Value>,
    saved: Map<String, Value>,
    values: Map<String, Value>,
    expanded: HashMap<String, bool>,
    baseline: Map<String, Value>,
}

impl ModConfig {
    pub fn new(fields: Vec<Value>, saved: Map<String, Value>) -> Self {
        let mut config = Self {
            fields,
            saved: saved.clone(),
            values: Map::new(),
            expanded: HashMap::new(),
            baseline: saved.clone(),
        };
        config.recompute();
        config.baseline = config.values.clone();
        config
    }

    pub fn fields(&self) -> &[Value] {
        &self.fields
    }

    /// The values the mod will see, with defaults and overrides applied.
    pub fn values(&self) -> &Map<String, Value> {
        &self.values
    }

    /// The values the user has actually chosen (no defaults).
    pub fn saved(&self) -> &Map<String, Value> {
        &self.saved
    }

    pub fn is_dirty(&self) -> bool {
        self.values != self.baseline
    }

    pub fn mark_clean(&mut self) {
        self.baseline = self.values.clone();
    }

    pub fn get(&self, id: &str) -> Option<&Value> {
        self.values.get(id)
    }

    pub fn set(&mut self, id: &str, value: Value) {
        self.saved.insert(id.to_string(), value);
        self.recompute();
    }

    pub fn reset(&mut self) {
        self.saved.clear();
        self.recompute();
    }

    pub fn set_expanded(&mut self, id: &str, expanded: bool) {
        self.expanded.insert(id.to_string(), expanded);
    }

    pub fn is_expanded(&self, field: &Value) -> bool {
        let Some(id) = field_id(field) else {
            return false;
        };
        self.expanded.get(id).copied().unwrap_or_else(|| {
            field
                .get("defaultExpanded")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
    }

    /// Whether a field should be shown, given `visible` bindings.
    pub fn is_visible(&self, field: &Value) -> bool {
        match field.get("visible") {
            None => true,
            Some(expr) => truthy(&evaluate(expr, &self.values, &self.expanded)),
        }
    }

    /// Rebuilds the effective values: defaults, then saved, then overrides.
    pub fn recompute(&mut self) {
        for _ in 0..2 {
            let defaults = self.defaults();
            let mut values = defaults;
            for (key, value) in &self.saved {
                values.insert(key.clone(), value.clone());
            }
            let mut all = Vec::new();
            flatten(&self.fields, &mut all);
            for field in all {
                let Some(id) = field_id(field) else { continue };
                if let Some(expr) = field.get("overrideValue") {
                    let overridden = evaluate(expr, &values, &self.expanded);
                    if !overridden.is_null() {
                        values.insert(id.to_string(), overridden);
                    }
                }
            }
            self.values = values;
        }
    }

    fn defaults(&self) -> Map<String, Value> {
        let mut out = Map::new();
        let mut all = Vec::new();
        flatten(&self.fields, &mut all);
        for field in all {
            let Some(id) = field_id(field) else { continue };
            let Some(raw) = field.get("defaultValue") else {
                continue;
            };
            if raw.is_null() {
                continue;
            }
            // `defaultValue` may itself be a binding; it is resolved against what
            // exists so far so that "derive this field from that one" works.
            let value = match raw {
                Value::Array(_) => evaluate(raw, &out, &self.expanded),
                other => other.clone(),
            };
            if !value.is_null() {
                out.insert(id.to_string(), value);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn config() -> ModConfig {
        let fields: Vec<Value> = serde_json::from_value(json!([
            {
                "id": "mode",
                "type": "select",
                "name": "Mode",
                "defaultValue": "normal"
            },
            {
                "id": "custom",
                "type": "number",
                "name": "Custom",
                "defaultValue": 10,
                "visible": ["eq", ["value", "mode"], "custom"]
            },
            {
                "id": "tracker",
                "type": "checkbox",
                "name": "Tracker",
                "defaultValue": true
            },
            {
                "id": "forced",
                "type": "checkbox",
                "name": "Forced",
                "defaultValue": true,
                "overrideValue": ["if", ["not", ["value", "tracker"]], false, null]
            },
            {
                "id": "numsection",
                "type": "section",
                "name": "Numbers",
                "defaultExpanded": false,
                "children": [
                    { "id": "inner", "type": "number", "name": "Inner", "defaultValue": 5 }
                ]
            }
        ]))
        .unwrap();
        ModConfig::new(fields, Map::new())
    }

    #[test]
    fn applies_defaults_including_children() {
        let config = config();
        assert_eq!(config.get("mode"), Some(&json!("normal")));
        assert_eq!(config.get("inner"), Some(&json!(5)));
    }

    #[test]
    fn visibility_follows_bindings() {
        let mut config = config();
        let all = config.fields().to_vec();
        let custom = &all[1];
        assert!(!config.is_visible(custom));
        config.set("mode", json!("custom"));
        assert!(config.is_visible(&all[1]));
    }

    #[test]
    fn override_forces_and_releases() {
        let mut config = config();
        assert_eq!(config.get("forced"), Some(&json!(true)));
        config.set("tracker", json!(false));
        assert_eq!(config.get("forced"), Some(&json!(false)));
        config.set("forced", json!(true));
        // Still overridden: overrideValue wins over a saved value.
        assert_eq!(config.get("forced"), Some(&json!(false)));
        config.set("tracker", json!(true));
        assert_eq!(config.get("forced"), Some(&json!(true)));
    }

    #[test]
    fn dirty_tracks_changes() {
        let mut config = config();
        assert!(!config.is_dirty());
        config.set("inner", json!(7));
        assert!(config.is_dirty());
        config.mark_clean();
        assert!(!config.is_dirty());
    }

    #[test]
    fn comparisons_and_logic() {
        let values: Map<String, Value> = serde_json::from_value(json!({"n": 5})).unwrap();
        let expanded = HashMap::new();
        assert_eq!(evaluate(&json!(["lt", ["value", "n"], 10]), &values, &expanded), json!(true));
        assert_eq!(
            evaluate(&json!(["in", ["value", "n"], [5, 6]]), &values, &expanded),
            json!(true)
        );
        assert_eq!(
            evaluate(&json!(["and", true, ["not", false]]), &values, &expanded),
            json!(true)
        );
    }
}
