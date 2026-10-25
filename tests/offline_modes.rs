//! Acceptance tests for quill's provider selection, offline grammar mode and `status`.
//! Spec: .ram-status/quill-modes-20251025-2255/DESIGN.md (Rev 4), sections A1-A4.
//!
//! Sealed harness (same idea as tests/cli.rs): cleared environment, per-test HOME, PATH holding
//! only fail-fast sentinels, QUILL_OLLAMA_URL pointing at a closed port unless a test overrides it,
//! and only 127.0.0.1 mock servers. Nothing here touches the real clipboard, skhd, Ollama or config.
#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const KEY: &str = "test-key-123";
const DEAD_OLLAMA: &str = "http://127.0.0.1:1";
static COUNTER: AtomicU64 = AtomicU64::new(0);

// ───────────────────────── mock server ─────────────────────────

#[derive(Clone)]
struct Route {
    path: &'static str,
    status: u16,
    body: String,
    hang: bool,
}

#[derive(Clone, Debug)]
struct Req {
    path: String,
    headers: Vec<(String, String)>,
    body: String,
}

impl Req {
    fn header(&self, name: &str) -> Option<String> {
        let n = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == n)
            .map(|(_, v)| v.clone())
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).expect("request body is JSON")
    }
}

struct Mock {
    base: String,
    reqs: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
    th: Option<thread::JoinHandle<()>>,
}

fn read_request(s: &mut TcpStream) -> Option<Req> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end;
    loop {
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            header_end = pos + 4;
            break;
        }
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => return None,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let mut lines = head.lines();
    let first = lines.next().unwrap_or("");
    let path = first
        .split_whitespace()
        .nth(1)
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/")
        .to_string();
    let mut headers = Vec::new();
    let mut clen = 0usize;
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().to_string();
            if k == "content-length" {
                clen = v.parse().unwrap_or(0);
            }
            headers.push((k, v));
        }
    }
    while buf.len() < header_end + clen {
        match s.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => buf.extend_from_slice(&chunk[..n]),
        }
    }
    Some(Req {
        path,
        headers,
        body: String::from_utf8_lossy(&buf[header_end..]).to_string(),
    })
}

impl Mock {
    fn start(routes: Vec<Route>) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let reqs = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (r2, s2) = (reqs.clone(), stop.clone());
        let th = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(90);
            while !s2.load(Ordering::SeqCst) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
                        if let Some(req) = read_request(&mut s) {
                            let route = routes.iter().find(|r| r.path == req.path).cloned();
                            r2.lock().unwrap().push(req);
                            match route {
                                Some(r) if r.hang => {
                                    let until = Instant::now() + Duration::from_secs(10);
                                    while !s2.load(Ordering::SeqCst) && Instant::now() < until {
                                        thread::sleep(Duration::from_millis(20));
                                    }
                                }
                                Some(r) => {
                                    let resp = format!(
                                        "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                        r.status,
                                        r.body.len(),
                                        r.body
                                    );
                                    let _ = s.write_all(resp.as_bytes());
                                }
                                None => {
                                    let _ = s.write_all(b"HTTP/1.1 404 X\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
                                }
                            }
                        }
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(_) => break,
                }
            }
        });
        Mock {
            base: format!("http://127.0.0.1:{port}"),
            reqs,
            stop,
            th: Some(th),
        }
    }
    /// Anthropic-style API: only /v1/messages.
    fn api(text: &str) -> Mock {
        Mock::start(vec![msg_route(200, text)])
    }
    fn api_status(status: u16) -> Mock {
        Mock::start(vec![Route {
            path: "/v1/messages",
            status,
            body: "{\"error\":\"nope\"}".into(),
            hang: false,
        }])
    }
    fn api_hang() -> Mock {
        Mock::start(vec![Route {
            path: "/v1/messages",
            status: 200,
            body: String::new(),
            hang: true,
        }])
    }
    /// Ollama-style server: /api/tags, /api/ps, /v1/messages.
    fn ollama(tags: &[&str], ps: &[&str], reply: &str) -> Mock {
        Mock::start(vec![
            Route {
                path: "/api/tags",
                status: 200,
                body: models_json(tags),
                hang: false,
            },
            Route {
                path: "/api/ps",
                status: 200,
                body: models_json(ps),
                hang: false,
            },
            msg_route(200, reply),
        ])
    }
    fn messages_url(&self) -> String {
        format!("{}/v1/messages", self.base)
    }
    fn count(&self) -> usize {
        self.reqs.lock().unwrap().len()
    }
    fn count_path(&self, p: &str) -> usize {
        self.reqs
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == p)
            .count()
    }
    fn last_msg(&self) -> Req {
        self.reqs
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|r| r.path == "/v1/messages")
            .cloned()
            .expect("a /v1/messages request")
    }
}

