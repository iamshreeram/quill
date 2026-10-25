//! Claude Messages API client. Every request goes through `post_raw`, which
//! applies the configured auth style, extra headers, timeout and redirect policy.
//!
//! Error strings are generated locally: they never contain the key, header values,
//! URLs, or raw response bodies.

use super::config::{ApiConfig, AuthStyle, Config};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

const CHECK_PROMPT: &str = "Reply with the single word OK";
const CHECK_SYSTEM: &str = "You are a connectivity check. Follow the user's instruction exactly.";

/// Token budget for `quill check` against the configured Anthropic API (kept small on purpose).
pub const API_CHECK_MAX_TOKENS: u32 = 16;

/// Token budget for `quill check` against a local Ollama model. Thinking-capable local models
/// (e.g. qwen3) emit a `thinking` block before any `text` block, and a 16-token budget ends
/// inside the thinking block, so the reply has no text ("response contained no text").
pub const LOCAL_CHECK_MAX_TOKENS: u32 = 1024;

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

/// Build a single-user-message request body.
fn user_request(model: &str, system: String, user_msg: String, max_tokens: u32) -> Request {
    Request {
        model: model.to_string(),
        system,
        messages: vec![Message {
            role: "user",
            content: vec![Content {
                r#type: "text",
                text: user_msg,
            }],
        }],
        max_tokens,
        stream: false,
    }
}

/// Build the full system prompt by injecting user context.
fn build_system_prompt(cfg: &Config, user_context: &str) -> String {
    let base = &cfg.transform.system_prompt;
    if user_context.trim().is_empty() {
        return base.clone();
    }
    format!(
        "{base}\n\nThe user has provided the following writing style preferences. \
         Follow these closely when transforming their text:\n\n{user_context}"
    )
}

/// The configured API, or a local error when none is configured.
fn require_api(cfg: &Config) -> Result<&ApiConfig, String> {
    cfg.api
        .as_ref()
        .ok_or_else(|| "API not configured".to_string())
}

/// Call the configured model and return the transformed text.
pub fn transform(
    cfg: &Config,
    instruction: &str,
    user_context: &str,
    text: &str,
) -> Result<String, String> {
    let api = require_api(cfg)?;
    let user_msg = format!("Instruction: {instruction}\n\nText to transform:\n{text}");
    let body = user_request(
        &api.model,
        build_system_prompt(cfg, user_context),
        user_msg,
        cfg.transform.max_tokens,
    );
    parse_text(&post_raw(api, &body)?)
}

/// Ask the model to analyze transformation history and produce a writing style summary.
pub fn analyze_style(
    cfg: &Config,
    history_text: &str,
    existing_context: &str,
) -> Result<String, String> {
    let api = require_api(cfg)?;
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

    let body = user_request(&api.model, system, user_msg, cfg.transform.max_tokens);
    parse_text(&post_raw(api, &body)?)
}

/// Send one minimal request to verify the configuration. Returns the model name on success.
pub fn check(cfg: &Config, max_tokens: u32) -> Result<String, String> {
    let api = require_api(cfg)?;
    let body = user_request(
        &api.model,
        CHECK_SYSTEM.to_string(),
        CHECK_PROMPT.to_string(),
        max_tokens,
    );
    parse_envelope(&post_raw(api, &body)?)?;
    Ok(api.model.clone())
}

/// Shared request function: returns the raw successful response body.
fn post_raw(api: &ApiConfig, body: &Request) -> Result<String, String> {
    let agent = build_agent(api)?;

    let mut req = agent.post(&api.url).set("Content-Type", "application/json");
    req = match api.auth {
        AuthStyle::XApiKey => req.set("x-api-key", &api.key),
        AuthStyle::Bearer => req.set("Authorization", &format!("Bearer {}", api.key)),
    };
    req = req.set("anthropic-version", &api.anthropic_version);
    for (name, value) in &api.headers {
        req = req.set(name, value);
    }

    let response = match req.send_json(body) {
        Ok(resp) => resp,
        Err(ureq::Error::Status(code, _)) => return Err(status_error(code)),
        Err(ureq::Error::Transport(transport)) => return Err(transport_error(&transport)),
    };

    // With redirects disabled, ureq 2 returns 3xx responses as Ok: treat them as failures.
    let code = response.status();
    if code >= 300 {
        return Err(status_error(code));
    }
    response.into_string().map_err(|e| body_error(&e))
}

