//! History management — stores before/after transformation pairs.
//!
//! Used by `quill learn` to extract writing style preferences.

use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Write;

use super::config;

#[derive(Serialize, Deserialize)]
pub struct Entry {
    pub timestamp: String,
    pub before: String,
    pub after: String,
}

/// Append a before/after pair to history.jsonl.
pub fn append(before: &str, after: &str) {
    let path = config::history_path();
    let entry = Entry {
        timestamp: now_iso(),
        before: before.to_string(),
        after: after.to_string(),
    };
    if let Ok(json) = serde_json::to_string(&entry) {
        if let Ok(mut f) = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            let _ = writeln!(f, "{json}");
        }
    }
}

/// Load all history entries.
pub fn load_all() -> Vec<Entry> {
    let path = config::history_path();
    let raw = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(_) => return Vec::new(),
    };
    raw.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

/// Clear history file.
pub fn clear() {
    let path = config::history_path();
    let _ = fs::write(&path, "");
}

/// Count of entries in history.
pub fn count() -> usize {
    let path = config::history_path();
    match fs::read_to_string(&path) {
        Ok(s) => s.lines().filter(|l| !l.trim().is_empty()).count(),
        Err(_) => 0,
    }
}

fn now_iso() -> String {
    // Simple ISO-ish timestamp without chrono dependency
    let dur = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!("unix:{}", dur.as_secs())
}
