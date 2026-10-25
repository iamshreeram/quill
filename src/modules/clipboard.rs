//! Cross-platform clipboard read/write.
//! Uses pbpaste/pbcopy on macOS for reliable persistence.

use std::process::{Command, Stdio};
use std::io::Write;

pub fn get_text() -> Option<String> {
    Command::new("pbpaste")
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .filter(|t| !t.trim().is_empty())
}

pub fn set_text(text: &str) -> bool {
    Command::new("pbcopy")
        .stdin(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            child.stdin.as_mut().unwrap().write_all(text.as_bytes())?;
            child.wait()
        })
        .map(|s| s.success())
        .unwrap_or(false)
}
