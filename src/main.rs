//! Quill — silent system-wide text transformer powered by Claude.
//!
//! Commands:
//!   quill init          Create default config at ~/.config/quill/config.toml
//!   quill check         Validate the config and send one test request
//!   quill setup         Install keyboard shortcut
//!   quill --now         Transform clipboard (one-shot)
//!   quill --stdin       Transform stdin and print the result to stdout
//!   quill --now -p X    Transform using prompt profile X
//!   quill learn         Analyze history and update writing context
//!   quill --help        Show usage
//!   quill --version     Print the version

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod modules;

use modules::{claude, clipboard, config, context, history};
use std::io::{Read, Write};

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

// ── Stdin: transform piped text, print to stdout ─────────────

fn cmd_stdin(prompt_name: Option<&str>) -> i32 {
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

    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("[quill] failed to read stdin as UTF-8 text: {e}");
        return 1;
    }
    if input.trim().is_empty() {
        eprintln!("[quill] no input: pipe text into `quill --stdin`");
        return 1;
    }

    // Load writing style context
    context::ensure_exists();
    let user_context = context::load();

    match claude::transform(&cfg, &instruction, &user_context, &input) {
        Ok(result) => {
            let mut out = std::io::stdout();
            let written = writeln!(out, "{}", result.trim()).and_then(|()| out.flush());
            if written.is_err() {
                eprintln!("[quill] failed to write the result to stdout");
                return 1;
            }
            0
        }
        Err(e) => {
            eprintln!("[quill] error: {e}");
            1
        }
    }
}

// ── Check: validate config and send one test request ─────────

fn cmd_check() -> i32 {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[quill] {e}");
            return 1;
        }
    };

    match claude::check(&cfg) {
        Ok(model) => {
            println!("OK: {model} responded");
            0
        }
        Err(e) => {
            eprintln!("[quill] check failed: {e}");
            1
        }
    }
}

// ── Init: create default config ───────────────────────────────

fn cmd_init() -> i32 {
    match config::write_default() {
        Ok(path) => {
            eprintln!("Config created at: {}", path.display());
            eprintln!("Prompts directory: {}", config::prompts_dir().display());
            eprintln!();
            eprintln!("Next steps:");
            eprintln!("  1. Edit the config and set your API url and key (or use QUILL_API_URL/QUILL_API_KEY)");
            eprintln!("  2. Run `quill check` to verify the connection");
            eprintln!("  3. Run `quill setup` to install the keyboard shortcut");
            0
        }
        Err(e) => {
            eprintln!("[quill] {e}");
            1
        }
    }
}

// ── Learn: analyze history and update context ────────────────

fn cmd_learn() -> i32 {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[quill] {e}");
            return 1;
        }
    };

    let entries = history::load_all();
    if entries.is_empty() {
        eprintln!("[quill] no transformation history found.");
        eprintln!("Use quill a few times first, then run `quill learn`.");
        return 1;
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
        Ok(new_context) => match context::save(&new_context) {
            Ok(()) => {
                eprintln!(
                    "[quill] context updated at: {}",
                    config::context_path().display()
                );
                eprintln!();
                eprintln!("{new_context}");
                // Clear processed history
                history::clear();
                eprintln!();
                eprintln!("History cleared. Future transforms will use these preferences.");
                0
            }
            Err(e) => {
                eprintln!("[quill] {e}");
                1
            }
        },
        Err(e) => {
            eprintln!("[quill] failed to analyze: {e}");
            1
        }
    }
}

// ── Setup: install keyboard shortcut ──────────────────────────

fn cmd_setup() -> i32 {
    let cfg = match config::load() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[quill] {e}");
            return 1;
        }
    };

    #[cfg(target_os = "macos")]
    {
        if let Err(e) = modules::setup::setup_macos(&cfg) {
            eprintln!("[quill] {e}");
            return 1;
        }
    }

    #[cfg(target_os = "windows")]
    setup_windows(&cfg);

    #[cfg(target_os = "linux")]
    eprintln!("Linux: bind your DE shortcut to run `quill --now`");
    0
}

