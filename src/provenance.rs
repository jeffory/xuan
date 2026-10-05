//! How a generated layer was made: the model, sampler and service request a
//! plugin reports with an `image` output. It is stored beside `Generated` in
//! `.xuan` files (format 8) and shown read-only in the layer's info.
//!
//! The record is small and strictly shaped, and secrets never enter it: keys
//! that look like credentials, the plugin's own secret names and any value
//! containing one of its secret values are dropped (see [`Redactor`]).
use std::collections::BTreeMap;

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Longest string, in bytes.
pub const MAX_STRING: usize = 256;
/// Largest serialized record, in bytes.
pub const MAX_BYTES: usize = 8 * 1024;
/// Levels of objects and arrays inside `extra`, counting `extra` itself.
pub const MAX_DEPTH: usize = 4;
/// Entries in one object or array of `extra`.
pub const MAX_ENTRIES: usize = 32;
/// Longest key, in bytes.
pub const MAX_KEY: usize = 64;
const MAX_STEPS: u32 = 1_000_000;
const MAX_CFG: f64 = 1.0e6;

/// Model and sampler details of one generated layer. Unknown keys are
/// rejected; anything else goes under `extra`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    /// The resolved model, as the service named it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// A hash of the model as the service reports it, in its own format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_hash: Option<String>,
    /// SHA-256 of the weights file: 64 hexadecimal digits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weights_sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sampler: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheduler: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfg: Option<f64>,
    /// The service that produced the layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub service: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Anything else worth keeping: strings, finite numbers, booleans, nulls
    /// and nested objects or arrays up to [`MAX_DEPTH`] levels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extra: BTreeMap<String, Value>,
}

/// What to keep out of a record: the plugin's declared secret names and the
/// values it was given. Generic credential-like key names are always dropped.
#[derive(Clone, Debug, Default)]
pub struct Redactor {
    names: Vec<String>,
    values: Vec<String>,
}

/// Fragments of normalized (lowercase, letters and digits only) key names
/// that mark a credential.
const SECRET_FRAGMENTS: [&str; 10] = [
    "apikey",
    "token",
    "authorization",
    "password",
    "passwd",
    "secret",
    "credential",
    "bearer",
    "cookie",
    "privatekey",
];

/// Secret values shorter than this are not searched for: they would remove
/// harmless text, and nobody's secret is that short.
const MIN_SECRET: usize = 4;

fn normalize(key: &str) -> String {
    key.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

impl Redactor {
    pub fn new<'a>(
        names: impl IntoIterator<Item = &'a str>,
        values: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        Self {
            names: names
                .into_iter()
                .map(normalize)
                .filter(|n| !n.is_empty())
                .collect(),
            values: values
                .into_iter()
                .filter(|v| v.len() >= MIN_SECRET)
                .map(String::from)
                .collect(),
        }
    }

    pub fn is_secret_key(&self, key: &str) -> bool {
        let key = normalize(key);
        SECRET_FRAGMENTS.iter().any(|f| key.contains(f)) || self.names.contains(&key)
    }

    fn leaks(&self, text: &str) -> bool {
        self.values.iter().any(|v| text.contains(v.as_str()))
    }

    /// `value` without secret-looking keys or leaking text, counting each
    /// entry removed. `None` when `value` itself is removed.
    fn strip(&self, value: Value, removed: &mut usize) -> Option<Value> {
        match value {
            Value::String(text) if self.leaks(&text) => {
                *removed += 1;
                None
            }
            Value::Array(items) => Some(Value::Array(
                items
                    .into_iter()
                    .filter_map(|item| self.strip(item, removed))
                    .collect(),
            )),
            Value::Object(map) => Some(Value::Object(
                map.into_iter()
                    .filter_map(|(key, item)| {
                        if self.is_secret_key(&key) || self.leaks(&key) {
                            *removed += 1;
                            return None;
                        }
                        self.strip(item, removed).map(|item| (key, item))
                    })
                    .collect::<Map<_, _>>(),
            )),
            other => Some(other),
        }
    }
}

