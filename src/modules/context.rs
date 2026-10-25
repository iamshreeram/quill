//! Context management — persistent writing style context for Claude.
//!
//! context.md is injected into every transform call so Claude understands
//! the user's preferred writing style.

use std::fs;

use super::config;

const DEFAULT_CONTEXT: &str = "\
# Quill Writing Context

This file describes your writing preferences. Quill injects this into every
transform call so Claude matches your style. Edit freely or run `quill learn`
to auto-update based on your transformation history.

## Preferences

- Keep text concise and direct
- Use a professional but approachable tone
- Avoid unnecessary jargon
";

/// Load context.md contents. Returns empty string if file doesn't exist.
pub fn load() -> String {
    let path = config::context_path();
    fs::read_to_string(&path).unwrap_or_default()
}

/// Write context.md contents.
pub fn save(content: &str) -> Result<(), String> {
    let path = config::context_path();
    fs::write(&path, content)
        .map_err(|e| format!("Failed to write {}: {e}", path.display()))
}

/// Create default context.md if it doesn't exist.
pub fn ensure_exists() {
    let path = config::context_path();
    if !path.exists() {
        let _ = fs::write(&path, DEFAULT_CONTEXT);
    }
}
