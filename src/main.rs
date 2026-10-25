//! Quill — silent system-wide text transformer. AI rewrite via a configured API or a local
//! Ollama model, with an offline grammar engine (harper-core) as the zero-network fallback.
//!
//! Commands:
//!   quill init          Create default config at ~/.config/quill/config.toml
//!   quill check         Validate the provider that would be used (no network in offline mode)
//!   quill status        Show mode, providers and hotkey state (never sends an AI request)
//!   quill setup         Install keyboard shortcut
//!   quill --now         Transform clipboard (one-shot)
//!   quill --stdin       Transform stdin and print the result to stdout
//!   quill --now -p X    Transform using prompt profile X (AI modes only)
//!   quill learn         Analyze history and update writing context (needs an AI provider)
//!   quill --mode MODE   auto (default) | ai | offline; --offline is an alias for offline
//!   quill --help        Show usage
//!   quill --version     Print the version

#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod modules;

use modules::config::{self, Config, Mode};
use modules::provider::{self, Offline, Provider};
use modules::{claude, clipboard, context, grammar, history, ollama};
use std::io::{Read, Write};

/// Sample used by `quill check` in offline mode to prove the engine works locally.
const OFFLINE_SELF_CHECK: &str = "this is teh test.";

/// Result of one transform through the chosen backend.
struct Transformed {
    text: String,
    /// True when the offline grammar engine produced the text.
    offline: bool,
    /// Number of corrections applied by the offline engine (0 for AI results).
    fixes: usize,
}

// ── Shared helpers ────────────────────────────────────────────

/// Explains why auto mode fell back to offline grammar mode.
fn print_not_configured_hint(note: Option<&str>) {
    eprintln!(
        "[quill] offline grammar mode: no API key configured and no usable Ollama model found."
    );
    eprintln!(
        "[quill] To enable AI rewrite: set [api] key in {} (or QUILL_API_KEY), or start Ollama (ollama serve; ollama pull <model>). Run 'quill status' for details.",
        config::config_path().display()
    );
    if let Some(note) = note {
        eprintln!("[quill] Ollama is reachable but unusable: {note}");
    }
}

/// Validates the prompt profile for AI providers (offline mode ignores profiles).
fn check_profile(cfg: &Config, provider: &Provider, profile: Option<&str>) -> Result<(), String> {
    if matches!(provider, Provider::Offline(_)) {
        return Ok(());
    }
    config::resolve_instruction(cfg, profile).map(|_| ())
}

/// Runs the offline grammar engine. Profiles cannot apply offline, so they are reported as ignored.
fn offline_transform(
    cfg: &Config,
    profile: Option<&str>,
    input: &str,
) -> Result<Transformed, String> {
    if profile.is_some() {
        eprintln!("[quill] warning: -p/--prompt is ignored in offline mode");
    }
    let corrected = grammar::correct(input, cfg.offline.dialect)?;
    Ok(Transformed {
        text: corrected.text,
        offline: true,
        fixes: corrected.fixes,
    })
}

/// Transforms `input` through the chosen provider. In auto mode a failing AI request falls back
/// to the offline engine; in ai mode it is an error. Errors are returned without the `[quill]`
/// prefix, which callers add.
fn run_transform(
    cfg: &Config,
    provider: &Provider,
    profile: Option<&str>,
    input: &str,
) -> Result<Transformed, String> {
    let request_cfg = match provider {
        Provider::Api => cfg.clone(),
        Provider::Ollama { base, model } => ollama::request_config(cfg, base, model),
        Provider::Unconfigured => {
            return Err(
                "error: no API key configured; set [api] key in config.toml or QUILL_API_KEY"
                    .to_string(),
            )
        }
        Provider::Offline(reason) => {
            if let Offline::NotConfigured { ollama_note } = reason {
                print_not_configured_hint(ollama_note.as_deref());
            }
            return offline_transform(cfg, profile, input);
        }
    };

    let instruction = config::resolve_instruction(cfg, profile)?;
    context::ensure_exists();
    let user_context = context::load();
    match claude::transform(&request_cfg, &instruction, &user_context, input) {
        Ok(text) => Ok(Transformed {
            text,
            offline: false,
            fixes: 0,
        }),
        Err(e) if cfg.mode == Mode::Auto => {
            eprintln!("[quill] AI request failed ({e}); falling back to offline grammar mode.");
            offline_transform(cfg, profile, input)
        }
        Err(e) => Err(format!("error: {e}")),
    }
}