impl Drop for Mock {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.th.take() {
            let _ = t.join();
        }
    }
}

fn models_json(names: &[&str]) -> String {
    let models: Vec<Value> = names
        .iter()
        .map(|n| serde_json::json!({"name": n, "model": n}))
        .collect();
    serde_json::json!({ "models": models }).to_string()
}

fn msg_route(status: u16, text: &str) -> Route {
    Route {
        path: "/v1/messages",
        status,
        body: serde_json::json!({"content": [{"type": "text", "text": text}]}).to_string(),
        hang: false,
    }
}

// ───────────────────────── sealed environment ─────────────────────────

const SENTINELS: &[&str] = &[
    "pbpaste",
    "pbcopy",
    "skhd",
    "osascript",
    "xclip",
    "xsel",
    "wl-paste",
    "wl-copy",
    "powershell",
    "ollama",
];

struct Env {
    root: PathBuf,
    home: PathBuf,
    bin: PathBuf,
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
    secs: f64,
}

impl Env {
    fn new() -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("quill-om-{}-{}", std::process::id(), n));
        let home = root.join("home");
        let bin = root.join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let env = Env { root, home, bin };
        for name in SENTINELS {
            let log = env.root.join("sentinel.log");
            env.install_fake(
                name,
                &format!("#!/bin/sh\necho {} >> '{}'\nexit 99\n", name, log.display()),
            );
        }
        env
    }
    fn conf(&self) -> PathBuf {
        self.home.join(".config").join("quill")
    }
    fn install_fake(&self, name: &str, script: &str) {
        let p = self.bin.join(name);
        fs::write(&p, script).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
    }
    fn write_conf(&self, rel: &str, content: &str) -> PathBuf {
        let p = self.conf().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
        p
    }
    fn cfg_api(&self, url: &str) {
        self.write_conf(
            "config.toml",
            &format!("[api]\nurl = \"{url}\"\nkey = \"{KEY}\"\n"),
        );
    }
    fn run(&self, args: &[&str], stdin: Option<&str>, extra_env: &[(&str, &str)]) -> Out {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let out_p = self.root.join(format!("out{n}.txt"));
        let err_p = self.root.join(format!("err{n}.txt"));
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_quill"));
        cmd.args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("PATH", &self.bin)
            .env("QUILL_OLLAMA_URL", DEAD_OLLAMA)
            .stdin(Stdio::piped())
            .stdout(fs::File::create(&out_p).unwrap())
            .stderr(fs::File::create(&err_p).unwrap());
        for (k, v) in extra_env {
            cmd.env(k, v);
        }
        let started = Instant::now();
        let mut child = cmd.spawn().expect("spawn quill");
        {
            let mut si = child.stdin.take().unwrap();
            if let Some(s) = stdin {
                let _ = si.write_all(s.as_bytes());
            }
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        let status = loop {
            if let Some(st) = child.try_wait().unwrap() {
                break st;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("quill did not exit within 60s: {:?}", args);
            }
            thread::sleep(Duration::from_millis(10));
        };
        Out {
            code: status.code().unwrap_or(-1),
            stdout: fs::read_to_string(&out_p).unwrap_or_default(),
            stderr: fs::read_to_string(&err_p).unwrap_or_default(),
            secs: started.elapsed().as_secs_f64(),
        }
    }
    fn no_sentinel(&self) {
        let hits = fs::read_to_string(self.root.join("sentinel.log")).unwrap_or_default();
        assert_eq!(hits, "", "a real-tool sentinel was invoked");
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn line_with<'a>(text: &'a str, prefix: &str) -> &'a str {
    text.lines()
        .find(|l| l.trim_start().starts_with(prefix))
        .unwrap_or_else(|| panic!("no line starting with {prefix:?} in:\n{text}"))
}

const SAMPLE: &str = "this is an test  of the grammer engine,it dont work good . i has two cat";

