//! Configuration loader for Quill.
//!
//! Config file: ~/.config/quill/config.toml

use serde::Deserialize;
use std::fs;
use std::path::PathBuf;

const CONFIG_DIR: &str = "quill";
const CONFIG_FILE: &str = "config.toml";

#[derive(Deserialize, Clone)]
pub struct Config {
    pub api: ApiConfig,
    pub transform: TransformConfig,
    pub shortcut: ShortcutConfig,
}

#[derive(Deserialize, Clone)]
pub struct ApiConfig {
    pub url: String,
    pub key: String,
    pub model: String,
    pub anthropic_version: String,
    pub user_type: String,
    pub user_name: String,
    pub accept_invalid_certs: bool,
}

#[derive(Deserialize, Clone)]
pub struct TransformConfig {
    pub instruction: String,
    pub system_prompt: String,
    pub max_tokens: u32,
    /// Name of a prompt file in prompts/ dir. Empty = use `instruction` above.
    #[serde(default)]
    pub prompt_override: String,
}

#[derive(Deserialize, Clone)]
pub struct ShortcutConfig {
    /// skhd syntax for macOS, e.g. "cmd + shift - c"
    pub macos: String,
    /// Windows hotkey, e.g. "ctrl + shift + c"
    pub windows: String,
}

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

/// Load config from disk. Fails with a helpful message if missing.
pub fn load() -> Result<Config, String> {
    let path = config_path();
    if !path.exists() {
        return Err(format!(
            "Config not found at {}\nRun `quill init` to create it.",
            path.display()
        ));
    }
    let raw = fs::read_to_string(&path)
        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
    toml::from_str(&raw)
        .map_err(|e| format!("Invalid config at {}: {e}", path.display()))
}

/// Resolve the effective instruction.
/// Priority: cli_prompt > QUILL_PROMPT env > config.prompt_override > config.instruction
pub fn resolve_instruction(cfg: &Config, cli_prompt: Option<&str>) -> Result<String, String> {
    // 1. CLI flag
    if let Some(name) = cli_prompt {
        return load_prompt_file(name);
    }
    // 2. Env var
    if let Ok(name) = std::env::var("QUILL_PROMPT") {
        if !name.is_empty() {
            return load_prompt_file(&name);
        }
    }
    // 3. Config prompt_override
    if !cfg.transform.prompt_override.is_empty() {
        return load_prompt_file(&cfg.transform.prompt_override);
    }
    // 4. Default instruction from config
    Ok(cfg.transform.instruction.clone())
}

/// Load a prompt file from ~/.config/quill/prompts/<name>.txt
fn load_prompt_file(name: &str) -> Result<String, String> {
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

/// Default config content — embedded from config.sample.toml at compile time.
/// The single source of truth lives in the project root, not in Rust code.
pub fn default_toml() -> &'static str {
    include_str!("../../config.sample.toml")
}

/// Write default config + scaffold directories + sample prompts.
pub fn write_default() -> Result<PathBuf, String> {
    let dir = config_dir();
    fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create {}: {e}", dir.display()))?;
    let path = config_path();
    if path.exists() {
        return Err(format!(
            "Config already exists at {}\nDelete it first if you want to reinitialize.",
            path.display()
        ));
    }
    // Write config
    fs::write(&path, default_toml())
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))?;

    // Create prompts directory with samples
    let prompts = prompts_dir();
    fs::create_dir_all(&prompts)
        .map_err(|e| format!("Failed to create {}: {e}", prompts.display()))?;

    let samples = [
        ("formal", "Rewrite in formal business English. Return ONLY the rewritten text."),
        ("casual", "Rewrite casually, like texting a friendly coworker. Return ONLY the rewritten text."),
        ("grammar", "Fix grammar and spelling. Keep the original meaning and tone. Return ONLY the corrected text."),
        ("summarize", "Summarize in 1-2 concise sentences. Return ONLY the summary."),
    ];
    for (name, content) in samples {
        let p = prompts.join(format!("{name}.txt"));
        let _ = fs::write(&p, content);
    }

    Ok(path)
}
