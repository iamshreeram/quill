//! Claude API client — reads all settings from config.

use super::config::Config;
use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct Request {
    model: String,
    system: String,
    messages: Vec<Message>,
    max_tokens: u32,
    stream: bool,
}

#[derive(Serialize)]
struct Message {
    role: &'static str,
    content: Vec<Content>,
}

#[derive(Serialize)]
struct Content {
    r#type: &'static str,
    text: String,
}

#[derive(Deserialize)]
struct Response {
    content: Vec<ResponseBlock>,
}

#[derive(Deserialize)]
struct ResponseBlock {
    text: Option<String>,
}

/// Build the full system prompt by injecting user context.
fn build_system_prompt(cfg: &Config, user_context: &str) -> String {
    let base = &cfg.transform.system_prompt;
    if user_context.trim().is_empty() {
        return base.clone();
    }
    format!(
        "{base}\n\n\
         The user has provided the following writing style preferences. \
         Follow these closely when transforming their text:\n\n\
         {user_context}"
    )
}

/// Call the configured Claude model and return the transformed text.
pub fn transform(
    cfg: &Config,
    instruction: &str,
    user_context: &str,
    text: &str,
) -> Result<String, String> {
    let user_msg = format!("Instruction: {instruction}\n\nText to transform:\n{text}");

    let body = Request {
        model: cfg.api.model.clone(),
        system: build_system_prompt(cfg, user_context),
        messages: vec![Message {
            role: "user",
            content: vec![Content {
                r#type: "text",
                text: user_msg,
            }],
        }],
        max_tokens: cfg.transform.max_tokens,
        stream: false,
    };

    let tls = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(cfg.api.accept_invalid_certs)
        .build()
        .map_err(|e| format!("TLS error: {e}"))?;
    let agent = ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(tls))
        .build();

    let resp = agent
        .post(&cfg.api.url)
        .set("Content-Type", "application/json")
        .set("x-api-key", &cfg.api.key)
        .set("anthropic-version", &cfg.api.anthropic_version)
        .set("x-user-type", &cfg.api.user_type)
        .set("x-user-name", &cfg.api.user_name)
        .send_json(&body)
        .map_err(|e| format!("API error: {e}"))?;

    let parsed: Response = resp.into_json().map_err(|e| format!("Parse error: {e}"))?;

    parsed
        .content
        .into_iter()
        .find_map(|b| b.text)
        .ok_or_else(|| "No text in response".to_string())
}

/// Ask Claude to analyze transformation history and produce a writing style summary.
pub fn analyze_style(
    cfg: &Config,
    history_text: &str,
    existing_context: &str,
) -> Result<String, String> {
    let system = "\
        You are a writing style analyst. You will receive a list of before/after \
        text transformation pairs. Analyze the patterns to understand what the user \
        prefers: tone, length, formality, word choices, sentence structure, etc.\n\n\
        Output a clean markdown document titled '# Quill Writing Context' with a \
        '## Preferences' section containing bullet points describing the user's \
        writing style preferences. Be specific and actionable.\n\n\
        If existing preferences are provided, update and refine them — don't start \
        from scratch. Add new insights, remove contradicted ones."
        .to_string();

    let user_msg = if existing_context.trim().is_empty() {
        format!("Here are my recent text transformations:\n\n{history_text}")
    } else {
        format!(
            "My current writing preferences:\n\n{existing_context}\n\n\
             Here are my recent text transformations:\n\n{history_text}"
        )
    };

    let body = Request {
        model: cfg.api.model.clone(),
        system,
        messages: vec![Message {
            role: "user",
            content: vec![Content {
                r#type: "text",
                text: user_msg,
            }],
        }],
        max_tokens: cfg.transform.max_tokens,
        stream: false,
    };

    let tls = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(cfg.api.accept_invalid_certs)
        .build()
        .map_err(|e| format!("TLS error: {e}"))?;
    let agent = ureq::AgentBuilder::new()
        .tls_connector(std::sync::Arc::new(tls))
        .build();

    let resp = agent
        .post(&cfg.api.url)
        .set("Content-Type", "application/json")
        .set("x-api-key", &cfg.api.key)
        .set("anthropic-version", &cfg.api.anthropic_version)
        .set("x-user-type", &cfg.api.user_type)
        .set("x-user-name", &cfg.api.user_name)
        .send_json(&body)
        .map_err(|e| format!("API error: {e}"))?;

    let parsed: Response = resp.into_json().map_err(|e| format!("Parse error: {e}"))?;

    parsed
        .content
        .into_iter()
        .find_map(|b| b.text)
        .ok_or_else(|| "No text in response".to_string())
}
