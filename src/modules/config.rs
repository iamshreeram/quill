//! Configuration loader for Quill.
//!
//! Config file: ~/.config/quill/config.toml (see config.sample.toml).
//! Environment overrides (win over the file; an empty value counts as unset):
//! QUILL_API_URL, QUILL_API_KEY, QUILL_MODEL, QUILL_MODE, QUILL_OLLAMA_URL, QUILL_OLLAMA_MODEL.
//!
//! Mode (`auto` | `ai` | `offline`) precedence: CLI > QUILL_MODE > `[general] mode` > `auto`.
//! In `ai` mode the API must be fully configured; in `auto` and `offline` mode a missing file or
//! missing/placeholder key is not an error (see DESIGN.md A2).

use serde::Deserialize;
use std::collections::BTreeMap;
use std::fs;
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};

use super::fsutil;
use super::grammar::Dialect;

const CONFIG_DIR: &str = "quill";
const CONFIG_FILE: &str = "config.toml";
const LOG_FILE: &str = "quill.log";

const DEFAULT_URL: &str = "https://api.anthropic.com/v1/messages";
const DEFAULT_MODEL: &str = "claude-sonnet-4-5";
pub const DEFAULT_ANTHROPIC_VERSION: &str = "2023-06-01";
pub const DEFAULT_TIMEOUT_SECS: i64 = 60;
const MIN_TIMEOUT_SECS: i64 = 1;
const MAX_TIMEOUT_SECS: i64 = 600;
const PLACEHOLDER_KEY: &str = "YOUR_API_KEY_HERE";
const DEFAULT_MAX_TOKENS: u32 = 4096;
const DEFAULT_INSTRUCTION: &str = "Rephrase, humanize, shorten, and articulate the following text politely. Return ONLY the rewritten text — no labels, no options, no explanations.";
const DEFAULT_SYSTEM_PROMPT: &str = "You are a text transformation assistant. Return ONLY the transformed text. Do NOT add quotes, explanations, labels, prefixes, or any extra formatting. Do NOT introduce yourself or ask questions. Just output the raw transformed text, nothing else.";
const DEFAULT_MACOS_SHORTCUT: &str = "cmd + shift - c";
const DEFAULT_WINDOWS_SHORTCUT: &str = "ctrl + shift + c";
pub const DEFAULT_OLLAMA_URL: &str = "http://127.0.0.1:11434";

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
    /// The configured API. `None` when no real key is present (auto/offline mode only).
    pub api: Option<ApiConfig>,
    pub transform: TransformConfig,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub shortcut: ShortcutConfig,
    pub mode: Mode,
    pub mode_source: ModeSource,
    pub ollama: OllamaConfig,
    pub offline: OfflineConfig,
}

/// Which backend family a command may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// Configured API, else a usable local Ollama model, else the offline grammar engine.
    Auto,
    /// Configured API only; failures are errors (no fallback, no Ollama probing).
    Ai,
    /// Offline grammar engine only; never touches the network.
    Offline,
}

impl Mode {
    pub fn parse(value: &str) -> Option<Mode> {
        match value.trim().to_ascii_lowercase().as_str() {
            "auto" => Some(Mode::Auto),
            "ai" => Some(Mode::Ai),
            "offline" => Some(Mode::Offline),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Auto => "auto",
            Mode::Ai => "ai",
            Mode::Offline => "offline",
        }
    }
}

/// Where the effective mode came from (shown by `quill status`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeSource {
    Default,
    Env,
    Config,
    Cli,
}

impl ModeSource {
    pub fn as_str(self) -> &'static str {
        match self {
            ModeSource::Default => "default",
            ModeSource::Env => "env",
            ModeSource::Config => "config",
            ModeSource::Cli => "cli",
        }
    }
}

#[derive(Clone)]
pub struct OllamaConfig {
    pub enabled: bool,
    /// Base URL without a trailing slash, e.g. http://127.0.0.1:11434
    pub url: String,
    /// Configured model name; empty means auto-pick.
    pub model: String,
}