// ── One-shot transform ────────────────────────────────────────

fn transform_clipboard(cli_mode: Option<Mode>, profile: Option<&str>) -> i32 {
    let cfg = match config::load(cli_mode) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[quill] {e}");
            return 1;
        }
    };
    let provider = provider::resolve(&cfg);
    if let Err(e) = check_profile(&cfg, &provider, profile) {
        eprintln!("[quill] {e}");
        return 1;
    }

    let text = match clipboard::get_text() {
        Some(t) if !t.trim().is_empty() => t.trim().to_string(),
        _ => {
            eprintln!("[quill] clipboard is empty or has no text");
            return 1;
        }
    };
    if !matches!(provider, Provider::Offline(_)) {
        eprintln!("[quill] processing {} chars...", text.len());
    }

    match run_transform(&cfg, &provider, profile, &text) {
        Ok(done) => {
            let clean = done.text.trim();
            if done.offline {
                // Offline results are never recorded: history is for LLM style learning.
                if done.fixes == 0 {
                    eprintln!("[quill] no changes");
                    return 0;
                }
                if clipboard::set_text(clean) {
                    eprintln!("[quill] done (offline grammar mode)");
                    0
                } else {
                    eprintln!("[quill] failed to write result to clipboard");
                    1
                }
            } else {
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
        }
        Err(e) => {
            eprintln!("[quill] {e}");
            1
        }
    }
}

// ── Stdin: transform piped text, print to stdout ─────────────

fn cmd_stdin(cli_mode: Option<Mode>, profile: Option<&str>) -> i32 {
    let cfg = match config::load(cli_mode) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[quill] {e}");
            return 1;
        }
    };
    let provider = provider::resolve(&cfg);
    if let Err(e) = check_profile(&cfg, &provider, profile) {
        eprintln!("[quill] {e}");
        return 1;
    }

    let mut input = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut input) {
        eprintln!("[quill] failed to read stdin as UTF-8 text: {e}");
        return 1;
    }
    if input.trim().is_empty() {
        eprintln!("[quill] no input: pipe text into `quill --stdin`");
        return 1;
    }

    match run_transform(&cfg, &provider, profile, &input) {
        Ok(done) => {
            let mut out = std::io::stdout();
            let written = writeln!(out, "{}", done.text.trim()).and_then(|()| out.flush());
            if written.is_err() {
                eprintln!("[quill] failed to write the result to stdout");
                return 1;
            }
            if done.offline && done.fixes == 0 {
                eprintln!("[quill] no changes");
            }
            0
        }
        Err(e) => {
            eprintln!("[quill] {e}");
            1
        }
    }
}

// ── Check: validate the active provider ───────────────────────

/// Offline self-check: runs a tiny local correction. No network.
fn offline_self_check(cfg: &Config) -> i32 {
    match grammar::correct(OFFLINE_SELF_CHECK, cfg.offline.dialect) {
        Ok(corrected) if corrected.text.contains("the test") => {
            println!("OK: offline grammar engine available");
            0
        }
        Ok(_) => {
            eprintln!("[quill] check failed: offline grammar engine gave an unexpected result");
            1
        }
        Err(e) => {
            eprintln!("[quill] check failed: {e}");
            1
        }
    }
}

