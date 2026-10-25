//! Cross-platform clipboard read/write.
//!
//! macOS uses pbpaste/pbcopy (the only tested platform). Windows and Linux
//! backends are EXPERIMENTAL and untested on real hardware.

use std::io::Write;
use std::process::{Command, Output, Stdio};

/// Turns captured command output into clipboard text (non-empty, non-blank).
fn text_from(out: std::io::Result<Output>) -> Option<String> {
    out.ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .filter(|t| !t.trim().is_empty())
}

/// Runs `cmd` with `text` on stdin. Returns Err when the program cannot be
/// spawned, otherwise Ok(true) when it wrote the text and exited successfully.
fn spawn_piped(mut cmd: Command, text: &str) -> std::io::Result<bool> {
    let mut child = cmd.stdin(Stdio::piped()).spawn()?;
    let wrote = match child.stdin.take() {
        Some(mut stdin) => stdin.write_all(text.as_bytes()).is_ok(),
        None => false,
    };
    let status = child.wait()?;
    Ok(wrote && status.success())
}

#[cfg(target_os = "macos")]
pub fn get_text() -> Option<String> {
    text_from(Command::new("pbpaste").output())
}

#[cfg(target_os = "macos")]
pub fn set_text(text: &str) -> bool {
    spawn_piped(Command::new("pbcopy"), text).unwrap_or(false)
}

/// EXPERIMENTAL: untested on real hardware. Reads the clipboard via PowerShell.
#[cfg(target_os = "windows")]
pub fn get_text() -> Option<String> {
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "[Console]::OutputEncoding=[Text.Encoding]::UTF8; Get-Clipboard -Raw",
    ]);
    text_from(cmd.output())
}

/// EXPERIMENTAL: untested on real hardware. Writes the clipboard via PowerShell,
/// passing the text on stdin as UTF-8.
#[cfg(target_os = "windows")]
pub fn set_text(text: &str) -> bool {
    let mut cmd = Command::new("powershell");
    cmd.args([
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "[Console]::InputEncoding=[Text.Encoding]::UTF8; $t=[Console]::In.ReadToEnd(); Set-Clipboard -Value $t",
    ]);
    spawn_piped(cmd, text).unwrap_or(false)
}

/// True when running under Wayland (WAYLAND_DISPLAY is set and non-empty).
#[cfg(target_os = "linux")]
fn on_wayland() -> bool {
    std::env::var("WAYLAND_DISPLAY")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
}

/// EXPERIMENTAL: untested on real hardware. Uses wl-paste on Wayland, otherwise
/// xclip (falling back to xsel when xclip cannot be spawned).
#[cfg(target_os = "linux")]
pub fn get_text() -> Option<String> {
    if on_wayland() {
        let mut cmd = Command::new("wl-paste");
        cmd.arg("--no-newline");
        return text_from(cmd.output());
    }
    let mut xclip = Command::new("xclip");
    xclip.args(["-selection", "clipboard", "-o"]);
    match xclip.output() {
        Ok(out) => text_from(Ok(out)),
        Err(_) => {
            let mut xsel = Command::new("xsel");
            xsel.args(["--clipboard", "--output"]);
            text_from(xsel.output())
        }
    }
}

/// EXPERIMENTAL: untested on real hardware. Uses wl-copy on Wayland, otherwise
/// xclip (falling back to xsel when xclip cannot be spawned).
#[cfg(target_os = "linux")]
pub fn set_text(text: &str) -> bool {
    if on_wayland() {
        return spawn_piped(Command::new("wl-copy"), text).unwrap_or(false);
    }
    let mut xclip = Command::new("xclip");
    xclip.args(["-selection", "clipboard", "-i"]);
    match spawn_piped(xclip, text) {
        Ok(ok) => ok,
        Err(_) => {
            let mut xsel = Command::new("xsel");
            xsel.args(["--clipboard", "--input"]);
            spawn_piped(xsel, text).unwrap_or(false)
        }
    }
}
