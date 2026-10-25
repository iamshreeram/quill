//! Configuration loader for Quill.
//!
//! Config file: ~/.config/quill/config.toml (see config.sample.toml).
//! Environment overrides (win over the file; an empty value counts as unset):
//! QUILL_API_URL, QUILL_API_KEY, QUILL_MODEL.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use super::fsutil;

const CONFIG_DIR: &str = "quill";
const CONFIG_FILE: &str = "config.toml";
const LOG_FILE: &str = "quill.log";

const DEFAULT_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_MODEL: &str = "claude-sonnet-4-5";
const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
const DEFAULT_TIMEOUT_SECS: i64 = 60;
const MIN_TIMEOUT_SECS: i64 = 1;
const MAX_TIMEOUT_SECS: i64 = 600;
const PLACEHOLDER_KEY: &str = "YOUR_API_KEY_HERE";
const DEFAULT_MAX_TOKENS: u32 = 4096;
const DEFAULT_INSTRUCTION: &str = "Rephrase, humanize, shorten, and articulate the following text politely. Return ONLY the rewritten text — no labels, no options, no explanations.";
const DEFAULT_SYSTEM_PROMPT: &str = "You are a text transformation assistant. Return ONLY the transformed text. Do NOT add quotes, explanations, labels, prefixes, or any extra formatting. Do NOT introduce yourself or ask questions. Just output the raw transformed text, nothing else.";
const DEFAULT_MACOS_SHORTCUT: &str = "cmd + shift - c";
const DEFAULT_WINDOWS_SHORTCUT: &str = "ctrl + shift + c";

/// Headers that Quill sets itself; they can never be configured in [api.headers].
const RESERVED_HEADERS: &[&str] = &[
    "authorization",
    "proxy-authorization",
    "x-api-key",
    "anthropic-version",
    "content-type",
    "content-length",
    "host",
    "transfer-encoding",
    "connection",
    "upgrade",
    "te",
    "cookie",
    "set-cookie",
];