#[derive(Clone)]
pub struct OfflineConfig {
    pub dialect: Dialect,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthStyle {
    /// `x-api-key: <key>` header (Anthropic default).
    XApiKey,
    /// `Authorization: Bearer <key>` header.
    Bearer,
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
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub macos: String,
    /// Windows hotkey, e.g. "ctrl + shift + c" (informational; only read on Windows)
    #[cfg_attr(not(windows), allow(dead_code))]
    pub windows: String,
}

// ── Raw file schema (everything optional) ─────────────────────

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawConfig {
    general: RawGeneral,
    api: RawApi,
    ollama: RawOllama,
    offline: RawOffline,
    transform: RawTransform,
    shortcut: RawShortcut,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawGeneral {
    mode: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawOllama {
    enabled: Option<bool>,
    url: Option<String>,
    model: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawOffline {
    dialect: Option<String>,
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

/// True when QUILL_API_KEY is set to a non-empty value (the value is never returned).
pub fn env_api_key_set() -> bool {
    env_nonempty("QUILL_API_KEY").is_some()
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

/// Load and validate the configuration. Repairs local file modes first; no network I/O.
pub fn load(cli_mode: Option<Mode>) -> Result<Config, String> {
    repair_local_files();
    inspect(cli_mode)
}

/// Load and validate the configuration without touching any file (used by `status`).
pub fn inspect(cli_mode: Option<Mode>) -> Result<Config, String> {
    let path = config_path();
    let (raw, found) = match fs::read_to_string(&path) {
        Ok(text) => (parse_file(&path, &text)?, true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => (RawConfig::default(), false),
        Err(e) => return Err(format!("Failed to read {}: {e}", path.display())),
    };
    let file_mode = match raw.general.mode.as_deref() {
        Some(value) => Some(Mode::parse(value).ok_or_else(|| {
            format!(
                "Invalid config at {}: general.mode must be auto, ai or offline",
                path.display()
            )
        })?),
        None => None,
    };
    let env_mode = env_nonempty("QUILL_MODE");
    let (mode, mode_source) = resolve_mode(cli_mode, env_mode.as_deref(), file_mode)?;
    build(raw, found, mode, mode_source, &path)
}

/// Mode precedence: CLI > QUILL_MODE > config file > `auto`. Invalid env values are errors.
pub fn resolve_mode(
    cli: Option<Mode>,
    env: Option<&str>,
    file: Option<Mode>,
) -> Result<(Mode, ModeSource), String> {
    let env_mode = match env {
        Some(value) => Some(Mode::parse(value).ok_or_else(|| {
            "invalid mode in QUILL_MODE: expected auto, ai or offline".to_string()
        })?),
        None => None,
    };
    Ok(if let Some(m) = cli {
        (m, ModeSource::Cli)
    } else if let Some(m) = env_mode {
        (m, ModeSource::Env)
    } else if let Some(m) = file {
        (m, ModeSource::Config)
    } else {
        (Mode::Auto, ModeSource::Default)
    })
}

/// Parse the file. Only a sanitized TOML error message and the line number are
/// reported, never the source snippet (which may contain the key).
fn parse_file(path: &Path, text: &str) -> Result<RawConfig, String> {
    toml::from_str::<RawConfig>(text).map_err(|e| {
        let message = safe_toml_message(e.message());
        match e.span() {
            Some(span) => {
                let upto = span.start.min(text.len());
                let line = text.as_bytes()[..upto]
                    .iter()
                    .filter(|b| **b == b'\n')
                    .count()
                    + 1;
                format!(
                    "Invalid config at {} (line {line}): {message}",
                    path.display()
                )
            }
            None => format!("Invalid config at {}: {message}", path.display()),
        }
    })
}

/// Serde type/value errors quote the offending value (`invalid type: string "..."`),
/// which could be a secret placed in the wrong field. Syntax errors never quote values.
fn safe_toml_message(message: &str) -> String {
    if message.contains('"') {
        "a value has the wrong type or is not valid for its field".to_string()
    } else {
        message.to_string()
    }
}

/// True when the key is absent or the placeholder (not a configuration error outside ai mode).
fn key_is_unset(key: &str) -> bool {
    let trimmed = key.trim();
    trimmed.is_empty() || trimmed == PLACEHOLDER_KEY
}

fn build(
    raw: RawConfig,
    found: bool,
    mode: Mode,
    mode_source: ModeSource,
    path: &Path,
) -> Result<Config, String> {
    let strict = mode == Mode::Ai;
    let key_env = env_nonempty("QUILL_API_KEY");
    if strict && !found && key_env.is_none() {
        return Err(format!(
            "Config not found at {}. Run `quill init`, or set QUILL_API_KEY (and QUILL_API_URL).",
            path.display()
        ));
    }
    let key_present = !key_is_unset(
        key_env
            .as_deref()
            .or(raw.api.key.as_deref())
            .unwrap_or_default(),
    );
    let api = if strict || key_present {
        Some(build_api(raw.api)?)
    } else {
        None
    };

    let ollama_url = env_nonempty("QUILL_OLLAMA_URL")
        .or(raw.ollama.url)
        .unwrap_or_else(|| DEFAULT_OLLAMA_URL.to_string());
    validate_url(&ollama_url, "ollama.url", "QUILL_OLLAMA_URL")?;
    let ollama = OllamaConfig {
        enabled: raw.ollama.enabled.unwrap_or(true),
        url: ollama_url.trim_end_matches('/').to_string(),
        model: env_nonempty("QUILL_OLLAMA_MODEL")
            .or(raw.ollama.model)
            .unwrap_or_default()
            .trim()
            .to_string(),
    };

    let dialect = match raw.offline.dialect.as_deref() {
        None => Dialect::American,
        Some(value) => value.parse::<Dialect>().map_err(|_| {
            format!(
                "Invalid config at {}: offline.dialect must be american, british, australian, canadian or indian",
                path.display()
            )
        })?,
    };

    let transform = TransformConfig {
        instruction: raw
            .transform
            .instruction
            .unwrap_or_else(|| DEFAULT_INSTRUCTION.to_string()),
        system_prompt: raw
            .transform
            .system_prompt
            .unwrap_or_else(|| DEFAULT_SYSTEM_PROMPT.to_string()),
        max_tokens: raw.transform.max_tokens.unwrap_or(DEFAULT_MAX_TOKENS),
        prompt_override: raw.transform.prompt_override.unwrap_or_default(),
    };
    let shortcut = ShortcutConfig {
        macos: raw
            .shortcut
            .macos
            .unwrap_or_else(|| DEFAULT_MACOS_SHORTCUT.to_string()),
        windows: raw
            .shortcut
            .windows
            .unwrap_or_else(|| DEFAULT_WINDOWS_SHORTCUT.to_string()),
    };
    Ok(Config {
        api,
        transform,
        shortcut,
        mode,
        mode_source,
        ollama,
        offline: OfflineConfig { dialect },
    })
}

fn build_api(raw: RawApi) -> Result<ApiConfig, String> {
    let url = env_nonempty("QUILL_API_URL")
        .or(raw.url)
        .unwrap_or_else(|| DEFAULT_URL.to_string());
    validate_url(&url, "api.url", "QUILL_API_URL")?;

    let key_raw = env_nonempty("QUILL_API_KEY")
        .or(raw.key)
        .unwrap_or_default();
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
    if key_is_unset(key) {
        return Err(
            "api.key is not set. Put your API key in config.toml under [api], or set QUILL_API_KEY."
                .to_string(),
        );
    }
    let trimmed = key.trim();
    if trimmed.chars().any(|c| c.is_control()) {
        return Err(
            "api.key contains control characters; check config.toml or QUILL_API_KEY.".to_string(),
        );
    }
    Ok(trimmed.to_string())
}

/// Validates an endpoint without echoing any part of it.
/// Allowed: https://, or http:// only for loopback hosts. `field` names the setting in messages.
fn validate_url(url: &str, field: &str, env_name: &str) -> Result<(), String> {
    if url.is_empty() || url.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(format!(
            "Invalid {field}: a full URL is required, e.g. https://host/v1/messages (or set {env_name})"
        ));
    }
    let (scheme, rest) = url.split_once("://").ok_or_else(|| {
        format!(
            "Invalid {field}: a full URL is required, e.g. https://host/v1/messages (or set {env_name})"
        )
    })?;
    let scheme = scheme.to_ascii_lowercase();
    if scheme != "https" && scheme != "http" {
        return Err(format!("Invalid {field}: the scheme must be https"));
    }

    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    if authority.contains('@') {
        return Err(format!(
            "{field} must not contain credentials (user:pass@); set the key in api.key or QUILL_API_KEY"
        ));
    }

    let (host, port, bracketed) = if let Some(inner) = authority.strip_prefix('[') {
        let close = inner
            .find(']')
            .ok_or_else(|| format!("{field} has an invalid IPv6 host"))?;
        let after = &inner[close + 1..];
        let port = if after.is_empty() {
            None
        } else {
            Some(
                after
                    .strip_prefix(':')
                    .ok_or_else(|| format!("{field} has an invalid IPv6 host"))?,
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
            return Err(format!("{field} has an invalid port"));
        }
    }
    if host.is_empty() {
        return Err(format!("{field} has no host"));
    }
    if bracketed && host.parse::<Ipv6Addr>().is_err() {
        return Err(format!("{field} has an invalid IPv6 host"));
    }

    let loopback = host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<Ipv4Addr>()
            .map(|a| a.is_loopback())
            .unwrap_or(false)
        || host
            .parse::<Ipv6Addr>()
            .map(|a| a.is_loopback())
            .unwrap_or(false);
    if scheme == "http" && !loopback {
        return Err(format!(
            "{field} must use https://; plain http is allowed only for localhost, 127.x.x.x or ::1"
        ));
    }
    Ok(())
}

fn parse_auth(value: Option<&str>) -> Result<AuthStyle, String> {
    match value {
        None | Some("x-api-key") => Ok(AuthStyle::XApiKey),
        Some("bearer") => Ok(AuthStyle::Bearer),
        Some(_) => {
            Err("Invalid auth setting: use \"x-api-key\" (default) or \"bearer\"".to_string())
        }
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
            return Err(
                "invalid header name in [api.headers]: use only token characters".to_string(),
            );
        }
        let lower = name.to_ascii_lowercase();
        if RESERVED_HEADERS.contains(&lower.as_str()) {
            return Err(
                "reserved header in [api.headers]: Quill sets this header itself".to_string(),
            );
        }
        if out.iter().any(|(k, _)| k.eq_ignore_ascii_case(&name)) {
            return Err("duplicate header in [api.headers]".to_string());
        }
        if value.chars().any(|c| c.is_control() && c != '\t') {
            return Err(
                "invalid header value in [api.headers]: must not contain CR, LF or NUL".to_string(),
            );
        }
        out.push((name, value));
    }
    Ok(out)
}

// ── Prompts ───────────────────────────────────────────────────

/// Profile names: `^[A-Za-z0-9_-]+$`.
fn is_valid_profile_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
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
/// of the `[api]` section only (other sections keep their own url values).
pub fn render_default_toml(url: Option<&str>, key: Option<&str>) -> String {
    let mut out = String::new();
    let mut section = String::new();
    for line in default_toml().lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') {
            section = trimmed.to_string();
        }
        let in_api = section == "[api]";
        match (url, key) {
            (Some(u), _) if in_api && line.starts_with("url = ") => {
                out.push_str(&format!("url = \"{}\"", toml_escape(u)));
            }
            (_, Some(k)) if in_api && line.starts_with("key = ") => {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_parse_is_case_insensitive_and_strict() {
        assert_eq!(Mode::parse("Offline"), Some(Mode::Offline));
        assert_eq!(Mode::parse(" ai "), Some(Mode::Ai));
        assert_eq!(Mode::parse("auto"), Some(Mode::Auto));
        assert_eq!(Mode::parse("bogus"), None);
        assert_eq!(Mode::parse(""), None);
    }

    #[test]
    fn mode_precedence_is_cli_env_config_default() {
        let (m, s) = resolve_mode(Some(Mode::Ai), Some("offline"), Some(Mode::Auto)).unwrap();
        assert_eq!((m, s), (Mode::Ai, ModeSource::Cli));
        let (m, s) = resolve_mode(None, Some("offline"), Some(Mode::Ai)).unwrap();
        assert_eq!((m, s), (Mode::Offline, ModeSource::Env));
        let (m, s) = resolve_mode(None, None, Some(Mode::Ai)).unwrap();
        assert_eq!((m, s), (Mode::Ai, ModeSource::Config));
        let (m, s) = resolve_mode(None, None, None).unwrap();
        assert_eq!((m, s), (Mode::Auto, ModeSource::Default));
    }

    #[test]
    fn invalid_env_mode_is_an_error_that_mentions_mode() {
        let err = resolve_mode(None, Some("bogus"), None).unwrap_err();
        assert!(err.contains("mode"), "{err}");
    }

    #[test]
    fn ollama_url_uses_the_same_rules_as_api_url() {
        assert!(validate_url("http://127.0.0.1:11434", "ollama.url", "QUILL_OLLAMA_URL").is_ok());
        let err = validate_url("http://example.com", "ollama.url", "QUILL_OLLAMA_URL").unwrap_err();
        assert!(err.contains("ollama.url") && err.contains("https"), "{err}");
        let err =
            validate_url("http://u:p@127.0.0.1:1", "ollama.url", "QUILL_OLLAMA_URL").unwrap_err();
        assert!(!err.contains("u:p"), "{err}");
    }

    #[test]
    fn init_replacement_only_touches_the_api_section() {
        let text = render_default_toml(Some("https://llm.example.test/v1/messages"), Some("k-1"));
        let v: toml::Value = toml::from_str(&text).unwrap();
        assert_eq!(
            v["api"]["url"].as_str(),
            Some("https://llm.example.test/v1/messages")
        );
        assert_eq!(v["api"]["key"].as_str(), Some("k-1"));
        assert_eq!(v["ollama"]["url"].as_str(), Some(DEFAULT_OLLAMA_URL));
        assert_eq!(v["general"]["mode"].as_str(), Some("auto"));
        assert_eq!(v["offline"]["dialect"].as_str(), Some("american"));
    }

    #[test]
    fn sample_config_parses_with_expected_defaults() {
        let raw: RawConfig = toml::from_str(default_toml()).unwrap();
        assert_eq!(raw.general.mode.as_deref(), Some("auto"));
        assert_eq!(raw.ollama.enabled, Some(true));
        assert_eq!(raw.offline.dialect.as_deref(), Some("american"));
    }
}
