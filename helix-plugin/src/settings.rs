//! Settings schemas (`[settings]` in `plugin.toml`): the user's settings for
//! a plugin are checked against them, and defaults filled in, before the
//! plugin sees them.

use std::collections::BTreeMap;

use anyhow::{bail, Context};
use serde::Deserialize;

/// One setting a plugin declares.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct SettingSpec {
    #[serde(rename = "type")]
    pub kind: SettingType,
    /// Used when the user doesn't set it.
    pub default: Option<toml::Value>,
    /// For `enum`: the allowed values.
    #[serde(default)]
    pub values: Vec<String>,
    #[serde(default)]
    pub description: String,
    /// For numbers: the range.
    pub min: Option<f64>,
    pub max: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SettingType {
    Boolean,
    String,
    Integer,
    Number,
    StringList,
    Enum,
}

impl SettingType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Boolean => "boolean",
            Self::String => "string",
            Self::Integer => "integer",
            Self::Number => "number",
            Self::StringList => "string-list",
            Self::Enum => "enum",
        }
    }
}

pub type Schema = BTreeMap<String, SettingSpec>;

impl SettingSpec {
    /// Whether `value` fits; `Err` says why not.
    fn check(&self, value: &serde_json::Value) -> Result<(), String> {
        use serde_json::Value;
        let fits = match (self.kind, value) {
            (SettingType::Boolean, Value::Bool(_)) => true,
            (SettingType::String, Value::String(_)) => true,
            (SettingType::Integer, Value::Number(n)) => n.is_i64() || n.is_u64(),
            (SettingType::Number, Value::Number(_)) => true,
            (SettingType::StringList, Value::Array(items)) => items.iter().all(Value::is_string),
            (SettingType::Enum, Value::String(s)) => {
                if !self.values.contains(s) {
                    return Err(format!("'{s}' is not one of {}", self.values.join(", ")));
                }
                true
            }
            _ => false,
        };
        if !fits {
            return Err(format!("expected {}, got {value}", self.kind.as_str()));
        }
        if let Some(n) = value.as_f64() {
            if self.min.is_some_and(|min| n < min) || self.max.is_some_and(|max| n > max) {
                return Err(format!(
                    "{n} is outside {}..={}",
                    self.min.map_or("".into(), |m| m.to_string()),
                    self.max.map_or("".into(), |m| m.to_string())
                ));
            }
        }
        Ok(())
    }
}

/// Check a schema itself (from `plugin.toml`): enums have values, defaults
/// fit.
pub fn check_schema(schema: &Schema) -> anyhow::Result<()> {
    for (key, spec) in schema {
        if spec.kind == SettingType::Enum && spec.values.is_empty() {
            bail!("setting '{key}': an enum needs values");
        }
        if let Some(default) = &spec.default {
            let default = serde_json::to_value(default)?;
            spec.check(&default)
                .map_err(|err| anyhow::anyhow!("setting '{key}': default: {err}"))?;
        }
    }
    Ok(())
}

/// The user's `config` (a JSON object) checked against `schema`, with the
/// defaults of unset settings filled in. Without a schema it is as it was.
pub fn apply(plugin: &str, schema: &Schema, config: &str) -> anyhow::Result<String> {
    if schema.is_empty() {
        return Ok(config.to_string());
    }
    let mut object: serde_json::Map<String, serde_json::Value> = serde_json::from_str(config)
        .with_context(|| format!("[{plugin}] settings must be a table"))?;
    for (key, value) in &object {
        let Some(spec) = schema.get(key) else {
            let known: Vec<&str> = schema.keys().map(String::as_str).collect();
            bail!(
                "[{plugin}] unknown setting '{key}' (the plugin has: {})",
                known.join(", ")
            );
        };
        spec.check(value)
            .map_err(|err| anyhow::anyhow!("[{plugin}] {key}: {err}"))?;
    }
    for (key, spec) in schema {
        if let (false, Some(default)) = (object.contains_key(key), &spec.default) {
            object.insert(key.clone(), serde_json::to_value(default)?);
        }
    }
    Ok(serde_json::Value::Object(object).to_string())
}
