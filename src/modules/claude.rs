//! Claude Messages API client. Every request goes through `post_message`, which
//! applies the configured auth style, extra headers, timeout and redirect policy.
//!
//! Error strings are generated locally: they never contain the key, header values,
//! URLs, or raw response bodies.

use super::config::{AuthStyle, Config};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;

const CHECK_PROMPT: &str = "Reply with the single word OK";
const CHECK_SYSTEM: &str = "You are a connectivity check. Follow the user's instruction exactly.";
const CHECK_MAX_TOKENS: u32 = 16;

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

/// Call the configured Claude model and return the transformed text.
pub fn transform(
    cfg: &Config,
    instruction: &str,
    user_context: &str,
    text: &str,
) -> Result<String, String> {
    let user_msg = format!("Instruction: {instruction}\n\nText to transform:\n{text}");
    let body = user_request(
        &cfg.api.model,
        build_system_prompt(cfg, user_context),
        user_msg,
        cfg.transform.max_tokens,
    );
    post_message(cfg, &body)
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

    let body = user_request(&cfg.api.model, system, user_msg, cfg.transform.max_tokens);
    post_message(cfg, &body)
}

/// Send one minimal request to verify the configuration. Returns the model name on success.
pub fn check(cfg: &Config) -> Result<String, String> {
    let body = user_request(
        &cfg.api.model,
        CHECK_SYSTEM.to_string(),
        CHECK_PROMPT.to_string(),
        CHECK_MAX_TOKENS,
    );
    post_message(cfg, &body)?;
    Ok(cfg.api.model.clone())
}

/// Shared request function for transform, analyze_style and check.
fn post_message(cfg: &Config, body: &Request) -> Result<String, String> {
    let agent = build_agent(cfg)?;

    let mut req = agent.post(&cfg.api.url).set("Content-Type", "application/json");
    req = match cfg.api.auth {
        AuthStyle::XApiKey => req.set("x-api-key", &cfg.api.key),
        AuthStyle::Bearer => req.set("Authorization", &format!("Bearer {}", cfg.api.key)),
    };
    req = req.set("anthropic-version", &cfg.api.anthropic_version);
    for (name, value) in &cfg.api.headers {
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
    let text = response.into_string().map_err(|e| body_error(&e))?;
    parse_text(&text)
}

fn build_agent(cfg: &Config) -> Result<ureq::Agent, String> {
    let tls = native_tls::TlsConnector::builder()
        .danger_accept_invalid_certs(cfg.api.accept_invalid_certs)
        .build()
        .map_err(|_| "request failed: tls error".to_string())?;
    Ok(ureq::AgentBuilder::new()
        .tls_connector(Arc::new(tls))
        .redirects(0)
        .timeout(Duration::from_secs(cfg.api.timeout_secs))
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

fn parse_text(body: &str) -> Result<String, String> {
    let parsed: Response =
        serde_json::from_str(body).map_err(|_| "unexpected response format".to_string())?;
    parsed
        .content
        .into_iter()
        .find_map(|b| b.text)
        .ok_or_else(|| "response contained no text".to_string())
}