#[cfg(target_os = "windows")]
fn setup_windows(cfg: &config::Config) {
    eprintln!("Windows shortcut: {}", cfg.shortcut.windows);
    eprintln!("Run `quill` (no flags) to start the background hotkey daemon.");
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

#[derive(Debug, PartialEq, Eq)]
enum Command {
    /// No arguments at all (Windows: also starts the hotkey daemon).
    NoArgs,
    Usage,
    Version,
    Init,
    Check,
    Setup,
    Learn,
    Now,
    Stdin,
}

struct Cli {
    command: Command,
    prompt: Option<String>,
}

/// Parses the arguments. A value that follows -p/--prompt is consumed as the
/// profile name and is never treated as a command.
fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut command: Option<Command> = None;
    let mut prompt: Option<String> = None;
    let mut iter = args.iter().skip(1);
    while let Some(arg) = iter.next() {
        let found = match arg.as_str() {
            "-p" | "--prompt" => {
                let value = iter
                    .next()
                    .ok_or_else(|| format!("{arg} needs a profile name"))?;
                prompt = Some(value.clone());
                continue;
            }
            "init" => Command::Init,
            "check" => Command::Check,
            "setup" => Command::Setup,
            "learn" => Command::Learn,
            "--now" | "-n" => Command::Now,
            "--stdin" => Command::Stdin,
            "--help" | "-h" | "help" => Command::Usage,
            "--version" | "-V" => Command::Version,
            other => return Err(format!("unknown argument: {other}")),
        };
        if command.is_some() {
            return Err("only one command can be given at a time".to_string());
        }
        command = Some(found);
    }
    Ok(Cli {
        command: command.unwrap_or(Command::NoArgs),
        prompt,
    })
}

// ── Main ──────────────────────────────────────────────────

fn print_usage() {
    println!("Quill v{}", env!("CARGO_PKG_VERSION"));
    println!();
    println!("Commands:");
    println!(
        "  quill init             Create default config (uses QUILL_API_URL/QUILL_API_KEY if set)"
    );
    println!("  quill check            Validate the config and send one test request");
    println!("  quill setup            Install keyboard shortcut");
    println!("  quill --now            Transform clipboard");
    println!("  quill --stdin          Transform stdin and print the result to stdout");
    println!("  quill --now -p formal  Transform using prompt profile (also works with --stdin)");
    println!("  quill learn            Update writing style from history");
    println!("  quill --version        Print the version");
    println!("  quill --help           Show this help");
    println!();
    println!("Config:   {}", config::config_path().display());
    println!("Context:  {}", config::context_path().display());
    println!("Prompts:  {}", config::prompts_dir().display());
    println!(
        "History:  {} ({} entries)",
        config::history_path().display(),
        history::count()
    );
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let cli = match parse_args(&args) {
        Ok(cli) => cli,
        Err(e) => {
            eprintln!("[quill] {e}");
            eprintln!("Run `quill --help` for usage.");
            std::process::exit(1);
        }
    };

    let code = match cli.command {
        Command::Now => transform_clipboard(cli.prompt.as_deref()),
        Command::Stdin => cmd_stdin(cli.prompt.as_deref()),
        Command::Check => cmd_check(),
        Command::Init => cmd_init(),
        Command::Setup => cmd_setup(),
        Command::Learn => cmd_learn(),
        Command::Usage => {
            print_usage();
            0
        }
        Command::Version => {
            println!("quill {}", env!("CARGO_PKG_VERSION"));
            0
        }
        Command::NoArgs => {
            print_usage();
            #[cfg(target_os = "windows")]
            {
                eprintln!();
                eprintln!("Starting hotkey daemon...");
                windows_hotkey_loop();
            }
            0
        }
    };
    std::process::exit(code);
}
