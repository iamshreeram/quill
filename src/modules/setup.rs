//! `quill setup`: installs the keyboard shortcut.
//!
//! The pure helpers compile on every platform so they can be unit-tested
//! anywhere. `setup_macos` is the macOS entry point (hotkey.sh + managed
//! block in ~/.skhdrc).

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "macos")]
use std::path::{Path, PathBuf};

#[cfg(target_os = "macos")]
use super::{config, fsutil};

const BEGIN_MARKER: &str = "# >>> quill managed block v1 >>>";
const END_MARKER: &str = "# <<< quill managed block v1 <<<";
const UNBALANCED: &str = "unbalanced quill markers in .skhdrc; fix or remove them manually";

/// Quotes `s` for POSIX sh: wraps it in single quotes and turns each embedded
/// `'` into `'\''`.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// AppleScript for the notification helper; `on run argv` shows argv[1] as the banner.
pub(crate) const NOTIFY_APPLESCRIPT: &[&str] = &[
    "on run argv",
    "display notification (item 1 of argv) with title \"Quill\"",
    "end run",
];

/// AppleScript that copies the selection via Edit > Copy of the frontmost app.
/// Prints "ok", or "ERR <number>: <message>" when the menu click fails.
pub(crate) const COPY_APPLESCRIPT: &[&str] = &[
    "try",
    "tell application \"System Events\"",
    "click menu item \"Copy\" of menu \"Edit\" of menu bar 1 of (first application process whose frontmost is true)",
    "end tell",
    "return \"ok\"",
    "on error errMsg number errNum",
    "return \"ERR \" & errNum & \": \" & errMsg",
    "end try",
];

pub(crate) const MSG_ACCESSIBILITY: &str = "Quill: skhd needs Accessibility permission. Open System Settings > Privacy & Security > Accessibility and enable skhd, then try again.";
pub(crate) const MSG_AUTOMATION: &str = "Quill: skhd is not allowed to control System Events. Open System Settings > Privacy & Security > Automation, enable System Events under skhd, then try again.";
pub(crate) const MSG_COPY_FAILED: &str = "Quill: could not copy the selection (System Events error -1719). Check Accessibility and Automation permission for skhd in System Settings > Privacy & Security, then try again.";

/// Turns each AppleScript line into a shell-quoted `-e` argument.
fn applescript_args(lines: &[&str]) -> Vec<String> {
    lines
        .iter()
        .map(|line| format!("-e {}", shell_quote(line)))
        .collect()
}

/// The hotkey.sh template; `@NAME@` placeholders are filled by `render_hotkey_script`.
const HOTKEY_TEMPLATE: &str = include_str!("hotkey.sh.in");