/// Validated runtime configuration.
#[derive(Clone)]
pub struct Config {
    pub api: ApiConfig,
    pub transform: TransformConfig,
    pub shortcut: ShortcutConfig,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthStyle {
    /// `x-api-key: <key>` header (Anthropic default).
    XApiKey,
    /// `Authorization: Bearer <key>` header.
    Bearer,
}

#[derive(Clone)]
pub struct ApiConfig {
    /// Full Messages API endpoint.
    pub url: String,
    /// API key, trimmed and validated.
    pub key: String,
    pub model: String,
    pub anthropic_version: String,
    pub auth: AuthStyle,
    /// Whole-request timeout, validated to 1..=600.
    pub timeout_secs: u64,
    /// Unsafe: disables TLS certificate checks. Local debugging only.
    pub accept_invalid_certs: bool,
    /// Extra request headers, validated.
    pub headers: Vec<(String, String)>,
}

#[derive(Clone)]
pub struct TransformConfig {
    pub instruction: String,
    pub system_prompt: String,
    pub max_tokens: u32,
    /// Name of a prompt file in prompts/ dir. Empty = use `instruction` above.
    pub prompt_override: String,
}

#[derive(Clone)]
pub struct ShortcutConfig {
    /// skhd syntax for macOS, e.g. "cmd + shift - c"
    pub macos: String,
    /// Windows hotkey, e.g. "ctrl + shift + c"
    pub windows: String,
}

// ── Raw file schema (everything optional) ─────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawConfig {
    api: RawApi,
    transform: RawTransform,
    shortcut: RawShortcut,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawApi {
    url: Option<String>,
    key: Option<String>,
    model: Option<String>,
    anthropic_version: Option<String>,
    auth: Option<String>,
    timeout_secs: Option<i64>,
    accept_invalid_certs: Option<bool>,
    headers: BTreeMap<String, String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawTransform {
    instruction: Option<String>,
    system_prompt: Option<String>,
    max_tokens: Option<u32>,
    prompt_override: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawShortcut {
    macos: Option<String>,
    windows: Option<String>,
}

// ── Paths ─────────────────────────────────────────────────────

/// Returns the config directory path: ~/.config/quill/
pub fn config_dir() -> PathBuf {
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join(CONFIG_DIR)
}

/// Returns the config file path: ~/.config/quill/config.toml
pub fn config_path() -> PathBuf {
    config_dir().join(CONFIG_FILE)
}

/// Returns the prompts directory: ~/.config/quill/prompts/
pub fn prompts_dir() -> PathBuf {
    config_dir().join("prompts")
}

/// Returns the context file path: ~/.config/quill/context.md
pub fn context_path() -> PathBuf {
    config_dir().join("context.md")
}

/// Returns the history file path: ~/.config/quill/history.jsonl
pub fn history_path() -> PathBuf {
    config_dir().join("history.jsonl")
}

/// Returns the log file path: ~/.config/quill/quill.log
#[allow(dead_code)] // consumed by the hotkey/logging work in a later step
pub fn log_path() -> PathBuf {
    config_dir().join(LOG_FILE)
}

// ── Loading and validation ────────────────────────────────────

/// Reads an environment variable; an empty value counts as unset.
fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

/// Repairs permissions of Quill's local files when the config dir exists.
/// Symlinks are never chmod'ed or followed for writes; reads may follow them.
fn repair_local_files() {
    let dir = config_dir();
    if !dir.exists() {
        return;
    }
    let _ = fsutil::ensure_private_dir(&dir);
    for name in [CONFIG_FILE, "context.md", "history.jsonl", LOG_FILE] {
        fsutil::repair_file_mode(&dir.join(name));
    }
}

/// Load and validate the configuration. No network I/O happens here.
pub fn load() -> Result<Config, String> {
    repair_local_files();
    let path = config_path();
    let raw = match fs::read_to_string(&path) {
        Ok(text) => parse_file(&path, &text)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if env_nonempty("QUILL_API_KEY").is_none() {
                return Err(format!(
                    "Config not found at {}. Run `quill init`, or set QUILL_API_KEY (and QUILL_API_URL).",
                    path.display()
                ));
            }
            RawConfig::default()
        }
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    build(raw)
}

/// Parse the file. Only the TOML error message and line number are reported,
/// never the source snippet (which may contain the key).
fn parse_file(path: &Path, text: &str) -> Result<RawConfig, String> {
    toml::from_str::<RawConfig>(text).map_err(|e| match e.span() {
        Some(span) => {
            let upto = span.start.min(text.len());
            let line = text.as_bytes()[..upto].iter().filter(|b| **b == b'\n').count() + 1;
            format!("Invalid config at {} (line {line}): {}", path.display(), e.message())
        }
        None => format!("Invalid config at {}: {}", path.display(), e.message()),
    })
}

fn build(raw: RawConfig) -> Result<Config, String> {
    let api = build_api(raw.api)?;
    let transform = TransformConfig {
        instruction: raw.transform.instruction.unwrap_or_else(|| DEFAULT_INSTRUCTION.to_string()),
        system_prompt: raw.transform.system_prompt.unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string()),
        max_tokens: raw.transform.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        prompt_override: raw.transform.prompt_override.unwrap_or_default(),
    };
    let shortcut = ShortcutConfig {
        macos: raw.shortcut.macos.unwrap_or_else(|| DEFAULT_MACOS_SHORTCUT.to_string()),
        windows: raw.shortcut.windows.unwrap_or_else(|| DEFAULT_WINDOWS_SHORTCUT.to_string()),
    };
    Ok(Config { api, transform, shortcut })
}

fn build_api(raw: RawApi) -> Result<ApiConfig, String> {
    let url = env_nonempty("QUILL_API_URL")
        .or(raw.url)
        .unwrap_or_else(|| DEFAULT_URL.to_string());
    validate_url(&url)?;

    let key_raw = env_nonempty("QUILL_API_KEY").or(raw.key).unwrap_or_default();
    let key = validate_key(&key_raw)?;

    let model = env_nonempty("QUILL_MODEL")
        .or(raw.model)
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    let anthropic_version = raw
        .anthropic_version
        .unwrap_or_else(|| DEFAULT_ANTHROPIC_VERSION.to_string());
    let auth = parse_auth(raw.auth.as_deref())?;
    let timeout_secs = parse_timeout(raw.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS))?;
    let headers = validate_headers(raw.headers)?;

    Ok(ApiConfig {
        url,
        key,
        model,
        anthropic_version,
        auth,
        timeout_secs,
        accept_invalid_certs: raw.accept_invalid_certs.unwrap_or(false),
        headers,
    })
}

/// Returns the trimmed key, or an error. The key is never echoed.
fn validate_key(key: &str) -> Result<String, String> {
    let trimmed = key.trim();
    if trimmed.is_empty() || trimmed == PLACEHOLDER_KEY {
        return Err(
            "api.key is not set. Put your API key in config.toml under [api], or set QUILL_API_KEY."
                .to_string(),
        );
    }
    if trimmed.chars().any(|c| c.is_control()) {
        return Err("api.key contains control characters; check config.toml or QUILL_API_KEY.".to_string());
    }
    Ok(trimmed.to_string())
}

/// Validates the endpoint without echoing any part of it.
/// Allowed: https://, or http:// only for loopback hosts.
fn validate_url(url: &str) -> Result<(), String> {
    const HINT: &str = "api.url must be a full URL such as https://host/v1/messages (or set QUILL_API_URL)";
    if url.is_empty() || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(format!("Invalid api.url: {HINT}"));
    }
    let (scheme, rest) = url
        .split_once("://")
        .ok_or_else(|| format!("Invalid api.url: {HINT}"))?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return Err("Invalid api.url: the scheme must be https".to_string());
    }

    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains('@') {
        return Err(
            "api.url must not contain credentials (user:pass@); set the key in api.key or QUILL_API_KEY"
                .to_string(),
        );
    }

    let (host, port, bracketed) = if let Some(inner) = authority.strip_prefix('[') {
        let close = inner
            .find(']')
            .ok_or_else(|| "api.url has an invalid IPv6 host".to_string())?;
        let after = &inner[close + 1..];
        let port = if after.is_empty() {
            None
        } else {
            Some(
                after
                    .strip_prefix(':')
                    .ok_or_else(|| "api.url has an invalid IPv6 host".to_string())?,
            )
        };
        (&inner[..close], port, true)
    } else {
        match authority.split_once(':') {
            Some((h, p)) => (h, Some(p), false),
            None => (authority, None, false),
        }
    };

    if let Some(p) = port {
        if p.parse::<u16>().is_err() {
            return Err("api.url has an invalid port".to_string());
        }
    }
    if host.is_empty() {
        return Err("api.url has no host".to_string());
    }
    if bracketed && host.parse::<Ipv6Addr>().is_err() {
        return Err("api.url has an invalid IPv6 host".to_string());
    }

    let loopback = host.eq_ignore_ascii_case("localhost")
        || host.parse::<Ipv4Addr>().map(|a| a.is_loopback()).unwrap_or(false)
        || host.parse::<Ipv6Addr>().map(|a| a.is_loopback()).unwrap_or(false);
    if scheme == "http" && !loopback {
        return Err(
            "api.url must use https://; plain http is allowed only for localhost, 127.x.x.x or ::1"
                .to_string(),
        );
    }
    Ok(())
}