fn cmd_check(cli_mode: Option<Mode>) -> i32 {
    let cfg = match config::load(cli_mode) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[quill] {e}");
            return 1;
        }
    };

    let outcome = match provider::resolve(&cfg) {
        Provider::Api => claude::check(&cfg, claude::API_CHECK_MAX_TOKENS),
        Provider::Ollama { base, model } => {
            let request = ollama::request_config(&cfg, &base, &model);
            claude::check(&request, claude::LOCAL_CHECK_MAX_TOKENS)
        }
        Provider::Offline(reason) => {
            if let Offline::NotConfigured { ollama_note } = &reason {
                print_not_configured_hint(ollama_note.as_deref());
            }
            return offline_self_check(&cfg);
        }
        Provider::Unconfigured => Err("API not configured".to_string()),
    };

    match outcome {
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

// ── Status: report configuration without sending AI requests ──

fn cmd_status(cli_mode: Option<Mode>) -> i32 {
    let path = config::config_path();
    let found = if path.exists() { "found" } else { "not found" };
    println!("Quill v{} status", env!("CARGO_PKG_VERSION"));
    match config::inspect(cli_mode) {
        Ok(cfg) => print_status(&cfg, &path.display().to_string(), found),
        Err(e) => {
            println!("mode: unknown (config error)");
            println!("config: {} ({found})", path.display());
            println!("api: unknown (config error)");
            println!("ollama: skipped (config error)");
            println!("offline: available (harper-core)");
            println!("active: unknown ({e})");
        }
    }
    print_hotkey_status();
    0
}

fn print_status(cfg: &Config, config_path: &str, found: &str) {
    println!(
        "mode: {} (source: {})",
        cfg.mode.as_str(),
        cfg.mode_source.as_str()
    );
    println!("config: {config_path} ({found})");
    match &cfg.api {
        Some(api) => {
            let origin = if config::env_api_key_set() {
                "QUILL_API_KEY"
            } else {
                "config.toml"
            };
            println!("api: configured (model {}, key from {origin})", api.model);
        }
        None => {
            println!("api: not configured (set [api] key in {config_path} or export QUILL_API_KEY)")
        }
    }

    // Exactly one Ollama probe; explicit offline mode makes no network request at all.
    let probe = if cfg.mode == Mode::Offline {
        None
    } else {
        Some(ollama::probe(&cfg.ollama))
    };
    print_ollama_line(cfg, probe.as_ref());

    let dialect = format!("{:?}", cfg.offline.dialect).to_lowercase();
    println!("offline: available (harper-core, dialect {dialect})");
    let active = provider::pick(cfg, probe.as_ref());
    println!("active: {}", provider::describe(&active));
}

fn print_ollama_line(cfg: &Config, probe: Option<&ollama::Probe>) {
    let url = &cfg.ollama.url;
    let Some(probe) = probe else {
        println!("ollama: skipped (mode offline)");
        return;
    };
    match &probe.selection {
        ollama::Selection::Disabled => println!("ollama: disabled (enabled = false in [ollama])"),
        ollama::Selection::Unreachable => println!("ollama: not reachable at {url}"),
        ollama::Selection::Unusable(reason) => {
            println!("ollama: reachable at {url} but no usable model: {reason}")
        }
        ollama::Selection::Usable(model) => println!(
            "ollama: reachable at {url}; models: {}; selected: {model}",
            probe.tags.join(", ")
        ),
    }
}

#[cfg(target_os = "macos")]
fn print_hotkey_status() {
    let (installed, block) = modules::setup::hotkey_status();
    println!(
        "hotkey: skhd {}; quill block in ~/.skhdrc {}",
        if installed {
            "installed"
        } else {
            "not installed"
        },
        if block { "present" } else { "absent" }
    );
}

#[cfg(not(target_os = "macos"))]
fn print_hotkey_status() {}

// ── Init: create default config ───────────────────────────────

fn cmd_init() -> i32 {
    match config::write_default() {
        Ok(path) => {
            eprintln!("Config created at: {}", path.display());
            eprintln!("Prompts directory: {}", config::prompts_dir().display());
            eprintln!();
            eprintln!("Next steps:");
            eprintln!("  1. Optional: set [api] key (or QUILL_API_KEY) for AI rewrite; without it Quill uses Ollama or the offline grammar engine");
            eprintln!("  2. Run `quill status` to see which provider will be used");
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

fn cmd_learn(cli_mode: Option<Mode>) -> i32 {
    let cfg = match config::load(cli_mode) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[quill] {e}");
            return 1;
        }
    };

    // Resolve the provider before touching history: learning needs an AI model.
    let request_cfg = match provider::resolve(&cfg) {
        Provider::Api => cfg.clone(),
        Provider::Ollama { base, model } => ollama::request_config(&cfg, &base, &model),
        Provider::Offline(Offline::Requested) => {
            eprintln!("[quill] learn needs an AI provider and cannot run in offline mode");
            return 1;
        }
        Provider::Offline(Offline::NotConfigured { ollama_note }) => {
            eprintln!(
                "[quill] learn needs an AI provider, but no API key is configured and no usable Ollama model was found. Run `quill status` for details."
            );
            if let Some(note) = ollama_note {
                eprintln!("[quill] Ollama is reachable but unusable: {note}");
            }
            return 1;
        }
        Provider::Unconfigured => {
            eprintln!("[quill] learn needs an AI provider: no API key is configured");
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

    // Build history text for the model
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

    match claude::analyze_style(&request_cfg, &history_text, &existing_context) {
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
    #[cfg_attr(not(target_os = "macos"), allow(unused_variables))]
    let cfg = match config::load(None) {
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
fn setup_windows(cfg: &Config) {
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
                    transform_clipboard(None, None);
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
    Status,
    Setup,
    Learn,
    Now,
    Stdin,
}

struct Cli {
    command: Command,
    prompt: Option<String>,
    mode: Option<Mode>,
}

/// Records a mode flag; `--mode` and `--offline` must not disagree.
fn set_mode(slot: &mut Option<Mode>, value: Mode) -> Result<(), String> {
    match slot {
        Some(previous) if *previous != value => {
            Err("--mode and --offline conflict; give only one mode".to_string())
        }
        _ => {
            *slot = Some(value);
            Ok(())
        }
    }
}

/// Parses the arguments. A value that follows -p/--prompt or --mode is consumed as that
/// option's value and is never treated as a command.
fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut command: Option<Command> = None;
    let mut prompt: Option<String> = None;
    let mut mode: Option<Mode> = None;
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
            "--mode" => {
                let value = iter
                    .next()
                    .ok_or_else(|| "--mode needs a value: auto, ai or offline".to_string())?;
                let parsed = Mode::parse(value).ok_or_else(|| {
                    "invalid mode for --mode: expected auto, ai or offline".to_string()
                })?;
                set_mode(&mut mode, parsed)?;
                continue;
            }
            "--offline" => {
                set_mode(&mut mode, Mode::Offline)?;
                continue;
            }
            "init" => Command::Init,
            "check" => Command::Check,
            "status" => Command::Status,
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
        mode,
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
    println!("  quill check            Validate the provider that would be used");
    println!(
        "  quill status           Show mode, providers and hotkey state (sends no AI request)"
    );
    println!("  quill setup            Install keyboard shortcut");
    println!("  quill --now            Transform clipboard");
    println!("  quill --stdin          Transform stdin and print the result to stdout");
    println!("  quill --now -p formal  Transform using prompt profile (AI modes; also works with --stdin)");
    println!("  quill learn            Update writing style from history (needs an AI provider)");
    println!("  quill --version        Print the version");
    println!("  quill --help           Show this help");
    println!();
    println!(
        "Modes (--mode auto|ai|offline, or --offline; QUILL_MODE and [general] mode also work):"
    );
    println!("  auto     configured API, else local Ollama, else offline grammar (default)");
    println!("  ai       configured API only; errors are not retried offline");
    println!("  offline  grammar, punctuation and spelling fixes only; no network");
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
        Command::Now => transform_clipboard(cli.mode, cli.prompt.as_deref()),
        Command::Stdin => cmd_stdin(cli.mode, cli.prompt.as_deref()),
        Command::Check => cmd_check(cli.mode),
        Command::Status => cmd_status(cli.mode),
        Command::Init => cmd_init(),
        Command::Setup => cmd_setup(),
        Command::Learn => cmd_learn(cli.mode),
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