/// Replaces each `@NAME@` placeholder in `template` with the matching value from
/// `vars`, in a single pass: inserted text is never rescanned. An `@` that does not
/// open a known placeholder is copied through unchanged.
fn fill_template(template: &str, vars: &[(&str, &str)]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(start) = rest.find('@') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let value = after.find('@').and_then(|end| {
            vars.iter()
                .find(|(name, _)| *name == &after[..end])
                .map(|(_, v)| (end, *v))
        });
        match value {
            Some((end, value)) => {
                out.push_str(value);
                rest = &after[end + 1..];
            }
            None => {
                out.push('@');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// Renders hotkey.sh from the template. The script runs with umask 077, copies the
/// selection via the Edit > Copy menu, runs the binary with the log inside the config
/// dir, and shows a notification when the transform fails or a permission is missing.
pub fn render_hotkey_script(binary: &str, log_path: &str) -> String {
    let notify_args = applescript_args(NOTIFY_APPLESCRIPT).join(" ");
    let copy_args = applescript_args(COPY_APPLESCRIPT).join(" \\\n  ");
    fill_template(
        HOTKEY_TEMPLATE,
        &[
            ("LOG", &shell_quote(log_path)),
            ("BIN", &shell_quote(binary)),
            ("NOTIFY_ARGS", &notify_args),
            ("COPY_ARGS", &copy_args),
            ("MSG_AUTOMATION", &shell_quote(MSG_AUTOMATION)),
            ("MSG_ACCESSIBILITY", &shell_quote(MSG_ACCESSIBILITY)),
            ("MSG_COPY_FAILED", &shell_quote(MSG_COPY_FAILED)),
        ],
    )
}

/// Inserts or replaces the managed block in `existing`.
///
/// - No markers: the block is appended (a newline is added first if needed).
/// - Exactly one begin marker before exactly one end marker: everything from the
///   begin line through the end line is replaced.
/// - Any other combination is an error and `existing` must stay unchanged.
///
/// Bytes outside the block are preserved exactly.
pub fn apply_managed_block(existing: &str, body_line: &str) -> Result<String, String> {
    let block = format!("{BEGIN_MARKER}\n{body_line}\n{END_MARKER}\n");
    let lines: Vec<&str> = existing.split_inclusive('\n').collect();
    let begins = marker_positions(&lines, BEGIN_MARKER);
    let ends = marker_positions(&lines, END_MARKER);
    match (begins.as_slice(), ends.as_slice()) {
        ([], []) => {
            let mut out = String::with_capacity(existing.len() + block.len() + 1);
            out.push_str(existing);
            if !existing.is_empty() && !existing.ends_with('\n') {
                out.push('\n');
            }
            out.push_str(&block);
            Ok(out)
        }
        ([begin], [end]) if *begin < *end => {
            let mut out = String::with_capacity(existing.len() + block.len());
            for line in &lines[..*begin] {
                out.push_str(line);
            }
            out.push_str(&block);
            for line in &lines[*end + 1..] {
                out.push_str(line);
            }
            Ok(out)
        }
        _ => Err(UNBALANCED.to_string()),
    }
}

/// Indexes of the lines equal to `marker` (trailing whitespace ignored).
fn marker_positions(lines: &[&str], marker: &str) -> Vec<usize> {
    lines
        .iter()
        .enumerate()
        .filter(|(_, line)| line.trim_end() == marker)
        .map(|(i, _)| i)
        .collect()
}

/// Counts lines outside the managed block that run `/quill/hotkey.sh`.
/// Only call this on text that already passed `apply_managed_block`.
#[cfg(target_os = "macos")]
fn unmanaged_legacy_lines(existing: &str) -> usize {
    let mut inside = false;
    let mut count = 0;
    for line in existing.lines() {
        let trimmed = line.trim_end();
        if trimmed == BEGIN_MARKER {
            inside = true;
        } else if trimmed == END_MARKER {
            inside = false;
        } else if !inside && trimmed.ends_with("/quill/hotkey.sh") {
            count += 1;
        }
    }
    count
}

// ── macOS ─────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Returns the file that setup should edit.
///
/// A missing ~/.skhdrc is created. A regular file is used as-is. A symlink is
/// resolved and its target used, but only when the target is a regular file
/// owned by the same user who owns HOME. Anything else is an error.
#[cfg(target_os = "macos")]
fn skhdrc_target(home: &Path) -> Result<PathBuf, String> {
    use std::os::unix::fs::MetadataExt;
    let rc = home.join(".skhdrc");
    let meta = match fs::symlink_metadata(&rc) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(rc),
        Err(_) => return Err("could not inspect ~/.skhdrc; leaving it unchanged".to_string()),
    };
    if !meta.file_type().is_symlink() {
        return if meta.is_file() {
            Ok(rc)
        } else {
            Err("~/.skhdrc is not a regular file; leaving it unchanged".to_string())
        };
    }
    let target = fs::canonicalize(&rc)
        .map_err(|_| "~/.skhdrc is a dangling symlink; leaving it unchanged".to_string())?;
    let target_meta = fs::metadata(&target)
        .map_err(|_| "~/.skhdrc symlink target is unreadable; leaving it unchanged".to_string())?;
    let home_owner = fs::metadata(home)
        .map(|m| m.uid())
        .map_err(|_| "cannot inspect HOME; leaving ~/.skhdrc unchanged".to_string())?;
    if !target_meta.is_file() || target_meta.uid() != home_owner {
        return Err(
            "~/.skhdrc symlink must point to a regular file owned by you; leaving it unchanged"
                .to_string(),
        );
    }
    Ok(target)
}

/// Reads the current skhdrc text and its permission bits. A missing file reads
/// as empty with mode 0644.
#[cfg(target_os = "macos")]
fn read_skhdrc(target: &Path) -> Result<(String, u32), String> {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(target) {
        Ok(meta) => {
            let text = fs::read_to_string(target).map_err(|_| {
                "could not read ~/.skhdrc as UTF-8 text; leaving it unchanged".to_string()
            })?;
            Ok((text, meta.permissions().mode() & 0o777))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok((String::new(), 0o644)),
        Err(_) => Err("could not read ~/.skhdrc; leaving it unchanged".to_string()),
    }
}

/// Returns the first executable `program` found on PATH.
#[cfg(target_os = "macos")]
fn find_on_path(program: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(program))
        .find(|candidate| {
            fs::metadata(candidate)
                .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
        })
}

/// Installs the macOS shortcut: writes hotkey.sh (0700), updates the managed
/// block in ~/.skhdrc, and restarts skhd when it is installed.
#[cfg(target_os = "macos")]
pub fn setup_macos(cfg: &config::Config) -> Result<(), String> {
    let shortcut = cfg.shortcut.macos.trim();
    if shortcut.is_empty() || shortcut.chars().any(|c| c.is_control()) {
        return Err(
            "[shortcut] macos must be a single non-empty line, e.g. \"cmd + shift - c\""
                .to_string(),
        );
    }

    let config_dir = config::config_dir();
    fsutil::ensure_private_dir(&config_dir)?;

    let log_path = config::log_path();
    if fsutil::is_symlink(&log_path) {
        return Err(
            "refusing to log through a symlink: quill.log (remove it and re-run `quill setup`)"
                .to_string(),
        );
    }
    if fs::symlink_metadata(&log_path).is_err() {
        fsutil::write_atomic_mode(&log_path, b"", 0o600)?;
    }

    let binary = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "quill".to_string());
    let hotkey_path = config_dir.join("hotkey.sh");
    let script = render_hotkey_script(&binary, &log_path.display().to_string());
    fsutil::write_atomic_mode(&hotkey_path, script.as_bytes(), 0o700)?;

    let home = home_dir();
    let rc_path = home.join(".skhdrc");
    let rc_target = skhdrc_target(&home)?;
    let (existing, mode) = read_skhdrc(&rc_target)?;
    let body = format!(
        "{shortcut} : {}",
        shell_quote(&hotkey_path.display().to_string())
    );
    let updated = apply_managed_block(&existing, &body)?;
    fsutil::write_atomic_mode(&rc_target, updated.as_bytes(), mode)?;

    let legacy = unmanaged_legacy_lines(&existing);
    if legacy > 0 {
        eprintln!(
            "[quill] warning: {} has {legacy} unmanaged line(s) running /quill/hotkey.sh; remove them to avoid a duplicate binding",
            rc_path.display()
        );
    }

    eprintln!("Shortcut installed: {shortcut}");
    eprintln!("skhd config: {}", rc_path.display());
    eprintln!("Hotkey script: {}", hotkey_path.display());
    if find_on_path("skhd").is_some() {
        let _ = std::process::Command::new("skhd")
            .arg("--restart-service")
            .output();
    } else {
        eprintln!("[quill] skhd is not on PATH; the shortcut is written but inactive. Install and start it:");
        eprintln!("  brew install koekeishiya/formulae/skhd");
        eprintln!("  skhd --start-service");
    }
    eprintln!();
    eprintln!(
        "Permissions quill needs on macOS (macOS asks you; nothing is granted automatically):"
    );
    eprintln!(
        "  1. Accessibility: enable skhd in System Settings > Privacy & Security > Accessibility."
    );
    eprintln!("     (skhd needs it to receive the shortcut and to click Edit > Copy in the frontmost app.)");
    eprintln!("  2. Automation: the first time you press the shortcut, macOS asks whether skhd may control System Events. Click Allow.");
    eprintln!("     If you clicked Don't Allow, enable it in System Settings > Privacy & Security > Automation > skhd > System Events.");
    eprintln!("  3. Notifications (optional): to see Quill's failure and permission banners, allow notifications for Script Editor in System Settings > Notifications.");
    eprintln!("No permission is needed for the clipboard, ~/.config/quill or a local Ollama. AI mode needs outbound HTTPS to your configured API URL.");
    eprintln!("If the shortcut does nothing: check the items above, then read ~/.config/quill/quill.log (re-run `quill setup` after upgrading to refresh hotkey.sh).");
    Ok(())
}

/// Reports (skhd on PATH, quill managed block present in ~/.skhdrc) for `quill status`.
/// Read-only: never creates or edits ~/.skhdrc.
#[cfg(target_os = "macos")]
pub fn hotkey_status() -> (bool, bool) {
    let installed = find_on_path("skhd").is_some();
    let block = skhdrc_target(&home_dir())
        .and_then(|target| read_skhdrc(&target))
        .map(|(text, _)| text.contains(BEGIN_MARKER))
        .unwrap_or(false);
    (installed, block)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(body: &str) -> String {
        format!("{BEGIN_MARKER}\n{body}\n{END_MARKER}\n")
    }

    #[test]
    fn shell_quote_wraps_plain_paths() {
        assert_eq!(
            shell_quote("/opt/a/.config/quill/hotkey.sh"),
            "'/opt/a/.config/quill/hotkey.sh'"
        );
    }

    #[test]
    fn shell_quote_escapes_apostrophes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
        assert_eq!(shell_quote("Bob's dir"), "'Bob'\\''s dir'");
    }

    #[test]
    fn apply_appends_to_empty_file() {
        assert_eq!(apply_managed_block("", "B").unwrap(), block("B"));
    }

    #[test]
    fn apply_appends_newline_when_missing() {
        let out = apply_managed_block("alt - t : open", "B").unwrap();
        assert_eq!(out, format!("alt - t : open\n{}", block("B")));
    }

    #[test]
    fn apply_replaces_only_the_block_and_keeps_surrounding_bytes() {
        let existing = format!("# top\n{}# bottom\nz = 1", block("old"));
        let out = apply_managed_block(&existing, "new").unwrap();
        assert_eq!(out, format!("# top\n{}# bottom\nz = 1", block("new")));
    }

    #[test]
    fn apply_is_idempotent() {
        let once = apply_managed_block("x\n", "B").unwrap();
        let twice = apply_managed_block(&once, "B").unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn apply_keeps_crlf_bytes_before_the_block() {
        let out = apply_managed_block("a\r\nb", "B").unwrap();
        assert_eq!(out, format!("a\r\nb\n{}", block("B")));
    }

    #[test]
    fn apply_rejects_unbalanced_or_duplicated_markers() {
        let cases = [
            format!("{BEGIN_MARKER}\nstray\n"),
            format!("{END_MARKER}\n"),
            format!("{END_MARKER}\n{BEGIN_MARKER}\n"),
            format!("{BEGIN_MARKER}\n{BEGIN_MARKER}\n{END_MARKER}\n"),
            format!("{}{}", block("a"), block("b")),
        ];
        for bad in cases {
            assert_eq!(apply_managed_block(&bad, "B").unwrap_err(), UNBALANCED);
        }
    }

    #[cfg(unix)]
    mod hotkey {
        use super::super::*;
        use std::fs;

        // ---- hotkey.sh permission handling ----
        // Spec: .ram-status/quill-permissions-20261029T074751Z/DESIGN.md (Revision 2/3).
        //
        // The generated script is executed under /bin/sh with a fake `osascript` and a fake
        // quill binary; nothing touches the real clipboard, System Events or notifications.
        // The sandbox directory name contains a space and an apostrophe on purpose.

        use std::os::unix::fs::PermissionsExt as _;
        use std::path::PathBuf;
        use std::process::{Command, Stdio};

        use std::os::unix::fs::symlink;
        use std::path::Path;
        use std::process::Child;
        use std::sync::Mutex;
        use std::time::{Duration, Instant};

        // Serialises "write an executable" and "fork": a freshly written fake must never be
        // exec'd while another test thread still holds a write fd open (ETXTBSY on Linux).
        static SPAWN_LOCK: Mutex<()> = Mutex::new(());

        const FAKE_OSASCRIPT: &str = r#"#!/bin/sh
case "$*" in
*"display notification"*)
  for a in "$@"; do printf '%s\n' "$a"; done >> "$NOTIFY_LOG"
  printf -- '---\n' >> "$NOTIFY_LOG"
  if [ -n "$FAKE_NOTIFY_BG" ]; then
    sleep 5 &
  fi
  exit 0;;
esac
if [ -n "$FAKE_OSA_BLOCK" ]; then
  : > "$OSA_STARTED"
  i=0
  while [ ! -e "$QUILL_RELEASE" ] && [ "$i" -lt 400 ]; do sleep 0.05; i=$((i+1)); done
fi
printf '%s' "$FAKE_OSA_OUT"
exit 0
"#;

        const FAKE_QUILL: &str = r#"#!/bin/sh
echo called >> "$QUILL_REC"
if [ -n "$FAKE_QUILL_ERR" ]; then printf '%s\n' "$FAKE_QUILL_ERR" >&2; fi
if [ -n "$FAKE_QUILL_BLOCK" ]; then
  : > "$QUILL_STARTED"
  i=0
  while [ ! -e "$QUILL_RELEASE" ] && [ "$i" -lt 400 ]; do sleep 0.05; i=$((i+1)); done
fi
if [ -n "$FAKE_QUILL_SLEEP" ]; then sleep "$FAKE_QUILL_SLEEP"; fi
exit "${FAKE_QUILL_RC:-0}"
"#;

        struct Sandbox {
            root: PathBuf,
        }

        struct Outcome {
            code: i32,
            quill_calls: usize,
            notify: String,
            log: String,
        }

        impl Outcome {
            fn banners(&self) -> usize {
                self.notify.lines().filter(|l| *l == "---").count()
            }
        }

        #[derive(Clone)]
        struct Opts {
            osa_out: String,
            quill_rc: i32,
            quill_err: String,
            block: bool,
            sleep: Option<String>,
            notify_bg: bool,
            osa_block: bool,
            path: Option<String>,
        }

        impl Opts {
            fn new(osa_out: &str, quill_rc: i32) -> Opts {
                Opts {
                    osa_out: osa_out.to_string(),
                    quill_rc,
                    quill_err: "[quill] processing 5 chars...\n[quill] done".to_string(),
                    block: false,
                    sleep: None,
                    notify_bg: false,
                    osa_block: false,
                    path: None,
                }
            }
            fn err(mut self, e: &str) -> Opts {
                self.quill_err = e.to_string();
                self
            }
            fn block(mut self) -> Opts {
                self.block = true;
                self
            }
            fn sleep(mut self, s: &str) -> Opts {
                self.sleep = Some(s.to_string());
                self
            }
            fn notify_bg(mut self) -> Opts {
                self.notify_bg = true;
                self
            }
            fn osa_block(mut self) -> Opts {
                self.osa_block = true;
                self
            }
            fn path(mut self, p: String) -> Opts {
                self.path = Some(p);
                self
            }
        }

        impl Sandbox {
            fn new() -> Sandbox {
                // unique per test: pid + monotonic counter (clock ticks alone can collide across threads)
                static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
                let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let root = std::env::temp_dir()
                    .join(format!("quill hk's dir {}-{seq}", std::process::id()));
                let _ = fs::remove_dir_all(&root);
                fs::create_dir_all(root.join("bin")).unwrap();
                let sb = Sandbox { root };
                {
                    let _g = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                    sb.write_exec("bin/osascript", FAKE_OSASCRIPT);
                    sb.write_exec("fake quill", FAKE_QUILL);
                    let script = render_hotkey_script(
                        &sb.p("fake quill").display().to_string(),
                        &sb.p("quill.log").display().to_string(),
                    );
                    fs::write(sb.p("hotkey.sh"), script).unwrap();
                }
                sb
            }

            fn p(&self, rel: &str) -> PathBuf {
                self.root.join(rel)
            }

            fn write_exec(&self, rel: &str, body: &str) -> PathBuf {
                let p = self.p(rel);
                fs::write(&p, body).unwrap();
                fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
                p
            }

            /// Adds (or replaces) a fake tool in the sandbox bin dir that shadows the real one.
            fn add_tool(&self, name: &str, body: &str) {
                let _g = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                self.write_exec(&format!("bin/{name}"), body);
            }

            fn command(&self, o: &Opts, barrier: Option<&Path>) -> Command {
                let script = self.p("hotkey.sh");
                let mut c = Command::new("/bin/sh");
                match barrier {
                    None => {
                        c.arg(&script);
                    }
                    Some(_) => {
                        // start barrier: wait (bounded, 10 s) for the GO file, then run the script
                        c.arg("-c").arg(
                        r#"i=0; while [ ! -e "$GO" ] && [ "$i" -lt 1000 ]; do sleep 0.01; i=$((i+1)); done; exec /bin/sh "$HK""#,
                    );
                    }
                }
                let default_path = format!("{}:/usr/bin:/bin", self.p("bin").display());
                // env_clear() first: everything below is set on the cleared environment
                c.env_clear()
                    .env("PATH", o.path.clone().unwrap_or(default_path))
                    .env("HOME", &self.root)
                    .env("NOTIFY_LOG", self.p("notify.log"))
                    .env("QUILL_REC", self.p("quill.rec"))
                    .env("QUILL_STARTED", self.p("quill.started"))
                    .env("QUILL_RELEASE", self.p("quill.release"))
                    .env("OSA_STARTED", self.p("osa.started"))
                    .env("FAKE_OSA_OUT", &o.osa_out)
                    .env("FAKE_QUILL_RC", o.quill_rc.to_string())
                    .env("FAKE_QUILL_ERR", &o.quill_err)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null());
                if let Some(go) = barrier {
                    c.env("GO", go).env("HK", &script);
                }
                if o.block {
                    c.env("FAKE_QUILL_BLOCK", "1");
                }
                if o.osa_block {
                    c.env("FAKE_OSA_BLOCK", "1");
                }
                if let Some(s) = &o.sleep {
                    c.env("FAKE_QUILL_SLEEP", s);
                }
                if o.notify_bg {
                    c.env("FAKE_NOTIFY_BG", "1");
                }
                c
            }

            fn spawn(&self, o: &Opts) -> Child {
                let mut c = self.command(o, None);
                let _g = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                c.spawn().unwrap()
            }

            fn spawn_barrier(&self, o: &Opts, go: &Path) -> Child {
                let mut c = self.command(o, Some(go));
                let _g = SPAWN_LOCK.lock().unwrap_or_else(|e| e.into_inner());
                c.spawn().unwrap()
            }

            fn outcome(&self, code: i32) -> Outcome {
                Outcome {
                    code,
                    quill_calls: fs::read_to_string(self.p("quill.rec"))
                        .map(|s| s.lines().count())
                        .unwrap_or(0),
                    notify: fs::read_to_string(self.p("notify.log")).unwrap_or_default(),
                    log: fs::read_to_string(self.p("quill.log")).unwrap_or_default(),
                }
            }

            /// Runs the script to completion. Records accumulate across sequential runs in one
            /// sandbox (use a fresh Sandbox, or `reset()`, for independent counts).
            fn run_opts(&self, o: &Opts) -> Outcome {
                let status = self.spawn(o).wait().unwrap();
                self.outcome(status.code().unwrap_or(-1))
            }

            fn run(&self, osa_out: &str, quill_rc: i32) -> Outcome {
                self.run_opts(&Opts::new(osa_out, quill_rc))
            }

            fn wait_for(&self, rel: &str) {
                let deadline = Instant::now() + Duration::from_secs(10);
                while !self.p(rel).exists() {
                    assert!(Instant::now() < deadline, "timed out waiting for {rel}");
                    std::thread::sleep(Duration::from_millis(20));
                }
            }

            fn release(&self) {
                fs::write(self.p("quill.release"), "").unwrap();
            }

            /// A PATH that has the fakes and the shell utilities the script needs, but no perl.
            fn path_without_perl(&self) -> String {
                let dir = self.p("nobin");
                fs::create_dir_all(&dir).unwrap();
                for tool in [
                    "dirname", "mktemp", "chmod", "rm", "date", "cat", "grep", "sed", "tail",
                    "cut", "wc", "tr", "mv", "sleep", "head",
                ] {
                    for base in ["/usr/bin", "/bin"] {
                        let src = format!("{base}/{tool}");
                        if Path::new(&src).exists() {
                            let _ = symlink(&src, dir.join(tool));
                            break;
                        }
                    }
                }
                let _ = symlink(self.p("bin/osascript"), dir.join("osascript"));
                dir.display().to_string()
            }
        }

        impl Drop for Sandbox {
            fn drop(&mut self) {
                let _ = fs::write(self.root.join("quill.release"), "");
                let _ = fs::remove_dir_all(&self.root);
            }
        }

        fn signal(pid: u32, sig: &str) {
            let st = Command::new("kill")
                .arg(format!("-{sig}"))
                .arg(pid.to_string())
                .status()
                .unwrap();
            assert!(st.success(), "kill -{sig} {pid} failed");
        }

        /// "YYYY-MM-DD HH:MM:SS run pid=<n> rc=<n>"
        fn is_header(line: &str) -> bool {
            let b = line.as_bytes();
            if b.len() < 30 || !line.is_ascii() {
                return false;
            }
            let digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18]
                .iter()
                .all(|&i| b[i].is_ascii_digit());
            digits
                && b[4] == b'-'
                && b[7] == b'-'
                && b[10] == b' '
                && b[13] == b':'
                && b[16] == b':'
                && line[19..].starts_with(" run pid=")
                && line.contains(" rc=")
        }

        fn headers(log: &str) -> usize {
            log.lines().filter(|l| is_header(l)).count()
        }

        fn mode_of(p: &Path) -> u32 {
            fs::metadata(p).unwrap().permissions().mode() & 0o777
        }

        fn is_root() -> bool {
            Command::new("id")
                .arg("-u")
                .output()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "0")
                .unwrap_or(false)
        }

        fn assert_stopped(o: &Outcome, words: &[&str], why: &str) {
            assert_eq!(o.quill_calls, 0, "{why}: quill must NOT run");
            assert_ne!(o.code, 0, "{why}: script must exit non-zero");
            for w in words {
                assert!(
                    o.notify.contains(w),
                    "{why}: notification lacks {w:?}: {}",
                    o.notify
                );
                assert!(o.log.contains(w), "{why}: log lacks {w:?}: {}", o.log);
            }
        }

        #[test]
        fn hotkey_copy_ok_runs_quill_once_and_shows_the_success_banner() {
            let sb = Sandbox::new();
            let o = sb.run("ok", 0);
            assert_eq!(o.code, 0);
            assert_eq!(o.quill_calls, 1);
            assert!(o.notify.contains("paste with Cmd+V"), "{}", o.notify);
        }

        #[test]
        fn hotkey_accessibility_denied_25211_stops_with_message() {
            let sb = Sandbox::new();
            let o = sb.run(
            "ERR -25211: System Events got an error: osascript is not allowed assistive access.",
            0,
        );
            assert_stopped(&o, &["Accessibility", "skhd"], "-25211");
        }

        #[test]
        fn hotkey_accessibility_denied_1719_assistive_stops_with_message() {
            let sb = Sandbox::new();
            let o = sb.run(
                "ERR -1719: System Events got an error: osascript is not allowed assistive access.",
                0,
            );
            assert_stopped(&o, &["Accessibility", "skhd"], "-1719 assistive");
        }

        #[test]
        fn hotkey_automation_denied_1743_stops_with_message() {
            let sb = Sandbox::new();
            let o = sb.run(
                "ERR -1743: Not authorized to send Apple events to System Events.",
                0,
            );
            assert_stopped(&o, &["Automation", "System Events", "skhd"], "-1743");
        }

        #[test]
        fn hotkey_automation_wins_when_both_patterns_appear() {
            let sb = Sandbox::new();
            let o = sb.run("ERR -1743: Not authorized ... assistive access (-25211)", 0);
            assert_stopped(&o, &["Automation"], "both patterns");
        }

        #[test]
        fn hotkey_unrecognised_1719_stops_with_generic_message() {
            // A localised Accessibility denial has the same number but no English words.
            let sb = Sandbox::new();
            let o = sb.run(
            "ERR -1719: System Events hat einen Fehler festgestellt: osascript darf nicht auf Hilfsgeraete zugreifen.",
            0,
        );
            assert_stopped(
                &o,
                &["-1719", "Accessibility", "Automation"],
                "localised -1719",
            );
        }

        #[test]
        fn hotkey_benign_menu_failures_still_run_quill() {
            for out in [
                r#"ERR -1719: System Events got an error: Can't get menu "Edit" of menu bar 1. Invalid index."#,
                "ERR -1719: Invalid index.",
                "ERR -1719: Can't get menu item Copy",
                r#"ERR -1728: Can't get menu item "Copy" of menu "Edit""#,
                "ERR -9999: something unexpected",
                "",
            ] {
                let sb = Sandbox::new();
                let o = sb.run(out, 0);
                assert_eq!(o.quill_calls, 1, "{out:?} must continue to quill");
                assert_eq!(o.code, 0, "{out:?}");
                assert!(
                    !o.notify.contains("Accessibility") && !o.notify.contains("Automation"),
                    "{out:?}: {}",
                    o.notify
                );
            }
        }

        #[test]
        fn hotkey_quill_failure_still_notifies_with_log_pointer() {
            let sb = Sandbox::new();
            let o = sb.run("ok", 1);
            assert_eq!(o.quill_calls, 1);
            assert!(o.notify.contains("Quill failed"), "{}", o.notify);
            assert!(o.notify.contains("quill.log"), "{}", o.notify);
        }

        #[test]
        fn hotkey_notification_text_is_passed_as_the_last_argv_item() {
            let sb = Sandbox::new();
            let o = sb.run(
                "ERR -1743: Not authorized to send Apple events to System Events.",
                0,
            );
            let lines: Vec<&str> = o.notify.lines().collect();
            assert!(lines.len() >= 4, "{lines:?}");
            assert_eq!(lines[lines.len() - 1], "---", "{lines:?}");
            assert_eq!(lines[lines.len() - 3], "--", "{lines:?}");
            assert!(lines[lines.len() - 2].contains("Automation"), "{lines:?}");
            // the message is data, never spliced into an AppleScript -e line
            for l in &lines[..lines.len() - 2] {
                assert!(
                    !l.contains("Automation"),
                    "message leaked into script source: {l}"
                );
            }
        }

        #[test]
        fn hotkey_script_is_valid_sh_with_private_umask_and_awkward_paths() {
            let sb = Sandbox::new();
            let _ = sb.run("ok", 0);
            let script = sb.root.join("hotkey.sh");
            let ok = Command::new("/bin/sh")
                .arg("-n")
                .arg(&script)
                .status()
                .unwrap();
            assert!(ok.success());
            let text = fs::read_to_string(&script).unwrap();
            assert!(text.starts_with("#!/bin/sh\n"), "{text}");
            assert!(text.contains("umask 077"), "{text}");
            assert!(text.contains("quill.log"), "{text}");
        }

        #[test]
        fn hotkey_messages_are_shell_safe_and_name_the_fix() {
            for m in [MSG_ACCESSIBILITY, MSG_AUTOMATION, MSG_COPY_FAILED] {
                assert!(
                    !m.chars().any(|c| "'\"\\$`".contains(c)),
                    "unsafe character in {m:?}"
                );
                assert!(m.contains("skhd") && m.contains("System Settings"), "{m}");
            }
            assert!(MSG_ACCESSIBILITY.contains("Accessibility"));
            assert!(
                MSG_AUTOMATION.contains("Automation") && MSG_AUTOMATION.contains("System Events")
            );
            assert!(
                MSG_COPY_FAILED.contains("-1719")
                    && MSG_COPY_FAILED.contains("Accessibility")
                    && MSG_COPY_FAILED.contains("Automation")
            );
        }

        /// Compile-only check (never executed) that the AppleScript embedded in hotkey.sh is
        /// valid; a syntax error would otherwise be hidden by the script's stderr handling.
        #[cfg(target_os = "macos")]
        #[test]
        fn hotkey_applescript_sources_compile_with_osacompile() {
            let sb = Sandbox::new();
            for (name, lines) in [("notify", NOTIFY_APPLESCRIPT), ("copy", COPY_APPLESCRIPT)] {
                let mut cmd = Command::new("/usr/bin/osacompile");
                cmd.arg("-o").arg(sb.root.join(format!("{name}.scpt")));
                for l in lines {
                    cmd.arg("-e").arg(l);
                }
                match cmd.output() {
                    Ok(out) => assert!(
                        out.status.success(),
                        "{name} AppleScript failed to compile: {}",
                        String::from_utf8_lossy(&out.stderr)
                    ),
                    Err(e) => eprintln!("osacompile unavailable ({e}); skipping {name}"),
                }
            }
        }

        // ---- reliability: banners, appended log, single instance (spec: DESIGN Rev 4) ----

        #[test]
        fn hotkey_success_shows_paste_banner_and_logs_a_header() {
            let sb = Sandbox::new();
            let o = sb.run("ok", 0);
            assert_eq!(o.code, 0);
            assert_eq!(o.banners(), 1, "{}", o.notify);
            assert!(
                o.notify.contains("result is on the clipboard") && o.notify.contains("Cmd+V"),
                "{}",
                o.notify
            );
            assert_eq!(headers(&o.log), 1, "{}", o.log);
            assert!(o.log.lines().any(|l| is_header(l) && l.ends_with(" rc=0")));
            assert!(o.log.contains("[quill] done"), "{}", o.log);
        }

        fn banner_for(err: &str, rc: i32) -> String {
            let sb = Sandbox::new();
            let o = sb.run_opts(&Opts::new("ok", rc).err(err));
            assert_eq!(o.banners(), 1, "{}", o.notify);
            o.notify
        }

        #[test]
        fn hotkey_fallback_banner_names_the_reason() {
            let n = banner_for(
            "[quill] processing 5 chars...\n[quill] AI request failed (API error: HTTP 401 (authentication failed)); falling back to offline grammar mode.\n[quill] done (offline grammar mode)",
            0,
        );
            assert!(
                n.contains("AI failed (API error: HTTP 401 (authentication failed))"),
                "{n}"
            );
            assert!(
                n.contains("used offline grammar fixes") && n.contains("Cmd+V"),
                "{n}"
            );
        }

        #[test]
        fn hotkey_fallback_with_nothing_to_fix_says_so() {
            let n = banner_for(
            "[quill] AI request failed (timeout); falling back to offline grammar mode.\n[quill] no changes",
            0,
        );
            assert!(
                n.contains("AI failed (timeout)") && n.contains("found nothing to fix"),
                "{n}"
            );
            assert!(!n.contains("Cmd+V"), "{n}");
        }

        #[test]
        fn hotkey_offline_only_and_no_change_banners() {
            let n = banner_for("[quill] done (offline grammar mode)", 0);
            assert!(
                n.contains("offline grammar fixes applied") && n.contains("Cmd+V"),
                "{n}"
            );
            let n = banner_for("[quill] no changes", 0);
            assert!(
                n.contains("no changes found") && !n.contains("Cmd+V"),
                "{n}"
            );
        }

        #[test]
        fn hotkey_failure_banner_shows_the_last_stderr_line() {
            let n = banner_for(
                "[quill] processing 5 chars...\n[quill] clipboard is empty or has no text",
                1,
            );
            assert!(
                n.contains("Quill failed: clipboard is empty or has no text. See"),
                "{n}"
            );
            assert!(n.contains("quill.log"), "{n}");
            let n = banner_for("", 3);
            assert!(n.contains("Quill failed (exit 3)"), "{n}");
        }

        #[test]
        fn hotkey_unknown_output_gets_a_generic_banner() {
            let n = banner_for("something odd", 0);
            assert!(n.contains("Quill: finished"), "{n}");
        }

        #[test]
        fn hotkey_status_lines_are_matched_exactly_not_as_substrings() {
            // a diagnostic that merely contains the words must not decide the banner
            let n = banner_for(
            "[quill] AI request failed (no changes yet); falling back to offline grammar mode.\n[quill] done (offline grammar mode)",
            0,
        );
            assert!(n.contains("used offline grammar fixes"), "{n}");
            assert!(!n.contains("found nothing"), "{n}");
            for err in [
                "q done\n[quill] done.",
                "  [quill] done",
                "[quill] done!",
                "[quill] donex",
            ] {
                let n = banner_for(err, 0);
                assert!(n.contains("Quill: finished"), "{err:?} => {n}");
            }
        }

        #[test]
        fn hotkey_banner_text_is_data_not_code_and_is_bounded() {
            let sb = Sandbox::new();
            let nasty =
                "[quill] it said \"no\" and 'maybe' $(touch pwned) `touch pwned` ; rm -rf /";
            let o = sb.run_opts(&Opts::new("ok", 1).err(nasty));
            assert!(o.notify.contains("$(touch pwned)"), "{}", o.notify);
            assert!(!Path::new("pwned").exists() && !sb.p("pwned").exists());
            let long = format!("[quill] {}", "x".repeat(400));
            let sb = Sandbox::new();
            let o = sb.run_opts(&Opts::new("ok", 1).err(&long));
            let lines: Vec<&str> = o.notify.lines().collect();
            let msg = lines[lines.len() - 2];
            assert!(msg.len() < 260, "banner not bounded: {} chars", msg.len());
            assert!(msg.ends_with("quill.log"), "{msg}");
        }

        #[test]
        fn hotkey_log_is_appended_across_runs() {
            let sb = Sandbox::new();
            let _ = sb.run("ok", 0);
            let o = sb.run_opts(&Opts::new("ok", 0).err("[quill] no changes"));
            assert_eq!(headers(&o.log), 2, "{}", o.log);
            assert!(o.log.contains("[quill] done") && o.log.contains("[quill] no changes"));
        }

        #[test]
        fn hotkey_log_is_trimmed_and_stays_private() {
            let sb = Sandbox::new();
            let mut old = String::new();
            for i in 0..3000 {
                old.push_str(&format!("old line {i:05} padding padding padding\n"));
            }
            assert!(old.len() > 100_000);
            fs::write(sb.p("quill.log"), &old).unwrap();
            let o = sb.run("ok", 0);
            let n = o.log.lines().count();
            assert!((300..=400).contains(&n), "{n} lines left");
            assert!(
                o.log.contains("[quill] done"),
                "new block must survive the trim"
            );
            assert!(
                !o.log.contains("old line 00000"),
                "oldest lines must be gone"
            );
            assert_eq!(mode_of(&sb.p("quill.log")), 0o600);
        }

        #[test]
        fn hotkey_existing_log_is_made_private() {
            let sb = Sandbox::new();
            fs::write(sb.p("quill.log"), "old\n").unwrap();
            fs::set_permissions(sb.p("quill.log"), fs::Permissions::from_mode(0o644)).unwrap();
            let o = sb.run("ok", 0);
            assert_eq!(mode_of(&sb.p("quill.log")), 0o600);
            assert!(
                o.log.starts_with("old\n") && headers(&o.log) == 1,
                "{}",
                o.log
            );
        }

        #[test]
        fn hotkey_symlinked_or_non_regular_log_is_refused() {
            let sb = Sandbox::new();
            let victim = sb.p("victim.txt");
            fs::write(&victim, "VICTIM").unwrap();
            symlink(&victim, sb.p("quill.log")).unwrap();
            let o = sb.run("ok", 0);
            assert_ne!(o.code, 0);
            assert_eq!(o.quill_calls, 0, "quill must not run");
            assert_eq!(o.banners(), 1, "{}", o.notify);
            assert!(o.notify.contains("log file"), "{}", o.notify);
            assert_eq!(fs::read_to_string(&victim).unwrap(), "VICTIM");

            let sb = Sandbox::new();
            fs::create_dir(sb.p("quill.log")).unwrap();
            let o = sb.run("ok", 0);
            assert_ne!(o.code, 0);
            assert_eq!(o.quill_calls, 0);
            assert_eq!(o.banners(), 1, "{}", o.notify);
            assert!(o.notify.contains("not a regular file"), "{}", o.notify);
        }

        #[test]
        fn hotkey_failing_chmod_stops_before_copying() {
            let sb = Sandbox::new();
            sb.add_tool("chmod", "#!/bin/sh\nexit 1\n");
            fs::write(sb.p("quill.log"), "old\n").unwrap();
            let o = sb.run("ok", 0);
            assert_ne!(o.code, 0);
            assert_eq!(o.quill_calls, 0);
            assert_eq!(o.banners(), 1, "{}", o.notify);
            assert!(o.notify.contains("cannot restrict"), "{}", o.notify);
            assert_eq!(o.log, "old\n");
        }

        #[test]
        fn hotkey_unwritable_log_is_reported_not_swallowed() {
            if is_root() {
                eprintln!("running as root: file modes do not block writes; skipping");
                return;
            }
            let sb = Sandbox::new();
            sb.add_tool("chmod", "#!/bin/sh\nexit 0\n"); // so the preflight cannot repair the mode
            fs::write(sb.p("quill.log"), "old\n").unwrap();
            fs::set_permissions(sb.p("quill.log"), fs::Permissions::from_mode(0o444)).unwrap();
            let o = sb.run("ok", 0);
            assert_ne!(o.code, 0);
            assert_eq!(o.quill_calls, 1);
            assert_eq!(o.banners(), 1, "{}", o.notify);
            assert!(o.notify.contains("cannot write"), "{}", o.notify);
            assert!(
                o.notify.contains("Cmd+V"),
                "the normal banner is still shown: {}",
                o.notify
            );
            assert_eq!(fs::read_to_string(sb.p("quill.log")).unwrap(), "old\n");
        }

        #[test]
        fn hotkey_leaves_no_temp_files_and_releases_after_a_permission_stop() {
            let sb = Sandbox::new();
            let o = sb.run(
                "ERR -1743: Not authorized to send Apple events to System Events.",
                0,
            );
            assert_ne!(o.code, 0);
            assert_eq!(o.quill_calls, 0);
            let o = sb.run("ok", 0);
            assert_eq!(
                o.quill_calls, 1,
                "the lock must be free after a permission stop"
            );
            let leftovers: Vec<String> = fs::read_dir(&sb.root)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with(".hotkey.") && n != ".hotkey.lock")
                .collect();
            assert!(
                leftovers.is_empty(),
                "temp files left behind: {leftovers:?}"
            );
        }

        #[test]
        fn hotkey_concurrent_triggers_run_quill_exactly_once() {
            for round in 0..5 {
                let sb = Sandbox::new();
                let go = sb.p("go");
                let o = Opts::new("ok", 0).sleep("0.5");
                let kids: Vec<Child> = (0..8).map(|_| sb.spawn_barrier(&o, &go)).collect();
                fs::write(&go, "").unwrap();
                for mut k in kids {
                    assert!(k.wait().unwrap().success());
                }
                let out = sb.outcome(0);
                assert_eq!(
                    out.quill_calls, 1,
                    "round {round}: quill ran {} times",
                    out.quill_calls
                );
                assert_eq!(out.banners(), 1, "round {round}: {}", out.notify);
                assert_eq!(headers(&out.log), 1, "round {round}: {}", out.log);
            }
        }

        #[test]
        fn hotkey_lock_follows_the_running_quill_even_if_the_shell_is_killed() {
            let sb = Sandbox::new();
            let mut owner = sb.spawn(&Opts::new("ok", 0).block());
            sb.wait_for("quill.started");
            signal(owner.id(), "KILL"); // the shell dies; its foreground quill keeps running
            let _ = owner.wait();
            let o = sb.run("ok", 0);
            assert_eq!(o.code, 0);
            assert_eq!(
                o.quill_calls, 1,
                "contender must skip while the old quill lives"
            );
            assert_eq!(o.banners(), 0, "contender must be silent: {}", o.notify);
            sb.release();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let o = sb.run("ok", 0);
                if o.quill_calls >= 2 {
                    break;
                }
                assert!(Instant::now() < deadline, "lock never became free");
                std::thread::sleep(Duration::from_millis(100));
            }
        }

        #[test]
        fn hotkey_live_owner_is_never_taken_over_whatever_the_lock_file_age() {
            let sb = Sandbox::new();
            let mut owner = sb.spawn(&Opts::new("ok", 0).block());
            sb.wait_for("quill.started");
            let st = Command::new("touch")
                .args(["-t", "200001010000"])
                .arg(sb.p(".hotkey.lock"))
                .status()
                .unwrap();
            assert!(st.success());
            let o = sb.run("ok", 0);
            assert_eq!(o.quill_calls, 1);
            assert_eq!(o.banners(), 0, "{}", o.notify);
            sb.release();
            assert!(owner.wait().unwrap().success());
        }

        #[test]
        fn hotkey_notification_helpers_do_not_keep_the_lock() {
            let sb = Sandbox::new();
            let a = sb.run_opts(&Opts::new("ok", 0).notify_bg()); // banner starts `sleep 5 &`
            assert_eq!(a.quill_calls, 1);
            let b = sb.run_opts(&Opts::new("ok", 0).notify_bg());
            assert_eq!(
                b.quill_calls, 2,
                "a lingering notifier must not hold the lock"
            );
        }

        #[test]
        fn hotkey_sigterm_releases_after_the_child_ends_and_cleans_up() {
            let sb = Sandbox::new();
            let mut owner = sb.spawn(&Opts::new("ok", 0).block());
            sb.wait_for("quill.started");
            signal(owner.id(), "TERM");
            sb.release();
            let st = owner.wait().unwrap();
            assert_eq!(st.code(), Some(143), "{st:?}");
            let o = sb.run("ok", 0);
            assert_eq!(o.quill_calls, 2, "lock must be free after the owner ended");
            let leftovers: Vec<String> = fs::read_dir(&sb.root)
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
                .filter(|n| n.starts_with(".hotkey.") && n != ".hotkey.lock")
                .collect();
            assert!(leftovers.is_empty(), "{leftovers:?}");
        }

        #[test]
        fn hotkey_unusable_lock_file_is_reported() {
            let sb = Sandbox::new();
            let victim = sb.p("victim.txt");
            fs::write(&victim, "VICTIM").unwrap();
            symlink(&victim, sb.p(".hotkey.lock")).unwrap();
            let o = sb.run("ok", 0);
            assert_ne!(o.code, 0);
            assert_eq!(o.quill_calls, 0);
            assert_eq!(o.banners(), 1, "{}", o.notify);
            assert!(o.notify.contains("lock file"), "{}", o.notify);
            assert_eq!(fs::read_to_string(&victim).unwrap(), "VICTIM");

            let sb = Sandbox::new();
            fs::create_dir(sb.p(".hotkey.lock")).unwrap();
            let o = sb.run("ok", 0);
            assert_ne!(o.code, 0);
            assert_eq!(o.quill_calls, 0);
            assert!(o.notify.contains("lock file"), "{}", o.notify);
        }

        #[test]
        fn hotkey_without_perl_degrades_to_running_unlocked_and_says_so() {
            let sb = Sandbox::new();
            let p = sb.path_without_perl();
            let o = sb.run_opts(&Opts::new("ok", 0).path(p));
            assert_eq!(o.code, 0);
            assert_eq!(o.quill_calls, 1);
            assert_eq!(o.banners(), 1, "{}", o.notify);
            assert!(
                o.log
                    .contains("[quill] warning: single-instance lock unavailable"),
                "{}",
                o.log
            );
        }

        #[test]
        fn hotkey_template_substitution_is_single_pass_and_complete() {
            let s = render_hotkey_script("/bin/x", "/tmp/@MSG_AUTOMATION@/@BIN@/quill.log");
            assert!(s.contains("'/tmp/@MSG_AUTOMATION@/@BIN@/quill.log'"), "{s}");
            let s = render_hotkey_script("/bin/x", "/tmp/quill.log");
            assert!(!s.contains('@'), "unreplaced placeholder in:\n{s}");
            let ok = Command::new("/bin/sh")
                .arg("-n")
                .arg("-c")
                .arg(&s)
                .status()
                .unwrap();
            assert!(ok.success());
        }

        #[test]
        fn hotkey_copy_step_keeps_the_lock_when_the_shell_is_killed() {
            let sb = Sandbox::new();
            let mut owner = sb.spawn(&Opts::new("ok", 0).osa_block());
            sb.wait_for("osa.started");
            signal(owner.id(), "KILL"); // shell dies; its Edit>Copy osascript keeps running
            let _ = owner.wait();
            let o = sb.run("ok", 0);
            assert_eq!(
                o.quill_calls, 0,
                "contender must skip while the copy step lives"
            );
            assert_eq!(o.banners(), 0, "{}", o.notify);
            sb.release();
            let deadline = Instant::now() + Duration::from_secs(10);
            loop {
                let o = sb.run("ok", 0);
                if o.quill_calls >= 1 {
                    break;
                }
                assert!(Instant::now() < deadline, "lock never became free");
                std::thread::sleep(Duration::from_millis(100));
            }
        }

        #[test]
        fn hotkey_sweeps_stale_temp_files_but_keeps_fresh_ones() {
            let sb = Sandbox::new();
            let stale = sb.p(".hotkey.AbCdEf");
            let fresh = sb.p(".hotkey.ZzYyXx");
            let other = sb.p("notes.txt");
            for f in [&stale, &fresh, &other] {
                fs::write(f, "x").unwrap();
            }
            for f in [&stale, &other] {
                let st = Command::new("touch")
                    .args(["-t", "200001010000"])
                    .arg(f)
                    .status()
                    .unwrap();
                assert!(st.success());
            }
            let o = sb.run("ok", 0);
            assert_eq!(o.quill_calls, 1);
            assert!(!stale.exists(), "stale temp file must be swept");
            assert!(fresh.exists(), "a fresh temp file may belong to a live run");
            assert!(other.exists(), "unrelated files are never touched");
            assert!(
                sb.p(".hotkey.lock").exists(),
                "the lock file itself is never swept"
            );
        }

        #[test]
        fn hotkey_log_trim_also_caps_bytes() {
            let sb = Sandbox::new();
            let mut old = String::from("short line\n");
            old.push_str(&"y".repeat(300_000));
            old.push('\n');
            fs::write(sb.p("quill.log"), &old).unwrap();
            let o = sb.run("ok", 0);
            assert!(o.log.len() <= 100_000, "log is {} bytes", o.log.len());
            assert!(o.log.contains("[quill] done"), "newest block must survive");
            assert_eq!(mode_of(&sb.p("quill.log")), 0o600);
        }

        #[test]
        fn hotkey_lock_error_other_than_busy_is_not_treated_as_busy() {
            // the script must only treat EWOULDBLOCK as "somebody else is running"
            let s = render_hotkey_script("/bin/x", "/tmp/quill.log");
            assert!(s.contains("$!{EWOULDBLOCK} ? 1 : 2"), "{s}");
        }
    }
}