fn parse_auth(value: Option<&str>) -> Result<AuthStyle, String> {
    match value {
        None | Some("x-api-key") => Ok(AuthStyle::XApiKey),
        Some("bearer") => Ok(AuthStyle::Bearer),
        Some(_) => Err("Invalid auth setting: use \"x-api-key\" (default) or \"bearer\"".to_string()),
    }
}

fn parse_timeout(secs: i64) -> Result<u64, String> {
    if !(MIN_TIMEOUT_SECS..=MAX_TIMEOUT_SECS).contains(&secs) {
        return Err(format!(
            "timeout_secs must be between {MIN_TIMEOUT_SECS} and {MAX_TIMEOUT_SECS}"
        ));
    }
    Ok(secs as u64)
}

/// RFC 7230 token characters.
fn is_token(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"!#$%&'*+-.^_`|~".contains(&b))
}

fn validate_headers(map: BTreeMap<String, String>) -> Result<Vec<(String, String)>, String> {
    let mut out: Vec<(String, String)> = Vec::new();
    for (name, value) in map {
        if !is_token(&name) {
            return Err("invalid header name in [api.headers]: use only token characters".to_string());
        }
        let lower = name.to_ascii_lowercase();
        if RESERVED_HEADERS.contains(&lower.as_str()) {
            return Err("reserved header in [api.headers]: Quill sets this header itself".to_string());
        }
        if out.iter().any(|(k, _)| k.eq_ignore_ascii_case(&name)) {
            return Err("duplicate header in [api.headers]".to_string());
        }
        if value.chars().any(|c| c.is_control() && c != '\t') {
            return Err("invalid header value in [api.headers]: must not contain CR, LF or NUL".to_string());
        }
        out.push((name, value));
    }
    Ok(out)
}

