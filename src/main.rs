//! Quill — silent system-wide text transformer powered by Claude.
//!
//! Commands:
//!   quill init          Create default config at ~/.config/quill/config.toml
//!   quill setup         Install keyboard shortcut
//!   quill --now         Transform clipboard (one-shot)
//!   quill --now -p X    Transform using prompt profile X
//!   quill learn         Analyze history and update writing context
//!   quill               Show usage

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod modules;

use modules::{claude, clipboard, config, context, history};

// ── One-shot transform ────────────────────────────────────────

fn transform_clipboard(prompt_name: Option<&str>) -> i32 {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[quill] {e}");
            return 1;
        }
    };

    let instruction = match config::resolve_instruction(&cfg, prompt_name) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("[quill] {e}");
            return 1;
        }
    };

    let text = match clipboard::get_text() {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            eprintln!("[quill] clipboard is empty or has no text");
            return 1;
        }
    };

    // Load writing style context
    context::ensure_exists();
    let user_context = context::load();

    eprintln!("[quill] processing {} chars...", text.len());

    match claude::transform(&cfg, &instruction, &user_context, &text) {
        Ok(result) => {
            let clean = result.trim();
            // Record before/after for learning
            history::append(&text, clean);
            if clipboard::set_text(clean) {
                eprintln!("[quill] done");
                0
            } else {
                eprintln!("[quill] failed to write result to clipboard");
                1
            }
        }
        Err(e) => {
            eprintln!("[quill] error: {e}");
            1
        }
    }
}

// ── Init: create default config ───────────────────────────────

fn cmd_init() {
    match config::write_default() {
        Ok(path) => {
            eprintln!("Config created at: {}", path.display());
            eprintln!("Prompts directory: {}", config::prompts_dir().display());
            eprintln!();
            eprintln!("Next steps:");
            eprintln!("  1. Edit the config and set your API url, key, and user_name");
            eprintln!("  2. Run `quill setup` to install the keyboard shortcut");
        }
        Err(e) => eprintln!("[quill] {e}"),
    }
}

// ── Learn: analyze history and update context ────────────────

fn cmd_learn() {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[quill] {e}");
            return;
        }
    };

    let entries = history::load_all();
    if entries.is_empty() {
        eprintln!("[quill] no transformation history found.");
        eprintln!("Use quill a few times first, then run `quill learn`.");
        return;
    }

    eprintln!("[quill] analyzing {} transformations...", entries.len());

    // Build history text for Claude
    let history_text: String = entries
        .iter()
        .enumerate()
        .map(|(i, e)| {
            format!(
                "--- Transformation {} ---\nBEFORE:\n{}\n\nAFTER:\n{}\n",
                i + 1,
                e.before,
                e.after
            )
        })
        .collect();

    let existing_context = context::load();

    match claude::analyze_style(&cfg, &history_text, &existing_context) {
        Ok(new_context) => {
            match context::save(&new_context) {
                Ok(()) => {
                    eprintln!("[quill] context updated at: {}", config::context_path().display());
                    eprintln!();
                    eprintln!("{new_context}");
                    // Clear processed history
                    history::clear();
                    eprintln!();
                    eprintln!("History cleared. Future transforms will use these preferences.");
                }
                Err(e) => eprintln!("[quill] {e}"),
            }
        }
        Err(e) => eprintln!("[quill] failed to analyze: {e}"),
    }
}

// ── Setup: install keyboard shortcut ──────────────────────────

fn cmd_setup() {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[quill] {e}");
            return;
        }
    };

    #[cfg(target_os = "macos")]
    setup_macos(&cfg);

    #[cfg(target_os = "windows")]
    setup_windows(&cfg);

    #[cfg(target_os = "linux")]
    eprintln!("Linux: bind your DE shortcut to run `quill --now`");
}