fn build_agent(api: &ApiConfig) -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(api.accept_invalid_certs)
        .build()
        .map_err(|_| "request failed: tls error".to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .redirects(0)
        .timeout(Duration::from_secs(api.timeout_secs))
        .build())
}

/// Fixed, locally generated message for an HTTP status.
fn status_error(code: u16) -> String {
    if (300..400).contains(&code) {
        return format!("redirects are not followed (HTTP {code})");
    }
    let phrase = match code {
        400 => "bad request",
        401 => "authentication failed",
        403 => "forbidden",
        404 => "endpoint not found",
        408 => "request timeout",
        413 => "payload too large",
        429 => "rate limited",
        500..=599 => "server error",
        _ => "unexpected status",
    };
    format!("API error: HTTP {code} ({phrase})")
}

/// Classifies a transport failure. The underlying message (which may contain the URL)
/// is only searched for keywords and is never printed.
fn transport_error(transport: &ureq::Transport) -> String {
    let kind = transport.kind();
    let detail = transport.to_string().to_ascii_lowercase();
    let mentions = |words: &[&str]| words.iter().any(|w| detail.contains(w));
    let class = if mentions(&["timed out", "timeout", "deadline"]) {
        "timeout"
    } else if matches!(
        kind,
        ureq::ErrorKind::Dns | ureq::ErrorKind::ConnectionFailed | ureq::ErrorKind::ProxyConnect
    ) {
        "connection error"
    } else if mentions(&["tls", "certificate", "ssl"]) {
        "tls error"
    } else {
        "connection error"
    };
    format!("request failed: {class}")
}

fn body_error(err: &std::io::Error) -> String {
    match err.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => {
            "request failed: timeout".to_string()
        }
        _ => "request failed: connection error".to_string(),
    }
}

/// Check-only validation: any well-formed Messages reply proves that the endpoint, key and model
/// answer. A thinking-capable model may spend the whole check budget on a `thinking` block and
/// emit no `text` block; that is still a working connection, so it is not an error here.
fn parse_envelope(body: &str) -> Result<(), String> {
    let parsed: serde_json::Value =
        serde_json::from_str(body).map_err(|_| "unexpected response format".to_string())?;
    match parsed.get("content").and_then(|c| c.as_array()) {
        Some(blocks) if !blocks.is_empty() => Ok(()),
        Some(_) => Err("response contained no content".to_string()),
        None => Err("unexpected response format".to_string()),
    }
}

fn parse_text(body: &str) -> Result<String, String> {
    let parsed: Response =
        serde_json::from_str(body).map_err(|_| "unexpected response format".to_string())?;
    parsed
        .content
        .into_iter()
        .find_map(|b| b.text)
        .ok_or_else(|| "response contained no text".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thinking_only_body_has_no_text() {
        // Shape captured from qwen3:30b via Ollama when the check budget (16 tokens) runs out
        // inside the thinking block: no text block at all, stop_reason "max_tokens".
        let body = r#"{"id":"msg_1","type":"message","role":"assistant","model":"qwen3:30b",
            "content":[{"type":"thinking","thinking":"Okay, the user wants me to reply with just"}],
            "stop_reason":"max_tokens"}"#;
        assert_eq!(parse_text(body).unwrap_err(), "response contained no text");
    }

    #[test]
    fn check_accepts_a_thinking_only_reply() {
        let body = r#"{"content":[{"type":"thinking","thinking":"Okay, the user"}],
            "stop_reason":"max_tokens"}"#;
        assert!(parse_envelope(body).is_ok());
        assert_eq!(
            parse_envelope(r#"{"content":[]}"#).unwrap_err(),
            "response contained no content"
        );
        assert!(parse_envelope("not json").is_err());
    }

    #[test]
    fn thinking_then_text_yields_the_text() {
        let body = r#"{"content":[{"type":"thinking","thinking":"hmm"},{"type":"text","text":"OK"}],
            "stop_reason":"end_turn"}"#;
        assert_eq!(parse_text(body).unwrap(), "OK");
    }

    #[test]
    fn check_budgets_are_pinned() {
        assert_eq!(LOCAL_CHECK_MAX_TOKENS, 1024);
        assert_eq!(API_CHECK_MAX_TOKENS, 16);
    }
}