// ── Prompts ───────────────────────────────────────────────────

/// Profile names: `^[A-Za-z0-9_-]+$`.
fn is_valid_profile_name(name: &str) -> bool {
    !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Resolve the effective instruction.
/// Priority: cli_prompt > QUILL_PROMPT env > config.prompt_override > config.instruction
pub fn resolve_instruction(cfg: &Config, cli_prompt: Option<&str>) -> Result<String, String> {
    // 1. CLI flag
    if let Some(name) = cli_prompt {
        return load_prompt_file(name);
    }
    // 2. Env var
    if let Some(name) = env_nonempty("QUILL_PROMPT") {
        return load_prompt_file(&name);
    }
    // 3. Config prompt_override
    let overridden = cfg.transform.prompt_override.trim();
    if !overridden.is_empty() {
        return load_prompt_file(overridden);
    }
    // 4. Default instruction from config
    Ok(cfg.transform.instruction.clone())
}

/// Load a prompt file from ~/.config/quill/prompts/<name>.txt
fn load_prompt_file(name: &str) -> Result<String, String> {
    if !is_valid_profile_name(name) {
        return Err(
            "Invalid prompt profile name: use only letters, digits, '-' and '_'".to_string(),
        );
    }
    let path = prompts_dir().join(format!("{name}.txt"));
    if !path.exists() {
        return Err(format!(
            "Prompt file not found: {}\nCreate it with your instruction text.",
            path.display()
        ));
    }
    fs::read_to_string(&path)
        .map(|s| s.trim().to_string())
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))
}

// ── Defaults and init ─────────────────────────────────────────

/// Default config content — embedded from config.sample.toml at compile time.
/// The single source of truth lives in the project root, not in Rust code.
pub fn default_toml() -> &'static str {
    include_str!("../../config.sample.toml")
}

/// Escapes a value for a TOML basic string.
fn toml_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '"' => out.push_str("\\\""),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => out.push_str(&format!("\\u{:04X}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

/// Renders the sample config, replacing the `url = ...` and `key = ...` lines
/// of [api] with the given (escaped) values when provided.
pub fn render_default_toml(url: Option<&str>, key: Option<&str>) -> String {
    let mut out = String::new();
    for line in default_toml().lines() {
        match (url, key) {
            (Some(u), _) if line.starts_with("url = ") => {
                out.push_str(&format!("url = \"{}\"", toml_escape(u)));
            }
            (_, Some(k)) if line.starts_with("key = ") => {
                out.push_str(&format!("key = \"{}\"", toml_escape(k)));
            }
            _ => out.push_str(line),
        }
        out.push('\n');
    }
    out
}

/// Write default config + scaffold directories + sample prompts.
/// QUILL_API_URL / QUILL_API_KEY from the environment pre-fill the config.
pub fn write_default() -> Result<PathBuf, String> {
    fsutil::ensure_private_dir(&config_dir())?;
    let path = config_path();
    // Any entry counts, including a symlink (even dangling): never write through it.
    if fs::symlink_metadata(&path).is_ok() {
        return Err(format!(
            "Config already exists at {}\nDelete it first if you want to reinitialize.",
            path.display()
        ));
    }
    let url = env_nonempty("QUILL_API_URL");
    let key = env_nonempty("QUILL_API_KEY");
    // Write config (private: it may hold the API key)
    fsutil::write_atomic_mode(
        &path,
        render_default_toml(url.as_deref(), key.as_deref()).as_bytes(),
        0o600,
    )?;

    // Create prompts directory with samples
    let prompts = prompts_dir();
    fsutil::ensure_private_dir(&prompts)?;

    let samples = [
        ("formal", "Rewrite in formal business English. Return ONLY the rewritten text."),
        ("casual", "Rewrite casually, like texting a friendly coworker. Return ONLY the rewritten text."),
        ("grammar", "Fix grammar and spelling. Keep the original meaning and tone. Return ONLY the corrected text."),
        ("summarize", "Summarize in 1-2 concise sentences. Return ONLY the summary."),
    ];
    for (name, content) in samples {
        let p = prompts.join(format!("{name}.txt"));
        let _ = fsutil::write_atomic_mode(&p, content.as_bytes(), 0o644);
    }

    Ok(path)
}