fn check_text(what: &str, text: &str) -> Result<()> {
    ensure!(
        text.len() <= MAX_STRING,
        "provenance {what} is longer than {MAX_STRING} bytes"
    );
    ensure!(
        !text.chars().any(char::is_control),
        "provenance {what} holds control characters"
    );
    Ok(())
}

fn check_extra(value: &Value, depth: usize) -> Result<()> {
    match value {
        Value::Null | Value::Bool(_) => {}
        Value::Number(n) => ensure!(
            n.as_f64().is_some_and(f64::is_finite),
            "provenance numbers must be finite"
        ),
        Value::String(text) => check_text("text", text)?,
        Value::Array(items) => {
            ensure!(
                depth <= MAX_DEPTH,
                "provenance nests deeper than {MAX_DEPTH} levels"
            );
            ensure!(
                items.len() <= MAX_ENTRIES,
                "provenance lists hold at most {MAX_ENTRIES} entries"
            );
            for item in items {
                check_extra(item, depth + 1)?;
            }
        }
        Value::Object(map) => {
            ensure!(
                depth <= MAX_DEPTH,
                "provenance nests deeper than {MAX_DEPTH} levels"
            );
            ensure!(
                map.len() <= MAX_ENTRIES,
                "provenance objects hold at most {MAX_ENTRIES} entries"
            );
            for (key, item) in map {
                ensure!(
                    !key.is_empty() && key.len() <= MAX_KEY,
                    "provenance keys are 1 to {MAX_KEY} bytes"
                );
                check_text("key", key)?;
                check_extra(item, depth + 1)?;
            }
        }
    }
    Ok(())
}

impl Provenance {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The record a plugin reported: secrets removed, then checked against
    /// the schema. Also returns how many entries were removed as secrets (their
    /// names and values are never reported). `None` when nothing is left.
    pub fn from_plugin(value: &Value, redactor: &Redactor) -> Result<(Option<Self>, usize)> {
        let Some(map) = value.as_object() else {
            bail!("provenance must be an object");
        };
        let mut removed = 0;
        let cleaned = redactor
            .strip(Value::Object(map.clone()), &mut removed)
            .unwrap_or_default();
        let record: Self = serde_json::from_value(cleaned).map_err(|error| {
            let message = error.to_string();
            anyhow::anyhow!(
                "malformed provenance: {}",
                message.chars().take(200).collect::<String>()
            )
        })?;
        record.validate()?;
        Ok(((!record.is_empty()).then_some(record), removed))
    }

