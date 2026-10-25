//! Provider selection (DESIGN.md A3 / A3b): which backend a command should use.
//!
//! `auto`: configured API (real key) > usable local Ollama model > offline grammar engine.
//! `ai`: API only. `offline`: grammar engine only, never any network request.

use super::config::{Config, Mode};
use super::ollama::{self, Probe, Selection};

/// The backend chosen for one command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Provider {
    /// The configured Anthropic-compatible API.
    Api,
    /// A local Ollama model reached through the Anthropic-messages client.
    Ollama { base: String, model: String },
    /// The offline grammar engine.
    Offline(Offline),
    /// `ai` mode without a configured API (only reachable from `status`).
    Unconfigured,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offline {
    /// `offline` mode was requested explicitly.
    Requested,
    /// `auto` mode found no API key and no usable Ollama model. `ollama_note` explains why
    /// Ollama was rejected when it was reachable.
    NotConfigured { ollama_note: Option<String> },
}

/// Whether auto mode needs an Ollama probe to decide the provider.
pub fn needs_probe(cfg: &Config) -> bool {
    cfg.mode == Mode::Auto && cfg.api.is_none()
}

/// Decides the provider from the configuration and an optional probe result.
pub fn pick(cfg: &Config, probe: Option<&Probe>) -> Provider {
    match cfg.mode {
        Mode::Offline => Provider::Offline(Offline::Requested),
        Mode::Ai => {
            if cfg.api.is_some() {
                Provider::Api
            } else {
                Provider::Unconfigured
            }
        }
        Mode::Auto => {
            if cfg.api.is_some() {
                return Provider::Api;
            }
            match probe.map(|p| &p.selection) {
                Some(Selection::Usable(model)) => Provider::Ollama {
                    base: cfg.ollama.url.clone(),
                    model: model.clone(),
                },
                Some(Selection::Unusable(reason)) => Provider::Offline(Offline::NotConfigured {
                    ollama_note: Some(reason.clone()),
                }),
                Some(Selection::Disabled) | Some(Selection::Unreachable) | None => {
                    Provider::Offline(Offline::NotConfigured { ollama_note: None })
                }
            }
        }
    }
}

/// Resolves the provider, probing Ollama (once) only when auto mode needs it.
pub fn resolve(cfg: &Config) -> Provider {
    let probe = if needs_probe(cfg) {
        Some(ollama::probe(&cfg.ollama))
    } else {
        None
    };
    pick(cfg, probe.as_ref())
}

/// Short description for `quill status`: `api`, `ollama (<model>)`, `offline`, or `none`.
pub fn describe(provider: &Provider) -> String {
    match provider {
        Provider::Api => "api".to_string(),
        Provider::Ollama { model, .. } => format!("ollama ({model})"),
        Provider::Offline(_) => "offline".to_string(),
        Provider::Unconfigured => "none (ai mode needs an API key)".to_string(),
    }
}
