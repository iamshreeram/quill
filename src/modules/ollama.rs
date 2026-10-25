//! Local Ollama discovery (DESIGN.md A3). Ollama exposes an Anthropic-compatible
//! `/v1/messages` endpoint next to its native `/api/tags` and `/api/ps` routes.
//!
//! Probes use short timeouts, never follow redirects, cap the body size and never print
//! the URL or response bodies in errors.

use super::config::{
    ApiConfig, AuthStyle, Config, OllamaConfig, DEFAULT_ANTHROPIC_VERSION, DEFAULT_TIMEOUT_SECS,
};
use serde_json::Value;
use std::io::Read;
use std::sync::Arc;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_millis(400);
const TOTAL_TIMEOUT: Duration = Duration::from_millis(1500);
const BODY_CAP: usize = 1024 * 1024;
/// Minimum whole-request timeout for Ollama completions (local models can be slow to load).
const MIN_REQUEST_TIMEOUT_SECS: u64 = 120;
/// Key sent to Ollama (it ignores the value, but the header is required).
pub const OLLAMA_KEY: &str = "ollama";

/// Outcome of probing the local Ollama server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Selection {
    /// `[ollama] enabled = false`: not probed.
    Disabled,
    /// No valid `/api/tags` answer (refused, timeout, HTTP error, bad JSON).
    Unreachable,
    /// Reachable, but no usable model; the string says why (safe to print).
    Unusable(String),
    /// Reachable with a model to use.
    Usable(String),
}

/// Result of one probe: the selection plus the installed model names for display.
#[derive(Debug, Clone)]
pub struct Probe {
    pub selection: Selection,
    pub tags: Vec<String>,
}

/// Probes the server once (tags, and running models only when no model is configured).
pub fn probe(cfg: &OllamaConfig) -> Probe {
    if !cfg.enabled {
        return Probe {
            selection: Selection::Disabled,
            tags: Vec::new(),
        };
    }
    let tags = match get_json(&cfg.url, "/api/tags").and_then(|v| names(&v)) {
        Some(tags) => tags,
        None => {
            return Probe {
                selection: Selection::Unreachable,
                tags: Vec::new(),
            }
        }
    };
    let running = if cfg.model.is_empty() {
        get_json(&cfg.url, "/api/ps")
            .and_then(|v| names(&v))
            .unwrap_or_default()
    } else {
        Vec::new()
    };
    let selection = match select_model(&cfg.model, &tags, &running) {
        Ok(model) => Selection::Usable(model),
        Err(reason) => Selection::Unusable(reason),
    };
    Probe { selection, tags }
}

/// Model choice: the configured model if installed; otherwise the first running non-embedding
/// model, then the first installed non-embedding model.
pub fn select_model(
    configured: &str,
    tags: &[String],
    running: &[String],
) -> Result<String, String> {
    if !configured.is_empty() {
        return if tags.iter().any(|t| t == configured) {
            Ok(configured.to_string())
        } else {
            Err(format!(
                "model \"{configured}\" is not pulled (run: ollama pull {configured})"
            ))
        };
    }
    let usable = |name: &&String| !name.to_ascii_lowercase().contains("embed");
    if let Some(model) = running.iter().find(usable) {
        return Ok(model.clone());
    }
    if let Some(model) = tags.iter().find(usable) {
        return Ok(model.clone());
    }
    if tags.is_empty() {
        Err("no models are pulled (run: ollama pull <model>)".to_string())
    } else {
        Err("only embedding models are installed (run: ollama pull <model>)".to_string())
    }
}

/// Collects `models[].name` from an Ollama JSON document.
fn names(doc: &Value) -> Option<Vec<String>> {
    let models = doc.get("models")?.as_array()?;
    Some(
        models
            .iter()
            .filter_map(|m| m.get("name").and_then(Value::as_str).map(str::to_string))
            .collect(),
    )
}

/// GET `{base}{path}` with the probe limits. Any failure yields `None`.
fn get_json(base: &str, path: &str) -> Option<Value> {
    let tls = native_tls::TlsConnector::builder().build().ok()?;
    let agent = ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .redirects(0)
        .timeout_connect(CONNECT_TIMEOUT)
        .timeout(TOTAL_TIMEOUT)
        .build();
    let response = agent.get(&format!("{base}{path}")).call().ok()?;
    if response.status() != 200 {
        return None;
    }
    let mut buf = Vec::new();
    response
        .into_reader()
        .take(BODY_CAP as u64 + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() > BODY_CAP {
        return None;
    }
    serde_json::from_slice(&buf).ok()
}

/// A fresh API configuration for talking to Ollama through the Anthropic-messages client.
/// Only transform settings and context travel with it: no api.key, api.headers or
/// accept_invalid_certs from the Anthropic configuration are ever copied.
pub fn request_config(cfg: &Config, base: &str, model: &str) -> Config {
    let configured = cfg
        .api
        .as_ref()
        .map(|api| api.timeout_secs)
        .unwrap_or(DEFAULT_TIMEOUT_SECS as u64);
    let mut out = cfg.clone();
    out.api = Some(ApiConfig {
        url: format!("{}/v1/messages", base.trim_end_matches('/')),
        key: OLLAMA_KEY.to_string(),
        model: model.to_string(),
        anthropic_version: DEFAULT_ANTHROPIC_VERSION.to_string(),
        auth: AuthStyle::XApiKey,
        timeout_secs: configured.max(MIN_REQUEST_TIMEOUT_SECS),
        accept_invalid_certs: false,
        headers: Vec::new(),
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_string()).collect()
    }

    #[test]
    fn configured_model_must_be_installed() {
        assert_eq!(
            select_model("qwen3:30b", &s(&["qwen3:30b"]), &[]).unwrap(),
            "qwen3:30b"
        );
        let err = select_model("nope:1b", &s(&["qwen3:30b"]), &[]).unwrap_err();
        assert!(err.contains("nope:1b"), "{err}");
    }

    #[test]
    fn running_non_embedding_model_wins_then_first_installed() {
        let tags = s(&["embeddinggemma-2:latest", "llama3:8b", "qwen3:30b"]);
        assert_eq!(
            select_model("", &tags, &s(&["qwen3:30b"])).unwrap(),
            "qwen3:30b"
        );
        assert_eq!(select_model("", &tags, &[]).unwrap(), "llama3:8b");
        assert_eq!(
            select_model("", &tags, &s(&["embeddinggemma-2:latest"])).unwrap(),
            "llama3:8b"
        );
    }

    #[test]
    fn embedding_only_is_unusable_with_reason() {
        let err = select_model("", &s(&["embeddinggemma-2:latest"]), &[]).unwrap_err();
        assert!(err.contains("embedding"), "{err}");
        let err = select_model("", &[], &[]).unwrap_err();
        assert!(err.contains("no models"), "{err}");
    }

    #[test]
    fn names_reads_models_array_only() {
        let doc: Value =
            serde_json::from_str(r#"{"models":[{"name":"a"},{"model":"x"}]}"#).unwrap();
        assert_eq!(names(&doc).unwrap(), s(&["a"]));
        let bad: Value = serde_json::from_str(r#"{"other":1}"#).unwrap();
        assert!(names(&bad).is_none());
    }
}