    /// Check the schema; run for plugin results and when a project loads.
    /// Credential-like key names are refused here as well.
    pub fn validate(&self) -> Result<()> {
        for (what, text) in [
            ("model", &self.model),
            ("model_hash", &self.model_hash),
            ("sampler", &self.sampler),
            ("scheduler", &self.scheduler),
            ("service", &self.service),
            ("request_id", &self.request_id),
        ] {
            if let Some(text) = text {
                check_text(what, text)?;
            }
        }
        if let Some(hash) = &self.weights_sha256 {
            ensure!(
                hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()),
                "provenance weights_sha256 must be 64 hexadecimal digits"
            );
        }
        ensure!(
            self.steps.is_none_or(|steps| steps <= MAX_STEPS),
            "provenance steps must be at most {MAX_STEPS}"
        );
        ensure!(
            self.cfg
                .is_none_or(|cfg| cfg.is_finite() && cfg.abs() <= MAX_CFG),
            "provenance cfg must be a finite number"
        );
        let extra = Value::Object(self.extra.clone().into_iter().collect());
        check_extra(&extra, 1)?;
        let mut removed = 0;
        Redactor::default().strip(extra, &mut removed);
        ensure!(removed == 0, "provenance must not hold secrets");
        ensure!(
            serde_json::to_vec(self)?.len() <= MAX_BYTES,
            "provenance is larger than {} KiB",
            MAX_BYTES / 1024
        );
        Ok(())
    }

    /// Pretty JSON, for display and the Copy button.
    pub fn to_json(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_default()
    }

    /// The set fields as (name, text) rows, `extra` as one row per entry.
    pub fn rows(&self) -> Vec<(String, String)> {
        let mut rows = Vec::new();
        let mut add = |name: &str, value: Option<String>| {
            if let Some(value) = value {
                rows.push((name.to_string(), value));
            }
        };
        add("model", self.model.clone());
        add("model_hash", self.model_hash.clone());
        add("weights_sha256", self.weights_sha256.clone());
        add("sampler", self.sampler.clone());
        add("scheduler", self.scheduler.clone());
        add("steps", self.steps.map(|v| v.to_string()));
        add("seed", self.seed.map(|v| v.to_string()));
        add("cfg", self.cfg.map(|v| v.to_string()));
        add("service", self.service.clone());
        add("request_id", self.request_id.clone());
        for (key, value) in &self.extra {
            let shown = match value {
                Value::String(text) => text.clone(),
                other => other.to_string(),
            };
            add(&format!("extra.{key}"), Some(shown));
        }
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn parse(value: Value) -> Result<(Option<Provenance>, usize)> {
        Provenance::from_plugin(&value, &Redactor::default())
    }

    #[test]
    fn a_full_record_is_kept() {
        let hash = "ab".repeat(32);
        let (record, removed) = parse(json!({
            "model": "sdxl.safetensors", "model_hash": "AutoV2:1234", "weights_sha256": hash,
            "sampler": "euler", "scheduler": "karras", "steps": 30, "seed": u64::MAX,
            "cfg": 7.5, "service": "comfy cloud", "request_id": "r-1",
            "extra": {"lora": ["a", "b"], "nested": {"on": true, "n": null}},
        }))
        .unwrap();
        let record = record.unwrap();
        assert_eq!(removed, 0);
        assert_eq!(record.seed, Some(u64::MAX));
        assert_eq!(record.steps, Some(30));
        assert_eq!(record.extra["lora"], json!(["a", "b"]));
        let again: Provenance = serde_json::from_str(&record.to_json()).unwrap();
        assert_eq!(again, record);
        assert!(
            record
                .rows()
                .iter()
                .any(|(k, v)| k == "extra.lora" && v == r#"["a","b"]"#)
        );
    }

    #[test]
    fn empty_and_null_records_store_nothing() {
        assert_eq!(parse(json!({})).unwrap().0, None);
        assert_eq!(parse(json!({"model": null})).unwrap().0, None);
        assert!(parse(json!("text")).is_err());
        assert!(parse(json!([1])).is_err());
    }

    #[test]
    fn unknown_keys_and_wrong_types_are_rejected() {
        for bad in [
            json!({"modle": "x"}),
            json!({"model": 3}),
            json!({"steps": "30"}),
            json!({"steps": -1}),
            json!({"steps": 2_000_000}),
            json!({"seed": -5}),
            json!({"seed": 1.5}),
            json!({"cfg": "high"}),
            json!({"cfg": 1.0e9}),
            json!({"weights_sha256": "abc"}),
            json!({"extra": [1]}),
            json!({"extra": {"": 1}}),
        ] {
            assert!(parse(bad.clone()).is_err(), "{bad}");
        }
    }

    #[test]
    fn strings_are_capped() {
        assert!(parse(json!({"model": "x".repeat(MAX_STRING)})).is_ok());
        assert!(parse(json!({"model": "x".repeat(MAX_STRING + 1)})).is_err());
        assert!(parse(json!({"extra": {"k": "x".repeat(MAX_STRING + 1)}})).is_err());
        assert!(parse(json!({"extra": {"k".repeat(MAX_KEY + 1): 1}})).is_err());
        assert!(parse(json!({"model": "a\nb"})).is_err());
    }

    #[test]
    fn depth_and_width_are_limited() {
        // `extra` is level one, so up to four levels of containers fit.
        assert!(parse(json!({"extra": {"a": {"b": {"c": {"d": 1}}}}})).is_ok());
        assert!(parse(json!({"extra": {"a": {"b": {"c": {"d": {"e": 1}}}}}})).is_err());
        assert!(parse(json!({"extra": {"a": [[[1]]]}})).is_ok());
        assert!(parse(json!({"extra": {"a": [[[[1]]]]}})).is_err());
        let wide: Map<String, Value> = (0..=MAX_ENTRIES)
            .map(|i| (format!("k{i}"), json!(i)))
            .collect();
        assert!(parse(json!({"extra": wide})).is_err());
        let list = vec![0; MAX_ENTRIES + 1];
        assert!(parse(json!({"extra": {"a": list}})).is_err());
    }

    #[test]
    fn the_total_size_is_limited() {
        let big: Map<String, Value> = (0..MAX_ENTRIES)
            .map(|i| (format!("k{i}"), json!("v".repeat(MAX_STRING))))
            .collect();
        // 32 entries of about 265 bytes are just over 8 KiB.
        assert!(parse(json!({"extra": big})).is_err());
        let ok: Map<String, Value> = (0..16)
            .map(|i| (format!("k{i}"), json!("v".repeat(MAX_STRING))))
            .collect();
        assert!(parse(json!({"extra": ok})).is_ok());
    }

    #[test]
    fn numbers_must_be_finite() {
        for cfg in [f64::NAN, f64::INFINITY] {
            let record = Provenance {
                cfg: Some(cfg),
                ..Provenance::default()
            };
            assert!(record.validate().is_err());
        }
    }

    #[test]
    fn secret_looking_keys_are_stripped_without_echoing_values() {
        let (record, removed) = parse(json!({
            "model": "m",
            "api_key": "sk-live-1234567890",
            "extra": {
                "Authorization": "Bearer abc", "x-Api-Key": "k", "my_password": "p",
                "client_secret": "s", "tokenCount": 3, "kept": 1,
                "deep": {"access_token": "t", "ok": true},
            },
        }))
        .unwrap();
        let record = record.unwrap();
        assert_eq!(removed, 7);
        let json = record.to_json();
        assert!(
            !json.contains("sk-live") && !json.contains("Bearer"),
            "{json}"
        );
        assert_eq!(record.extra["kept"], json!(1));
        assert_eq!(record.extra["deep"], json!({"ok": true}));
    }

    #[test]
    fn declared_secret_names_and_values_are_stripped() {
        let redactor = Redactor::new(["comfy_key"], ["hunter2-secret-value"]);
        let (record, removed) = Provenance::from_plugin(
            &json!({
                "model": "m",
                "request_id": "hunter2-secret-value",
                "service": "ok",
                "extra": {
                    "comfy_key": "x",
                    "note": "sent hunter2-secret-value!",
                    "list": ["fine", "hunter2-secret-value"],
                },
            }),
            &redactor,
        )
        .unwrap();
        let record = record.unwrap();
        assert_eq!(removed, 4);
        assert_eq!(record.request_id, None);
        assert_eq!(record.extra["list"], json!(["fine"]));
        assert!(!record.to_json().contains("hunter2"));
        // A secret next to a field that fails the schema never shows up in the error.
        let error = Provenance::from_plugin(
            &json!({"steps": "x", "model": "hunter2-secret-value", "hunter2-secret-value": 1}),
            &redactor,
        )
        .unwrap_err()
        .to_string();
        assert!(!error.contains("hunter2"), "{error}");
    }

    #[test]
    fn stored_secrets_are_refused_on_load() {
        let mut record = Provenance::default();
        record.extra.insert("api_key".into(), json!("x"));
        assert!(record.validate().is_err());
    }
}