// ═════════════════════ A1: offline grammar engine ═════════════════════

#[test]
fn o01_basic_fixes_and_no_network() {
    let env = Env::new();
    let api = Mock::api("SHOULD NOT BE USED");
    let oll = Mock::ollama(&["qwen3:30b"], &[], "SHOULD NOT BE USED");
    env.cfg_api(&api.messages_url());
    let out = env.run(
        &["--stdin", "--mode", "offline"],
        Some(SAMPLE),
        &[("QUILL_OLLAMA_URL", oll.base.as_str())],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout
            .contains("This is a test of the grammar engine, it"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains(". I have"), "{}", out.stdout);
    assert!(
        !out.stdout.contains("  "),
        "double space left: {}",
        out.stdout
    );
    assert_eq!(
        api.count() + oll.count(),
        0,
        "offline mode must not use the network"
    );
    env.no_sentinel();
}

#[test]
fn o01b_offline_flag_is_alias_and_works_without_any_config() {
    let env = Env::new();
    let out = env.run(&["--stdin", "--offline"], Some("this is teh test."), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("the test"), "{}", out.stdout);
    assert!(
        !env.conf().join("config.toml").exists(),
        "offline must not create config"
    );
}

#[test]
fn o02_already_correct_text_is_unchanged() {
    let env = Env::new();
    let out = env.run(
        &["--stdin", "--mode", "offline"],
        Some("This is a perfectly fine sentence.\n"),
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "This is a perfectly fine sentence.\n");
    assert!(out.stderr.contains("no changes"), "{}", out.stderr);
}

#[test]
fn o03_crlf_tabs_and_blank_lines_are_preserved() {
    let env = Env::new();
    let input =
        "First line is fine.\r\n\r\nsecond line has a typo teh word.\r\n\tTabbed line is fine.";
    let out = env.run(&["--stdin", "--mode", "offline"], Some(input), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("First line is fine.\r\n\r\n"),
        "{:?}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("typo the word.\r\n\tTabbed line is fine."),
        "{:?}",
        out.stdout
    );
}

#[test]
fn o04_unicode_and_emoji_survive() {
    let env = Env::new();
    let out = env.run(
        &["--stdin", "--mode", "offline"],
        Some("café is nice \u{1F642} and teh end"),
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("café is nice \u{1F642} and the end")
            || out.stdout.contains("Café is nice \u{1F642} and the end"),
        "{}",
        out.stdout
    );
}

#[test]
fn o05_empty_input_is_rejected() {
    let env = Env::new();
    let out = env.run(&["--stdin", "--mode", "offline"], Some("  \n"), &[]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("no input"), "{}", out.stderr);
    assert_eq!(out.stdout, "");
}

#[test]
fn o06_oversized_input_is_rejected_cleanly() {
    let env = Env::new();
    let big = "word ".repeat(20_001); // 100_005 chars
    let out = env.run(&["--stdin", "--mode", "offline"], Some(&big), &[]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("too long"), "{}", out.stderr);
    assert_eq!(out.stdout, "");
}

#[test]
fn o07_protected_spans_are_never_modified() {
    let env = Env::new();
    let input = "check https://example.com/teh_path?x=1, and mail bob@exmple.com then run `teh --flgs` and Quill, snake_case_wrd, CamelCaseWrd, foo.rs plus teh typo.";
    let out = env.run(&["--stdin", "--mode", "offline"], Some(input), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    for protected in [
        "https://example.com/teh_path?x=1",
        "bob@exmple.com",
        "`teh --flgs`",
        "Quill,",
        "snake_case_wrd",
        "CamelCaseWrd",
        "foo.rs",
    ] {
        assert!(
            out.stdout.contains(protected),
            "lost {protected:?} in: {}",
            out.stdout
        );
    }
    assert!(out.stdout.contains("plus the typo."), "{}", out.stdout);
}

#[test]
fn o07b_unknown_capitalized_names_and_fenced_code_are_untouched() {
    let env = Env::new();
    let out = env.run(
        &["--stdin", "--mode", "offline"],
        Some("Please ask Zyxwvu about teh report."),
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("Zyxwvu about the report"),
        "{}",
        out.stdout
    );

    let fenced = "Intro text is fine.\n\n```\nlet x = teh;\n```\n\nand here teh typo.";
    let out = env.run(&["--stdin", "--mode", "offline"], Some(fenced), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("```\nlet x = teh;\n```"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("here the typo"), "{}", out.stdout);
}

#[test]
fn o07c_length_changing_edits_before_protected_spans_do_not_shift_protection() {
    let env = Env::new();
    // Several length-changing fixes (capital I, "has"->"have", double spaces) precede each protected span.
    let input = "i  has a  teh problem with https://a.example/x_y and  snake_case_id and `code  here`.  i  has  teh  other  with  https://b.example/q_r  and  `x  y`.";
    let out = env.run(&["--stdin", "--mode", "offline"], Some(input), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    for protected in [
        "https://a.example/x_y",
        "snake_case_id",
        "`code  here`",
        "https://b.example/q_r",
        "`x  y`",
    ] {
        assert!(
            out.stdout.contains(protected),
            "lost {protected:?} in: {}",
            out.stdout
        );
    }
    assert!(out.stdout.contains("I have"), "{}", out.stdout);
}

#[test]
fn o07f_meaning_changing_rewrites_are_never_applied() {
    let env = Env::new();
    // negations and contractions without apostrophes must never flip or split
    let out = env.run(
        &["--stdin", "--mode", "offline"],
        Some("I wont go. We werent there. They arent ready. He cant stay."),
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let lower = out.stdout.to_lowercase();
    for bad in ["want", "rent", "a rent", "can stay", "we are"] {
        assert!(
            !lower.contains(bad),
            "unexpected {bad:?} in: {}",
            out.stdout
        );
    }
    // each original word survives (an apostrophe may have been added: won't, can't, ...)
    assert!(
        lower.contains("won") && lower.contains("go"),
        "{}",
        out.stdout
    );
    assert!(
        lower.contains("there") && lower.contains("ready") && lower.contains("stay"),
        "{}",
        out.stdout
    );

    // abbreviations and slang are not expanded
    let out = env.run(
        &["--stdin", "--mode", "offline"],
        Some("imo this is fine, idk, tbh, brb, ttyl."),
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    for kept in ["imo", "idk", "tbh", "brb", "ttyl"] {
        assert!(out.stdout.contains(kept), "lost {kept:?}: {}", out.stdout);
    }
}

#[test]
fn o07e_technical_words_are_not_turned_into_other_words() {
    let env = Env::new();
    let input = "please run sudo nginx yaml kubectl docker postgres terraform on tmrw, lol, then fix teh grammer.";
    let out = env.run(&["--stdin", "--mode", "offline"], Some(input), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    for kept in [
        "sudo",
        "nginx",
        "yaml",
        "kubectl",
        "docker",
        "postgres",
        "terraform",
        "tmrw",
        "lol",
    ] {
        assert!(out.stdout.contains(kept), "lost {kept:?}: {}", out.stdout);
    }
    assert!(out.stdout.contains("fix the grammar"), "{}", out.stdout);
}

#[test]
fn o07d_short_slang_tokens_are_not_replaced() {
    let env = Env::new();
    let out = env.run(
        &["--stdin", "--mode", "offline"],
        Some("Please send me teh file asap, thx. Then ok the plan, pls."),
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("thx"), "{}", out.stdout);
    assert!(out.stdout.contains("pls"), "{}", out.stdout);
    assert!(out.stdout.contains("send me the file"), "{}", out.stdout);
}

#[test]
fn o08_offline_correction_is_idempotent() {
    let env = Env::new();
    let first = env.run(&["--stdin", "--mode", "offline"], Some(SAMPLE), &[]);
    assert_eq!(first.code, 0, "{}", first.stderr);
    let second = env.run(&["--stdin", "--mode", "offline"], Some(&first.stdout), &[]);
    assert_eq!(second.code, 0, "{}", second.stderr);
    assert_eq!(first.stdout, second.stdout);
}

#[test]
fn o09_profile_is_ignored_with_warning_in_offline() {
    let env = Env::new();
    let out = env.run(
        &["--stdin", "--mode", "offline", "-p", "formal"],
        Some("this is teh test."),
        &[],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.contains("ignored"), "{}", out.stderr);
}

#[test]
fn o10_invalid_dialect_is_a_config_error() {
    let env = Env::new();
    env.write_conf("config.toml", "[offline]\ndialect = \"klingon\"\n");
    let out = env.run(
        &["--stdin", "--mode", "offline"],
        Some("hello teh world"),
        &[],
    );
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("offline.dialect"), "{}", out.stderr);
}

#[cfg(target_os = "macos")]
#[test]
fn o11_now_offline_replaces_clipboard_and_skips_history() {
    let env = Env::new();
    let cin = env.root.join("clip_in.txt");
    let cout = env.root.join("clip_out.txt");
    fs::write(&cin, "this is teh test of the clipboard.").unwrap();
    env.install_fake(
        "pbpaste",
        &format!("#!/bin/sh\n/bin/cat '{}'\n", cin.display()),
    );
    env.install_fake(
        "pbcopy",
        &format!("#!/bin/sh\n/bin/cat > '{}'\n", cout.display()),
    );
    let out = env.run(&["--now", "--mode", "offline"], None, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        fs::read_to_string(&cout).unwrap().trim(),
        "This is the test of the clipboard."
    );
    assert!(
        !env.conf().join("history.jsonl").exists()
            || fs::read_to_string(env.conf().join("history.jsonl"))
                .unwrap()
                .is_empty()
    );
}

// ═════════════════════ A2/A3: provider resolution ═════════════════════

#[test]
fn m01_configured_api_wins_over_running_ollama() {
    let env = Env::new();
    let api = Mock::api("AI REWRITE");
    let oll = Mock::ollama(&["qwen3:30b"], &[], "OLLAMA REWRITE");
    env.cfg_api(&api.messages_url());
    let out = env.run(
        &["--stdin"],
        Some("hello there"),
        &[("QUILL_OLLAMA_URL", oll.base.as_str())],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout.trim(), "AI REWRITE");
    assert_eq!(api.count(), 1);
    assert_eq!(oll.count_path("/v1/messages"), 0);
    assert!(
        !out.stderr.contains("offline grammar mode"),
        "{}",
        out.stderr
    );
}

#[test]
fn m02_ollama_is_used_when_no_api_key() {
    let env = Env::new();
    let oll = Mock::ollama(&["qwen3:30b"], &[], "OLLAMA REWRITE");
    let out = env.run(
        &["--stdin"],
        Some("hello there"),
        &[("QUILL_OLLAMA_URL", oll.base.as_str())],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout.trim(), "OLLAMA REWRITE");
    let r = oll.last_msg();
    assert_eq!(r.json()["model"], "qwen3:30b");
    assert_eq!(r.header("x-api-key").as_deref(), Some("ollama"));
    assert!(
        !out.stderr.contains("offline grammar mode"),
        "{}",
        out.stderr
    );
}

#[test]
fn m03_running_non_embedding_model_is_preferred_then_first_non_embedding() {
    let env = Env::new();
    let tags = ["embeddinggemma-2:latest", "llama3:8b", "qwen3:30b"];
    let oll = Mock::ollama(&tags, &["qwen3:30b"], "R");
    let out = env.run(
        &["--stdin"],
        Some("hi"),
        &[("QUILL_OLLAMA_URL", oll.base.as_str())],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(oll.last_msg().json()["model"], "qwen3:30b");

    let oll2 = Mock::ollama(&tags, &[], "R");
    let out = env.run(
        &["--stdin"],
        Some("hi"),
        &[("QUILL_OLLAMA_URL", oll2.base.as_str())],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(oll2.last_msg().json()["model"], "llama3:8b");

    // env override of the model, present in tags
    let oll3 = Mock::ollama(&tags, &[], "R");
    let out = env.run(
        &["--stdin"],
        Some("hi"),
        &[
            ("QUILL_OLLAMA_URL", oll3.base.as_str()),
            ("QUILL_OLLAMA_MODEL", "qwen3:30b"),
        ],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(oll3.last_msg().json()["model"], "qwen3:30b");
}

#[test]
fn m04_embedding_only_ollama_falls_back_to_offline_with_hint() {
    let env = Env::new();
    let oll = Mock::ollama(&["embeddinggemma-2:latest"], &[], "NO");
    let out = env.run(
        &["--stdin"],
        Some("this is teh test."),
        &[("QUILL_OLLAMA_URL", oll.base.as_str())],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("the test"), "{}", out.stdout);
    assert_eq!(oll.count_path("/v1/messages"), 0);
    assert!(
        out.stderr.contains("offline grammar mode"),
        "{}",
        out.stderr
    );
}

#[test]
fn m05_configured_model_missing_means_offline_and_names_the_model() {
    let env = Env::new();
    let oll = Mock::ollama(&["qwen3:30b"], &[], "NO");
    let out = env.run(
        &["--stdin"],
        Some("this is teh test."),
        &[
            ("QUILL_OLLAMA_URL", oll.base.as_str()),
            ("QUILL_OLLAMA_MODEL", "nope:1b"),
        ],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("the test"), "{}", out.stdout);
    assert_eq!(oll.count_path("/v1/messages"), 0);
    assert!(out.stderr.contains("nope:1b"), "{}", out.stderr);
}

#[test]
fn m06_nothing_available_prints_actionable_hint_quickly() {
    let env = Env::new();
    let out = env.run(&["--stdin"], Some("this is teh test."), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("the test"), "{}", out.stdout);
    assert!(
        out.stderr.contains("offline grammar mode"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("config.toml"), "{}", out.stderr);
    assert!(
        out.stderr.to_lowercase().contains("ollama"),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("quill status"), "{}", out.stderr);
    assert!(out.secs < 10.0, "took {}s", out.secs);
}

#[test]
fn m07_broken_ollama_probe_falls_back_to_offline() {
    let env = Env::new();
    for (status, body) in [(500u16, "boom"), (200u16, "not json at all")] {
        let oll = Mock::start(vec![Route {
            path: "/api/tags",
            status,
            body: body.into(),
            hang: false,
        }]);
        let out = env.run(
            &["--stdin"],
            Some("this is teh test."),
            &[("QUILL_OLLAMA_URL", oll.base.as_str())],
        );
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(out.stdout.contains("the test"), "{}", out.stdout);
        assert!(
            out.stderr.contains("offline grammar mode"),
            "{}",
            out.stderr
        );
    }
}

#[test]
fn m08_api_failure_in_auto_mode_falls_back_to_offline_without_leaking_secrets() {
    let env = Env::new();
    let api = Mock::api_status(500);
    env.cfg_api(&api.messages_url());
    let out = env.run(&["--stdin"], Some("this is teh test."), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("the test"), "{}", out.stdout);
    assert!(
        out.stderr.contains("falling back to offline grammar mode"),
        "{}",
        out.stderr
    );
    assert!(!out.stderr.contains(KEY) && !out.stdout.contains(KEY));
    assert_eq!(api.count(), 1);
}

#[test]
fn m09_api_failure_in_ai_mode_is_an_error_without_fallback() {
    let env = Env::new();
    let api = Mock::api_status(500);
    env.cfg_api(&api.messages_url());
    let out = env.run(
        &["--stdin"],
        Some("this is teh test."),
        &[("QUILL_MODE", "ai")],
    );
    assert_eq!(out.code, 1);
    assert_eq!(out.stdout, "");
    assert!(!out.stderr.contains("falling back"), "{}", out.stderr);
}

#[test]
fn m09b_api_timeout_in_auto_mode_falls_back() {
    let env = Env::new();
    let api = Mock::api_hang();
    env.write_conf(
        "config.toml",
        &format!(
            "[api]\nurl = \"{}\"\nkey = \"{KEY}\"\ntimeout_secs = 1\n",
            api.messages_url()
        ),
    );
    let out = env.run(&["--stdin"], Some("this is teh test."), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("the test"), "{}", out.stdout);
    assert!(
        out.stderr.contains("falling back to offline grammar mode"),
        "{}",
        out.stderr
    );
    assert!(out.secs < 15.0, "took {}s", out.secs);
}

#[test]
fn m10_ai_mode_keeps_todays_strictness_and_never_probes_ollama() {
    let env = Env::new();
    let oll = Mock::ollama(&["qwen3:30b"], &[], "NO");
    let out = env.run(
        &["--stdin"],
        Some("hello"),
        &[
            ("QUILL_MODE", "ai"),
            ("QUILL_OLLAMA_URL", oll.base.as_str()),
        ],
    );
    assert_eq!(out.code, 1);
    assert_eq!(out.stdout, "");
    assert_eq!(oll.count(), 0);
}

#[test]
fn m11_invalid_mode_values_are_rejected() {
    let env = Env::new();
    let out = env.run(&["--stdin"], Some("hello"), &[("QUILL_MODE", "bogus")]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("mode"), "{}", out.stderr);
    let out = env.run(&["--stdin", "--mode", "bogus"], Some("hello"), &[]);
    assert_eq!(out.code, 1);
    env.write_conf("config.toml", "[general]\nmode = \"bogus\"\n");
    let out = env.run(&["--stdin"], Some("hello"), &[]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("general.mode"), "{}", out.stderr);
}

#[test]
fn m12_mode_precedence_cli_over_env_over_config() {
    let env = Env::new();
    let api = Mock::api("AI REWRITE");
    env.write_conf(
        "config.toml",
        &format!(
            "[general]\nmode = \"offline\"\n[api]\nurl = \"{}\"\nkey = \"{KEY}\"\n",
            api.messages_url()
        ),
    );
    // config says offline -> no network
    let out = env.run(&["--stdin"], Some("this is teh test."), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("the test"));
    assert_eq!(api.count(), 0);
    // env beats config
    let out = env.run(&["--stdin"], Some("hello"), &[("QUILL_MODE", "ai")]);
    assert_eq!(out.stdout.trim(), "AI REWRITE");
    assert_eq!(api.count(), 1);
    // CLI beats env
    let out = env.run(
        &["--stdin", "--mode", "offline"],
        Some("this is teh test."),
        &[("QUILL_MODE", "ai")],
    );
    assert!(out.stdout.contains("the test"), "{}", out.stdout);
    assert_eq!(api.count(), 1);
}

#[test]
fn m13_ollama_never_receives_api_headers_or_credentials() {
    let env = Env::new();
    let oll = Mock::ollama(&["qwen3:30b"], &[], "OLLAMA REWRITE");
    env.write_conf(
        "config.toml",
        "[api]\naccept_invalid_certs = true\n[api.headers]\n\"X-Gateway-Secret\" = \"s3cret-gw-value\"\n",
    );
    let out = env.run(
        &["--stdin"],
        Some("hello"),
        &[("QUILL_OLLAMA_URL", oll.base.as_str())],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout.trim(), "OLLAMA REWRITE");
    let r = oll.last_msg();
    assert!(r.header("x-gateway-secret").is_none(), "{:?}", r.headers);
    let all = format!("{:?}{}", r.headers, r.body);
    assert!(!all.contains("s3cret-gw-value"));
    assert_eq!(r.header("x-api-key").as_deref(), Some("ollama"));
}

#[test]
fn m14_thinking_blocks_do_not_break_text_extraction() {
    let env = Env::new();
    let body = serde_json::json!({"content": [
        {"type": "thinking", "thinking": "hmm"},
        {"type": "text", "text": "pong"}
    ]})
    .to_string();
    let oll = Mock::start(vec![
        Route {
            path: "/api/tags",
            status: 200,
            body: models_json(&["qwen3:30b"]),
            hang: false,
        },
        Route {
            path: "/api/ps",
            status: 200,
            body: models_json(&[]),
            hang: false,
        },
        Route {
            path: "/v1/messages",
            status: 200,
            body,
            hang: false,
        },
    ]);
    let out = env.run(
        &["--stdin"],
        Some("ping"),
        &[("QUILL_OLLAMA_URL", oll.base.as_str())],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout.trim(), "pong");
    let out = env.run(&["check"], None, &[("QUILL_OLLAMA_URL", oll.base.as_str())]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("OK:"), "{}", out.stdout);
}

#[test]
fn m15_init_replacement_is_section_aware() {
    let env = Env::new();
    let out = env.run(
        &["init"],
        None,
        &[
            ("QUILL_API_URL", "http://127.0.0.1:9/v1/messages"),
            ("QUILL_API_KEY", "k-for-init-test"),
        ],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let text = fs::read_to_string(env.conf().join("config.toml")).unwrap();
    let v: toml::Value = toml::from_str(&text).unwrap();
    assert_eq!(
        v["api"]["url"].as_str(),
        Some("http://127.0.0.1:9/v1/messages")
    );
    assert_eq!(v["api"]["key"].as_str(), Some("k-for-init-test"));
    assert_eq!(v["ollama"]["url"].as_str(), Some("http://127.0.0.1:11434"));
    assert_eq!(v["general"]["mode"].as_str(), Some("auto"));
    assert_eq!(v["offline"]["dialect"].as_str(), Some("american"));
}

// ═════════════════════ A3b: per-command mode semantics ═════════════════════

#[test]
fn m16_offline_commands_make_zero_network_requests_and_learn_refuses() {
    let env = Env::new();
    let api = Mock::api("X");
    let oll = Mock::ollama(&["qwen3:30b"], &[], "X");
    env.cfg_api(&api.messages_url());
    let hist_line = "{\"before\":\"a\",\"after\":\"b\"}\n";
    let hist = env.write_conf("history.jsonl", hist_line);
    let args_extra: &[(&str, &str)] = &[
        ("QUILL_MODE", "offline"),
        ("QUILL_OLLAMA_URL", oll.base.as_str()),
    ];
    {
        let out = env.run(&["check"], None, args_extra);
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(
            out.stdout.contains("offline grammar engine"),
            "{}",
            out.stdout
        );
        let out = env.run(&["learn"], None, args_extra);
        assert_eq!(out.code, 1);
        assert!(
            out.stderr.contains("cannot run in offline mode"),
            "{}",
            out.stderr
        );
        let out = env.run(&["status"], None, args_extra);
        assert_eq!(out.code, 0, "{}", out.stderr);
        assert!(
            line_with(&out.stdout, "ollama:").contains("skipped"),
            "{}",
            out.stdout
        );
    }
    // same via config and via CLI flag
    env.write_conf(
        "config.toml",
        &format!(
            "[general]\nmode = \"offline\"\n[api]\nurl = \"{}\"\nkey = \"{KEY}\"\n",
            api.messages_url()
        ),
    );
    let out = env.run(&["learn"], None, &[("QUILL_OLLAMA_URL", oll.base.as_str())]);
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.contains("cannot run in offline mode"),
        "{}",
        out.stderr
    );
    let out = env.run(
        &["check", "--mode", "offline"],
        None,
        &[("QUILL_OLLAMA_URL", oll.base.as_str())],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(
        api.count() + oll.count(),
        0,
        "offline mode leaked a request"
    );
    assert_eq!(
        fs::read_to_string(&hist).unwrap(),
        hist_line,
        "history must be untouched"
    );
}

#[test]
fn m17_learn_without_any_provider_fails_clearly_in_auto_mode() {
    let env = Env::new();
    env.write_conf("history.jsonl", "{\"before\":\"a\",\"after\":\"b\"}\n");
    let out = env.run(&["learn"], None, &[]);
    assert_eq!(out.code, 1);
    assert!(
        out.stderr.to_lowercase().contains("ai provider"),
        "{}",
        out.stderr
    );
}

// ═════════════════════ A4: status ═════════════════════

#[test]
fn s01_status_with_nothing_configured_says_how_to_configure() {
    let env = Env::new();
    let out = env.run(&["status"], None, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        line_with(&out.stdout, "mode:").contains("auto"),
        "{}",
        out.stdout
    );
    let api = line_with(&out.stdout, "api:");
    assert!(
        api.contains("not configured") && api.contains("config.toml"),
        "{api}"
    );
    assert!(
        line_with(&out.stdout, "active:").contains("offline"),
        "{}",
        out.stdout
    );
    assert!(line_with(&out.stdout, "offline:")
        .to_lowercase()
        .contains("available"));
    assert!(
        !env.conf().join("config.toml").exists(),
        "status must not create files"
    );
}

#[test]
fn s02_status_reports_ollama_model_and_never_prints_the_key() {
    let env = Env::new();
    let oll = Mock::ollama(&["qwen3:30b"], &[], "X");
    let out = env.run(
        &["status"],
        None,
        &[("QUILL_OLLAMA_URL", oll.base.as_str())],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        line_with(&out.stdout, "ollama:").contains("qwen3:30b"),
        "{}",
        out.stdout
    );
    assert!(
        line_with(&out.stdout, "active:").contains("ollama"),
        "{}",
        out.stdout
    );
    assert_eq!(
        oll.count_path("/v1/messages"),
        0,
        "status must not send a completion request"
    );

    let api = Mock::api("X");
    env.cfg_api(&api.messages_url());
    let out = env.run(
        &["status"],
        None,
        &[("QUILL_OLLAMA_URL", oll.base.as_str())],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        line_with(&out.stdout, "active:").contains("api"),
        "{}",
        out.stdout
    );
    assert!(!out.stdout.contains(KEY) && !out.stderr.contains(KEY));
    assert_eq!(api.count(), 0, "status must not call the API");
}
