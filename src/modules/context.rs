//! Context management — persistent writing style context for Claude.
//!
//! context.md is injected into every transform call so Claude understands
//! the user's preferred writing style.

use std::fs;

use super::{config, fsutil};

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

/// Write context.md contents (atomically, mode 0600).
/// Returns Err, and leaves the target untouched, when context.md is a symlink.
pub fn save(content: &str) -> Result<(), String> {
    let path = config::context_path();
    fsutil::write_atomic_mode(&path, content.as_bytes(), 0o600)
}

/// Create default context.md if no entry exists at that path (a symlink counts
/// as an entry, so nothing is ever written through it).
pub fn ensure_exists() {
    let path = config::context_path();
    if fs::symlink_metadata(&path).is_err() {
        let _ = fsutil::write_atomic_mode(&path, DEFAULT_CONTEXT.as_bytes(), 0o600);
    }
}