#[cfg(target_os = "macos")]
fn setup_macos(cfg: &config::Config) {
    let binary = std::env::current_exe()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "quill".to_string());

    let hotkey_script = format!(
        "#!/bin/bash\n\
         # Auto-generated by `quill setup`\n\
         # Copies selected text via Edit>Copy menu, then transforms via Claude.\n\
         osascript -e 'tell application \"System Events\"' \
         -e 'try' \
         -e 'click menu item \"Copy\" of menu \"Edit\" of menu bar 1 of \
         (first application process whose frontmost is true)' \
         -e 'end try' \
         -e 'end tell'\n\
         sleep 0.1\n\
         \"{binary}\" --now 2>/tmp/quill.log\n"
    );

    let script_path = config::config_dir().join("hotkey.sh");
    if let Err(e) = std::fs::write(&script_path, &hotkey_script) {
        eprintln!("[quill] failed to write {}: {e}", script_path.display());
        return;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755));
    }

    let skhd_line = format!(
        "# Quill — auto-generated by `quill setup`\n\
         {} : {}\n",
        cfg.shortcut.macos,
        script_path.display()
    );

    let skhd_path = dirs_home().join(".skhdrc");
    let existing = std::fs::read_to_string(&skhd_path).unwrap_or_default();

    let cleaned: String = existing
        .lines()
        .filter(|l| !l.contains("quill") && !l.contains("Quill")
            && !l.contains("transformer") && !l.contains("Transformer"))
        .collect::<Vec<_>>()
        .join("\n");
    let new_content = if cleaned.trim().is_empty() {
        skhd_line
    } else {
        format!("{}\n{}", cleaned.trim(), skhd_line)
    };

    if let Err(e) = std::fs::write(&skhd_path, &new_content) {
        eprintln!("[quill] failed to write {}: {e}", skhd_path.display());
        return;
    }

    let _ = std::process::Command::new("skhd")
        .arg("--restart-service")
        .output();

    eprintln!("Shortcut installed: {}", cfg.shortcut.macos);
    eprintln!("skhd config: {}", skhd_path.display());
    eprintln!("Hotkey script: {}", script_path.display());
    eprintln!();
    eprintln!("Make sure skhd has Accessibility access in:");
    eprintln!("  System Settings > Privacy & Security > Accessibility");
}

#[cfg(target_os = "windows")]
fn setup_windows(cfg: &config::Config) {
    eprintln!("Windows shortcut: {}", cfg.shortcut.windows);
    eprintln!("Run `quill` (no flags) to start the background hotkey daemon.");
}

fn dirs_home() -> std::path::PathBuf {
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::path::PathBuf::from("."))
}

// ── Windows: native hotkey daemon ─────────────────────────────

#[cfg(target_os = "windows")]
fn windows_hotkey_loop() {
    #[link(name = "user32")]
    extern "system" {
        fn RegisterHotKey(hwnd: usize, id: i32, mods: u32, vk: u32) -> i32;
        fn GetMessageW(msg: *mut [u8; 48], hwnd: usize, min: u32, max: u32) -> i32;
    }

    const MOD_CTRL: u32 = 0x0002;
    const MOD_SHIFT: u32 = 0x0004;
    const VK_C: u32 = 0x43;
    const WM_HOTKEY: u32 = 0x0312;

    unsafe {
        if RegisterHotKey(0, 1, MOD_CTRL | MOD_SHIFT, VK_C) == 0 {
            eprintln!("[quill] failed to register hotkey");
            return;
        }
        eprintln!("[quill] listening for Ctrl+Shift+C...");
        loop {
            let mut msg = [0u8; 48];
            if GetMessageW(&mut msg, 0, 0, 0) > 0 {
                let message = u32::from_ne_bytes(msg[8..12].try_into().unwrap());
                if message == WM_HOTKEY {
                    transform_clipboard(None);
                }
            }
        }
    }
}

// ── CLI argument parsing ────────────────────────────────────

fn parse_prompt_flag(args: &[String]) -> Option<&str> {
    for (i, arg) in args.iter().enumerate() {
        if (arg == "--prompt" || arg == "-p") && i + 1 < args.len() {
            return Some(&args[i + 1]);
        }
    }
    None
}

// ── Main ──────────────────────────────────────────────────

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let has = |flag: &str| args.iter().any(|a| a == flag);

    if has("--now") || has("-n") {
        let prompt = parse_prompt_flag(&args);
        std::process::exit(transform_clipboard(prompt));
    }

    match args.get(1).map(|s| s.as_str()) {
        Some("init") => cmd_init(),
        Some("setup") => cmd_setup(),
        Some("learn") => cmd_learn(),
        _ => {
            eprintln!("Quill v{}", env!("CARGO_PKG_VERSION"));
            eprintln!();
            eprintln!("Commands:");
            eprintln!("  quill init             Create default config");
            eprintln!("  quill setup            Install keyboard shortcut");
            eprintln!("  quill --now            Transform clipboard");
            eprintln!("  quill --now -p formal  Transform using prompt profile");
            eprintln!("  quill learn            Update writing style from history");
            eprintln!();
            eprintln!("Config:   {}", config::config_path().display());
            eprintln!("Context:  {}", config::context_path().display());
            eprintln!("Prompts:  {}", config::prompts_dir().display());
            eprintln!("History:  {} ({} entries)", config::history_path().display(), history::count());

            #[cfg(target_os = "windows")]
            {
                eprintln!();
                eprintln!("Starting hotkey daemon...");
                windows_hotkey_loop();
            }
        }
    }
}
