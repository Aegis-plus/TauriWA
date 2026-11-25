//! JSON-with-comments settings file.
//!
//! Values are read once at startup. `set` edits the original text in place so
//! that the user's comments and formatting survive (same behaviour as the
//! `jsonc-parser` `modify` used by the Electron version).

use std::path::{Path, PathBuf};

use jsonc_parser::cst::{CstInputValue, CstRootNode};
use jsonc_parser::ParseOptions;
use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

pub struct JsonConfig {
    path: PathBuf,
    data: Map<String, Value>,
    raw_text: String,
    parse_error: Option<String>,
}

fn parse_options() -> ParseOptions {
    ParseOptions {
        allow_comments: true,
        allow_loose_object_property_names: false,
        allow_trailing_commas: true,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    }
}

impl JsonConfig {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        let mut config = Self {
            path: path.into(),
            data: Map::new(),
            raw_text: "{}".to_owned(),
            parse_error: None,
        };
        config.load();
        config
    }

    pub fn file(&self) -> &Path {
        &self.path
    }

    pub fn parse_error(&self) -> Option<&str> {
        self.parse_error.as_deref()
    }

    pub fn raw(&self, key: &str) -> Option<&Value> {
        self.data.get(key)
    }

    /// Typed getter. A missing key, or a value of the wrong type, gives `default`.
    pub fn get<T: DeserializeOwned>(&self, key: &str, default: T) -> T {
        match self.data.get(key) {
            None => default,
            Some(value) => match serde_json::from_value(value.clone()) {
                Ok(v) => v,
                Err(err) => {
                    log::warn!(
                        "config {}: invalid value for \"{key}\": {err}",
                        self.path.display()
                    );
                    default
                }
            },
        }
    }

    pub fn set(&mut self, key: &str, value: Value) {
        self.data.insert(key.to_owned(), value.clone());
        match CstRootNode::parse(&self.raw_text, &parse_options()) {
            Ok(root) => {
                let object = root.object_value_or_set();
                let input = to_cst_input(&value);
                match object.get(key) {
                    Some(prop) => prop.set_value(input),
                    None => {
                        object.append(key, input);
                    }
                }
                self.raw_text = root.to_string();
            }
            Err(err) => {
                // The file could not be parsed at load time either: start from scratch
                // rather than corrupting it further.
                log::warn!(
                    "config {}: rewriting unparsable file: {err}",
                    self.path.display()
                );
                self.raw_text =
                    serde_json::to_string_pretty(&self.data).unwrap_or_else(|_| "{}".into());
            }
        }
        self.save();
    }

    fn load(&mut self) {
        let text = match std::fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(err) => {
                if err.kind() != std::io::ErrorKind::NotFound {
                    log::warn!("cannot read {}: {err}", self.path.display());
                }
                return;
            }
        };

        match CstRootNode::parse(&text, &parse_options()) {
            Ok(root) => {
                match root.value().and_then(|v| v.to_serde_value()) {
                    Some(Value::Object(map)) => self.data = map,
                    None => {}
                    Some(_) => {
                        self.parse_error = Some("the top-level value must be an object".to_owned());
                    }
                }
                if self.parse_error.is_none() {
                    self.raw_text = text;
                }
            }
            Err(err) => {
                self.parse_error = Some(err.to_string());
                // keep the text so a later `set` does not wipe the file silently
                self.raw_text = text;
            }
        }
    }

    fn save(&self) {
        if let Some(dir) = self.path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Err(err) = std::fs::write(&self.path, &self.raw_text) {
            log::error!("Failed to save config to {}: {err}", self.path.display());
        }
    }
}

fn to_cst_input(value: &Value) -> CstInputValue {
    match value {
        Value::Null => CstInputValue::Null,
        Value::Bool(b) => CstInputValue::Bool(*b),
        Value::Number(n) => CstInputValue::Number(n.to_string()),
        Value::String(s) => CstInputValue::String(s.clone()),
        Value::Array(a) => CstInputValue::Array(a.iter().map(to_cst_input).collect()),
        Value::Object(o) => CstInputValue::Object(
            o.iter()
                .map(|(k, v)| (k.clone(), to_cst_input(v)))
                .collect(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn temp_file(name: &str, content: Option<&str>) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("elecwhat-test-{}-{name}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("config.json");
        let _ = std::fs::remove_file(&file);
        if let Some(content) = content {
            std::fs::write(&file, content).unwrap();
        }
        file
    }

    #[test]
    fn missing_file_uses_defaults() {
        let file = temp_file("missing", None);
        let mut config = JsonConfig::new(&file);
        assert!(config.parse_error().is_none());
        assert!(config.get("dbus", true));
        config.set("quit-on-close", json!(true));
        let reread = JsonConfig::new(&file);
        assert!(reread.get("quit-on-close", false));
    }

    #[test]
    fn comments_survive_set() {
        let file = temp_file(
            "comments",
            Some("{\n  // keep me\n  \"menu-bar\": false, /* and me */\n  \"quit-on-close\": false,\n}\n"),
        );
        let mut config = JsonConfig::new(&file);
        assert!(config.parse_error().is_none());
        assert!(!config.get("menu-bar", true));
        config.set("quit-on-close", json!(true));
        config.set("window-bounds", json!({"width": 10, "height": 20}));
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("// keep me"), "{text}");
        assert!(text.contains("/* and me */"), "{text}");
        let reread = JsonConfig::new(&file);
        assert!(reread.get("quit-on-close", false));
        assert_eq!(
            reread.raw("window-bounds"),
            Some(&json!({"width": 10, "height": 20}))
        );
    }

    #[test]
    fn parse_error_is_reported() {
        let file = temp_file("broken", Some("{ \"a\": }"));
        let config = JsonConfig::new(&file);
        assert!(config.parse_error().is_some());
        assert_eq!(config.get("a", 3), 3);
    }

    #[test]
    fn wrong_type_gives_default() {
        let file = temp_file("types", Some("{ \"dbus\": \"yes\" }"));
        let config = JsonConfig::new(&file);
        assert!(config.get("dbus", true));
    }
}
