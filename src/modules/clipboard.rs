//! Cross-platform clipboard read/write.
//!
//! macOS uses pbpaste/pbcopy (the only tested platform). Windows and Linux
//! backends are EXPERIMENTAL and untested on real hardware.

use std::io::Write;
use std::process::{Command, Output, Stdio};
use std::time::Duration;

/// Attempts for a clipboard read that may briefly see an empty clipboard.
const READ_ATTEMPTS: u32 = 4;
/// Attempts for a clipboard write that may transiently fail.
const WRITE_ATTEMPTS: u32 = 3;
/// Pause between clipboard retries.
const RETRY_DELAY: Duration = Duration::from_millis(50);

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

/// Calls `f` up to `attempts` times, returning the first `Some`. Sleeps
/// `delay` only between tries, never after the last try or a success.
fn retry_nonempty<F: FnMut() -> Option<String>>(
    mut f: F,
    attempts: u32,
    delay: Duration,
) -> Option<String> {
    for attempt in 0..attempts {
        if let Some(value) = f() {
            return Some(value);
        }
        if attempt + 1 < attempts {
            std::thread::sleep(delay);
        }
    }
    None
}

/// Calls `f` up to `attempts` times, returning true as soon as it returns
/// true. Sleeps `delay` only between tries, never after the last try or a success.
fn retry_true<F: FnMut() -> bool>(mut f: F, attempts: u32, delay: Duration) -> bool {
    for attempt in 0..attempts {
        if f() {
            return true;
        }
        if attempt + 1 < attempts {
            std::thread::sleep(delay);
        }
    }
    false
}

/// Reads the clipboard, retrying briefly when it reads as empty.
pub fn get_text_retrying() -> Option<String> {
    retry_nonempty(get_text, READ_ATTEMPTS, RETRY_DELAY)
}

/// Writes the clipboard, retrying when the write fails.
pub fn set_text_retrying(text: &str) -> bool {
    retry_true(|| set_text(text), WRITE_ATTEMPTS, RETRY_DELAY)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::time::{Duration, Instant};

    #[test]
    fn retry_nonempty_returns_the_first_some_after_empty_reads() {
        let calls = Cell::new(0);
        let r = retry_nonempty(
            || {
                calls.set(calls.get() + 1);
                if calls.get() < 3 {
                    None
                } else {
                    Some("x".to_string())
                }
            },
            4,
            Duration::ZERO,
        );
        assert_eq!(r.as_deref(), Some("x"));
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn retry_nonempty_gives_up_after_exactly_the_attempts() {
        let calls = Cell::new(0);
        let r = retry_nonempty(
            || {
                calls.set(calls.get() + 1);
                None
            },
            4,
            Duration::ZERO,
        );
        assert_eq!(r, None);
        assert_eq!(calls.get(), 4);
    }

    #[test]
    fn retry_nonempty_calls_once_on_immediate_success_and_never_on_zero_attempts() {
        let calls = Cell::new(0);
        let r = retry_nonempty(
            || {
                calls.set(calls.get() + 1);
                Some("ok".to_string())
            },
            4,
            Duration::ZERO,
        );
        assert_eq!(r.as_deref(), Some("ok"));
        assert_eq!(calls.get(), 1);
        let calls = Cell::new(0);
        let r = retry_nonempty(
            || {
                calls.set(calls.get() + 1);
                Some("ok".to_string())
            },
            0,
            Duration::ZERO,
        );
        assert_eq!(r, None);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn retry_sleeps_between_tries_only() {
        let t = Instant::now();
        let _ = retry_nonempty(|| None, 3, Duration::from_millis(40));
        let e = t.elapsed();
        assert!(
            e >= Duration::from_millis(80),
            "slept {e:?}, expected 2 gaps of 40ms"
        );
        assert!(e < Duration::from_millis(2000), "slept too long: {e:?}");
        let t = Instant::now();
        let _ = retry_nonempty(|| Some("x".to_string()), 3, Duration::from_millis(300));
        assert!(
            t.elapsed() < Duration::from_millis(250),
            "must not sleep after a success"
        );
        let t = Instant::now();
        let _ = retry_true(|| true, 3, Duration::from_millis(300));
        assert!(
            t.elapsed() < Duration::from_millis(250),
            "must not sleep after a success"
        );
    }

    #[test]
    fn retry_true_counts_calls_for_fail_fail_success_permanent_failure_and_immediate() {
        let calls = Cell::new(0);
        let ok = retry_true(
            || {
                calls.set(calls.get() + 1);
                calls.get() == 3
            },
            3,
            Duration::ZERO,
        );
        assert!(ok);
        assert_eq!(calls.get(), 3);

        let calls = Cell::new(0);
        let ok = retry_true(
            || {
                calls.set(calls.get() + 1);
                false
            },
            3,
            Duration::ZERO,
        );
        assert!(!ok);
        assert_eq!(calls.get(), 3);

        let calls = Cell::new(0);
        let ok = retry_true(
            || {
                calls.set(calls.get() + 1);
                true
            },
            3,
            Duration::ZERO,
        );
        assert!(ok);
        assert_eq!(calls.get(), 1);

        let calls = Cell::new(0);
        let ok = retry_true(
            || {
                calls.set(calls.get() + 1);
                true
            },
            0,
            Duration::ZERO,
        );
        assert!(!ok);
        assert_eq!(calls.get(), 0);
    }
}
