//! Acceptance tests for the quill CLI (spec: DESIGN.md Rev 3, T1-T32).
//!
//! Sealed harness: every child process runs with a cleared environment, a per-test HOME,
//! a per-test PATH that contains ONLY fail-fast sentinels (and the functional fakes a test
//! installs), and talks only to a bounded mock HTTP server on 127.0.0.1.
#![cfg(unix)]

use serde_json::Value;
use std::fs;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const KEY: &str = "test-key-123";
static COUNTER: AtomicU64 = AtomicU64::new(0);

#[cfg(target_os = "macos")]
type ModeT = u16;
#[cfg(not(target_os = "macos"))]
type ModeT = u32;
extern "C" {
    fn umask(mask: ModeT) -> ModeT;
}

// ───────────────────────── mock server ─────────────────────────

#[derive(Clone)]
enum Mode {
    Json(u16, String),
    Hang,
    Redirect(String),
}

#[derive(Clone, Debug)]
struct Req {
    headers: Vec<(String, String)>,
    body: String,
}

impl Req {
    fn header(&self, name: &str) -> Option<String> {
        let n = name.to_ascii_lowercase();
        self.headers.iter().find(|(k, _)| *k == n).map(|(_, v)| v.clone())
    }
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).expect("request body is JSON")
    }
}

struct Mock {
    url: String,
    reqs: Arc<Mutex<Vec<Req>>>,
    stop: Arc<AtomicBool>,
    th: Option<thread::JoinHandle<()>>,
}

fn ok_body(text: &str) -> String {
    serde_json::json!({"content": [{"type": "text", "text": text}]}).to_string()
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
    let mut headers = Vec::new();
    let mut clen = 0usize;
    for line in head.lines().skip(1) {
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
    let body = String::from_utf8_lossy(&buf[header_end..]).to_string();
    Some(Req { headers, body })
}

impl Mock {
    fn start(mode: Mode) -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let port = listener.local_addr().unwrap().port();
        let reqs = Arc::new(Mutex::new(Vec::new()));
        let stop = Arc::new(AtomicBool::new(false));
        let (r2, s2) = (reqs.clone(), stop.clone());
        let th = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(60);
            while !s2.load(Ordering::SeqCst) && Instant::now() < deadline {
                match listener.accept() {
                    Ok((mut s, _)) => {
                        let _ = s.set_nonblocking(false);
                        let _ = s.set_read_timeout(Some(Duration::from_secs(5)));
                        let _ = s.set_write_timeout(Some(Duration::from_secs(5)));
                        if let Some(req) = read_request(&mut s) {
                            r2.lock().unwrap().push(req);
                            match &mode {
                                Mode::Json(status, body) => {
                                    let resp = format!(
                                        "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                                        status,
                                        body.len(),
                                        body
                                    );
                                    let _ = s.write_all(resp.as_bytes());
                                }
                                Mode::Redirect(to) => {
                                    let resp = format!(
                                        "HTTP/1.1 302 Found\r\nLocation: {}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                                        to
                                    );
                                    let _ = s.write_all(resp.as_bytes());
                                }
                                Mode::Hang => {
                                    let until = Instant::now() + Duration::from_secs(10);
                                    while !s2.load(Ordering::SeqCst) && Instant::now() < until {
                                        thread::sleep(Duration::from_millis(20));
                                    }
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
        Mock { url: format!("http://127.0.0.1:{port}/v1/messages"), reqs, stop, th: Some(th) }
    }
    fn ok(text: &str) -> Mock {
        Mock::start(Mode::Json(200, ok_body(text)))
    }
    fn count(&self) -> usize {
        self.reqs.lock().unwrap().len()
    }
    fn req(&self, i: usize) -> Req {
        self.reqs.lock().unwrap()[i].clone()
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

// ───────────────────────── sealed environment ─────────────────────────

const SENTINELS: &[&str] = &[
    "pbpaste", "pbcopy", "skhd", "osascript", "xclip", "xsel", "wl-paste", "wl-copy", "powershell",
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
        Env::with_home_name("home")
    }
    fn with_home_name(home_name: &str) -> Env {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("quill-it-{}-{}", std::process::id(), n));
        let home = root.join(home_name);
        let bin = root.join("bin");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&bin).unwrap();
        let env = Env { root, home, bin };
        for name in SENTINELS {
            let log = env.root.join("sentinel.log");
            env.install_fake(name, &format!("#!/bin/sh\necho {} >> '{}'\nexit 99\n", name, log.display()));
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
    fn remove_fake(&self, name: &str) {
        let _ = fs::remove_file(self.bin.join(name));
    }
    fn write_conf(&self, rel: &str, content: &str) -> PathBuf {
        let p = self.conf().join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, content).unwrap();
        p
    }
    fn cfg(&self, url: &str) {
        self.write_conf("config.toml", &format!("[api]\nurl = \"{url}\"\nkey = \"{KEY}\"\n"));
    }
    fn sentinel_hits(&self) -> String {
        fs::read_to_string(self.root.join("sentinel.log")).unwrap_or_default()
    }
    fn assert_no_sentinel(&self) {
        assert_eq!(self.sentinel_hits(), "", "a real-tool sentinel was invoked");
    }
    fn run(&self, args: &[&str], stdin: Option<&str>, extra_env: &[(&str, &str)]) -> Out {
        unsafe {
            umask(0o022);
        }
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let out_p = self.root.join(format!("out{n}.txt"));
        let err_p = self.root.join(format!("err{n}.txt"));
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_quill"));
        cmd.args(args)
            .env_clear()
            .env("HOME", &self.home)
            .env("USERPROFILE", &self.home)
            .env("PATH", &self.bin)
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
        let deadline = Instant::now() + Duration::from_secs(30);
        let status = loop {
            if let Some(st) = child.try_wait().unwrap() {
                break st;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                panic!("quill did not exit within 30s: {:?}", args);
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
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

fn mode(p: &Path) -> u32 {
    fs::metadata(p).unwrap().permissions().mode() & 0o777
}

fn model_of(r: &Req) -> String {
    r.json()["model"].as_str().unwrap().to_string()
}

fn user_text(r: &Req) -> String {
    r.json()["messages"][0]["content"][0]["text"].as_str().unwrap().to_string()
}

fn system_text(r: &Req) -> String {
    r.json()["system"].as_str().unwrap().to_string()
}

// ───────────────────────── tests ─────────────────────────

#[test]
fn t01_init_creates_private_config() {
    let env = Env::new();
    let out = env.run(&["init"], None, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(mode(&env.conf()), 0o700);
    let cfg = env.conf().join("config.toml");
    assert_eq!(mode(&cfg), 0o600);
    assert!(env.conf().join("prompts").join("formal.txt").exists());
    let text = fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("YOUR_API_KEY_HERE"));
    for bad in ["user_type", "user_name"] {
        assert!(!text.to_lowercase().contains(&bad.to_lowercase()), "{bad} in sample config");
    }
    env.assert_no_sentinel();
}

#[test]
fn t02_init_twice_fails_and_keeps_file() {
    let env = Env::new();
    assert_eq!(env.run(&["init"], None, &[]).code, 0);
    let cfg = env.conf().join("config.toml");
    let before = fs::read(&cfg).unwrap();
    let out = env.run(&["init"], None, &[]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("already exists"), "{}", out.stderr);
    assert_eq!(before, fs::read(&cfg).unwrap());
}

#[test]
fn t03_init_uses_env_values() {
    let env = Env::new();
    let out = env.run(
        &["init"],
        None,
        &[("QUILL_API_URL", "https://llm.example.test/v1/messages"), ("QUILL_API_KEY", "abc123secret")],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let cfg = env.conf().join("config.toml");
    let text = fs::read_to_string(&cfg).unwrap();
    assert!(text.contains("https://llm.example.test/v1/messages"));
    assert!(text.contains("abc123secret"));
    assert!(!text.contains("YOUR_API_KEY_HERE"));
    assert_eq!(mode(&cfg), 0o600);
    assert!(!out.stdout.contains("abc123secret") && !out.stderr.contains("abc123secret"));
}

#[test]
fn t04_stdin_minimal_config_uses_defaults() {
    let env = Env::new();
    let m = Mock::ok("Hello polished");
    env.cfg(&m.url);
    let out = env.run(&["--stdin"], Some("hello world"), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "Hello polished\n");
    assert_eq!(m.count(), 1);
    let r = m.req(0);
    assert_eq!(r.header("x-api-key").as_deref(), Some(KEY));
    assert_eq!(r.header("anthropic-version").as_deref(), Some("2023-06-01"));
    assert!(r.header("content-type").unwrap().contains("application/json"));
    assert!(r.header("authorization").is_none());
    assert_eq!(model_of(&r), "claude-sonnet-4-5");
    let t = user_text(&r);
    assert!(t.contains("hello world") && t.contains("Instruction:"), "{t}");
    assert!(!system_text(&r).is_empty());
    assert!(!env.conf().join("history.jsonl").exists(), "--stdin must not write history");
    env.assert_no_sentinel();
}

#[test]
fn t05_bearer_auth_and_custom_headers() {
    let env = Env::new();
    let m = Mock::ok("x");
    env.write_conf(
        "config.toml",
        &format!("[api]\nurl = \"{}\"\nkey = \"{KEY}\"\nauth = \"bearer\"\n[api.headers]\nX-Gateway-Tenant = \"tenant-1\"\n", m.url),
    );
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let r = m.req(0);
    assert_eq!(r.header("authorization").as_deref(), Some("Bearer test-key-123"));
    assert!(r.header("x-api-key").is_none());
    assert_eq!(r.header("x-gateway-tenant").as_deref(), Some("tenant-1"));
}

#[test]
fn t06_env_only_configuration() {
    let env = Env::new();
    let m = Mock::ok("from env");
    let out = env.run(
        &["--stdin"],
        Some("hi"),
        &[("QUILL_API_URL", &m.url), ("QUILL_API_KEY", "envkey"), ("QUILL_MODEL", "my-model")],
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, "from env\n");
    let r = m.req(0);
    assert_eq!(r.header("x-api-key").as_deref(), Some("envkey"));
    assert_eq!(model_of(&r), "my-model");
}

#[test]
fn t07_missing_or_placeholder_key_is_rejected() {
    for key in ["", "   ", "YOUR_API_KEY_HERE"] {
        let env = Env::new();
        let m = Mock::ok("x");
        env.write_conf("config.toml", &format!("[api]\nurl = \"{}\"\nkey = \"{}\"\n", m.url, key));
        let out = env.run(&["--stdin"], Some("hi"), &[]);
        assert_eq!(out.code, 1, "key {key:?}");
        assert!(out.stderr.contains("api.key") || out.stderr.contains("QUILL_API_KEY"), "{}", out.stderr);
        assert_eq!(m.count(), 0);
    }
}

#[test]
fn t08_plain_http_to_non_loopback_is_rejected() {
    let env = Env::new();
    env.cfg("http://example.com/v1/messages");
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("https"), "{}", out.stderr);
    assert!(out.secs < 4.0, "must be rejected before any network I/O ({}s)", out.secs);
}

#[test]
fn t09_http_error_never_echoes_secrets() {
    let env = Env::new();
    let secret_header = "hdr-secret-777";
    let text = "my private note";
    let body = format!("{{\"error\":\"bad key {KEY} header {secret_header} text {text}\"}}");
    let m = Mock::start(Mode::Json(401, body));
    env.write_conf(
        "config.toml",
        &format!("[api]\nurl = \"{}\"\nkey = \"{KEY}\"\n[api.headers]\nX-Secret-Thing = \"{secret_header}\"\n", m.url),
    );
    let out = env.run(&["--stdin"], Some(text), &[]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("401"), "{}", out.stderr);
    assert_eq!(out.stdout, "");
    let mut all = format!("{}{}", out.stdout, out.stderr);
    if let Ok(rd) = fs::read_dir(env.conf()) {
        for e in rd.flatten() {
            if e.path().extension().map(|x| x == "log").unwrap_or(false) {
                all.push_str(&fs::read_to_string(e.path()).unwrap_or_default());
            }
        }
    }
    for s in [KEY, secret_header, text] {
        assert!(!all.contains(s), "leaked {s:?}: {all}");
    }
}

#[test]
fn t10_prompt_profiles() {
    let env = Env::new();
    let m = Mock::ok("x");
    env.cfg(&m.url);
    env.write_conf("prompts/formal.txt", "FORMAL-INSTR-TEXT\n");
    assert_eq!(env.run(&["--stdin", "-p", "formal"], Some("hi"), &[]).code, 0);
    assert!(user_text(&m.req(0)).contains("FORMAL-INSTR-TEXT"));
    assert_eq!(env.run(&["--stdin", "--prompt", "formal"], Some("hi"), &[]).code, 0);
    assert!(user_text(&m.req(1)).contains("FORMAL-INSTR-TEXT"));
    assert_eq!(env.run(&["--stdin"], Some("hi"), &[("QUILL_PROMPT", "formal")]).code, 0);
    assert!(user_text(&m.req(2)).contains("FORMAL-INSTR-TEXT"));
    let before = m.count();
    assert_eq!(env.run(&["--stdin", "-p", "nosuch"], Some("hi"), &[]).code, 1);
    assert_eq!(env.run(&["--stdin", "-p", "../x"], Some("hi"), &[]).code, 1);
    assert_eq!(env.run(&["--stdin", "-p", "a/b"], Some("hi"), &[]).code, 1);
    assert_eq!(m.count(), before, "invalid profile names must not reach the network");
}

#[test]
fn t11_context_is_injected_into_system_prompt() {
    let env = Env::new();
    let m = Mock::ok("x");
    env.cfg(&m.url);
    env.write_conf("context.md", "CTX-MARKER-XYZ\n");
    assert_eq!(env.run(&["--stdin"], Some("hi"), &[]).code, 0);
    assert!(system_text(&m.req(0)).contains("CTX-MARKER-XYZ"));
}

#[test]
fn t12_check_command() {
    let env = Env::new();
    let m = Mock::ok("OK");
    env.cfg(&m.url);
    let out = env.run(&["check"], None, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("OK:"), "{}", out.stdout);
    assert!(m.req(0).json()["max_tokens"].as_u64().unwrap() <= 32);
    let env2 = Env::new();
    let m2 = Mock::start(Mode::Json(500, "{}".into()));
    env2.cfg(&m2.url);
    assert_eq!(env2.run(&["check"], None, &[]).code, 1);
}

#[test]
fn t13_legacy_config_keys_are_ignored_and_never_sent() {
    let env = Env::new();
    let m = Mock::ok("x");
    env.write_conf(
        "config.toml",
        &format!(
            "[api]\nurl = \"{}\"\nkey = \"{KEY}\"\nmodel = \"m\"\nanthropic_version = \"2023-06-01\"\nuser_type = \"SOMETYPE\"\nuser_name = \"someone\"\naccept_invalid_certs = false\n",
            m.url
        ),
    );
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    for (k, v) in &m.req(0).headers {
        let all = format!("{k}={v}").to_lowercase();
        for bad in ["user_type", "user_name", "sometype", "someone"] {
            assert!(!all.contains(bad), "header {all}");
        }
    }
}

#[test]
fn t14_empty_stdin_is_rejected() {
    for input in ["", "  \n\t"] {
        let env = Env::new();
        let m = Mock::ok("x");
        env.cfg(&m.url);
        let out = env.run(&["--stdin"], Some(input), &[]);
        assert_eq!(out.code, 1);
        assert!(out.stderr.contains("no input"), "{}", out.stderr);
        assert_eq!(m.count(), 0);
    }
}

#[cfg(target_os = "macos")]
fn install_clipboard_fakes(env: &Env, clip_in: &str) -> PathBuf {
    let cin = env.root.join("clip_in.txt");
    let cout = env.root.join("clip_out.txt");
    fs::write(&cin, clip_in).unwrap();
    env.install_fake("pbpaste", &format!("#!/bin/sh\n/bin/cat '{}'\n", cin.display()));
    env.install_fake("pbcopy", &format!("#!/bin/sh\n/bin/cat > '{}'\n", cout.display()));
    cout
}

#[cfg(target_os = "macos")]
#[test]
fn t15_now_uses_clipboard_and_writes_private_history() {
    let env = Env::new();
    let m = Mock::ok("Result text");
    env.cfg(&m.url);
    let cout = install_clipboard_fakes(&env, "clipboard input");
    // pre-existing permissive history must be repaired (T30, macOS part)
    let hist = env.write_conf("history.jsonl", "");
    fs::set_permissions(&hist, fs::Permissions::from_mode(0o644)).unwrap();
    let out = env.run(&["--now"], None, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(fs::read_to_string(&cout).unwrap().trim(), "Result text");
    let lines: Vec<String> = fs::read_to_string(&hist).unwrap().lines().map(|s| s.to_string()).collect();
    assert_eq!(lines.len(), 1);
    let v: Value = serde_json::from_str(&lines[0]).unwrap();
    assert_eq!(v["before"], "clipboard input");
    assert_eq!(v["after"], "Result text");
    assert_eq!(mode(&hist), 0o600);
    assert!(!env.sentinel_hits().contains("pbpaste") && !env.sentinel_hits().contains("pbcopy"));
}

#[test]
fn t16_learn_updates_context_and_clears_history() {
    let env = Env::new();
    let m = Mock::ok("# Quill Writing Context\n\n## Preferences\n- LEARNED-MARKER\n");
    env.cfg(&m.url);
    let hist = env.write_conf(
        "history.jsonl",
        "{\"timestamp\":\"unix:1\",\"before\":\"a\",\"after\":\"b\"}\n{\"timestamp\":\"unix:2\",\"before\":\"c\",\"after\":\"d\"}\n",
    );
    let out = env.run(&["learn"], None, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(fs::read_to_string(env.conf().join("context.md")).unwrap().contains("LEARNED-MARKER"));
    assert_eq!(fs::read_to_string(&hist).unwrap().trim(), "");
    assert_eq!(m.count(), 1);
    // no history => exit 1, no request
    let env2 = Env::new();
    let m2 = Mock::ok("x");
    env2.cfg(&m2.url);
    let out2 = env2.run(&["learn"], None, &[]);
    assert_eq!(out2.code, 1);
    assert_eq!(m2.count(), 0);
}

#[cfg(target_os = "macos")]
const BEGIN: &str = "# >>> quill managed block v1 >>>";
#[cfg(target_os = "macos")]
const END: &str = "# <<< quill managed block v1 <<<";

#[cfg(target_os = "macos")]
fn fake_skhd(env: &Env) -> PathBuf {
    let log = env.root.join("skhd.log");
    env.install_fake("skhd", &format!("#!/bin/sh\necho \"$@\" >> '{}'\nexit 0\n", log.display()));
    log
}

#[cfg(target_os = "macos")]
#[test]
fn t17_setup_uses_a_managed_skhd_block() {
    let env = Env::with_home_name("Test Home's dir");
    env.cfg("https://llm.example.test/v1/messages");
    let skhd_log = fake_skhd(&env);
    let rc = env.home.join(".skhdrc");
    let original = "# my own bindings\nalt - t : open -a Terminal\n# transformer notes\nctrl - q : /opt/example/quill/hotkey.sh\n";
    fs::write(&rc, original).unwrap();
    let out = env.run(&["setup"], None, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    let after = fs::read_to_string(&rc).unwrap();
    assert!(after.starts_with(original), "unrelated bytes must be preserved:\n{after}");
    assert_eq!(after.matches(BEGIN).count(), 1);
    assert_eq!(after.matches(END).count(), 1);
    let block: Vec<&str> = after.split(BEGIN).nth(1).unwrap().split(END).next().unwrap().lines().filter(|l| !l.trim().is_empty()).collect();
    assert_eq!(block.len(), 1, "{block:?}");
    assert!(block[0].starts_with("cmd + shift - c : ") && block[0].ends_with("hotkey.sh'"), "{}", block[0]);
    // idempotent
    assert_eq!(env.run(&["setup"], None, &[]).code, 0);
    assert_eq!(after, fs::read_to_string(&rc).unwrap());
    // hotkey script
    let hk = env.conf().join("hotkey.sh");
    let script = fs::read_to_string(&hk).unwrap();
    assert_eq!(mode(&hk), 0o700);
    assert!(!script.contains("/tmp"), "{script}");
    assert!(script.contains("umask 077"));
    assert!(script.contains("quill.log"));
    let ok = Command::new("/bin/sh").arg("-n").arg(&hk).status().unwrap();
    assert!(ok.success(), "hotkey.sh must be valid sh even with a space/apostrophe in HOME:\n{script}");
    assert!(fs::read_to_string(&skhd_log).unwrap().contains("--restart-service"));
    // unbalanced marker => exit 1, bytes unchanged
    let broken = format!("{original}{BEGIN}\nstray\n");
    fs::write(&rc, &broken).unwrap();
    let out2 = env.run(&["setup"], None, &[]);
    assert_eq!(out2.code, 1);
    assert_eq!(broken, fs::read_to_string(&rc).unwrap());
    // symlinked .skhdrc: target updated, symlink kept
    let target = env.root.join("dotfiles_skhdrc");
    fs::write(&target, original).unwrap();
    fs::remove_file(&rc).unwrap();
    symlink(&target, &rc).unwrap();
    let out3 = env.run(&["setup"], None, &[]);
    assert_eq!(out3.code, 0, "{}", out3.stderr);
    assert!(fs::symlink_metadata(&rc).unwrap().file_type().is_symlink());
    assert!(fs::read_to_string(&target).unwrap().contains(BEGIN));
    // no skhd installed => exit 0 with a warning that mentions skhd
    let env2 = Env::new();
    env2.cfg("https://llm.example.test/v1/messages");
    env2.remove_fake("skhd");
    let out4 = env2.run(&["setup"], None, &[]);
    assert_eq!(out4.code, 0, "{}", out4.stderr);
    assert!(out4.stderr.to_lowercase().contains("skhd"), "{}", out4.stderr);
}

#[test]
fn t18_help_and_version() {
    let env = Env::new();
    for a in [vec!["--help"], vec!["-h"], vec!["help"], vec![]] {
        let out = env.run(&a, None, &[]);
        assert_eq!(out.code, 0, "{a:?} {}", out.stderr);
        let all = format!("{}{}", out.stdout, out.stderr);
        assert!(all.contains("quill init") && all.contains("--stdin"), "{all}");
    }
    for a in ["--version", "-V"] {
        let out = env.run(&[a], None, &[]);
        assert_eq!(out.code, 0);
        assert_eq!(out.stdout.trim(), format!("quill {}", env!("CARGO_PKG_VERSION")));
    }
    assert_eq!(env!("CARGO_PKG_VERSION"), "0.4.0");
    env.assert_no_sentinel();
}

#[test]
fn t19_request_timeout() {
    let env = Env::new();
    let m = Mock::start(Mode::Hang);
    env.write_conf("config.toml", &format!("[api]\nurl = \"{}\"\nkey = \"{KEY}\"\ntimeout_secs = 1\n", m.url));
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 1);
    assert!(out.secs < 8.0, "{}s", out.secs);
    assert!(out.stderr.to_lowercase().contains("timeout"), "{}", out.stderr);
}

#[test]
fn t20_unicode_and_multiline_round_trip() {
    let env = Env::new();
    let reply = "Ünïcode  résumé";
    let m = Mock::ok(reply);
    env.cfg(&m.url);
    let input = "héllo wörld — 日本語\nline two ";
    let out = env.run(&["--stdin"], Some(input), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(out.stdout, format!("{reply}\n"));
    assert!(user_text(&m.req(0)).contains(input));
}

#[test]
fn t21_invalid_url_schemes_are_rejected() {
    for url in ["ftp://127.0.0.1:1/x", "file:///etc/passwd", "not a url", ""] {
        let env = Env::new();
        env.cfg(url);
        let out = env.run(&["--stdin"], Some("hi"), &[]);
        assert_eq!(out.code, 1, "url {url:?}");
        assert!(out.stderr.contains("api.url") || out.stderr.contains("QUILL_API_URL"), "{}", out.stderr);
        assert!(out.secs < 4.0);
    }
}

#[test]
fn t22_url_with_userinfo_is_rejected_and_not_echoed() {
    let env = Env::new();
    env.cfg("https://someuser:hunter2pw@127.0.0.1:1/v1/messages");
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 1);
    let all = format!("{}{}", out.stdout, out.stderr);
    assert!(all.contains("api.url") || all.contains("QUILL_API_URL"), "{all}");
    assert!(!all.contains("hunter2pw") && !all.contains("someuser"), "{all}");
}

#[test]
fn t23_malformed_json_response() {
    let env = Env::new();
    let m = Mock::start(Mode::Json(200, "this is not json {".into()));
    env.cfg(&m.url);
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("unexpected response"), "{}", out.stderr);
    assert!(!out.stderr.contains("this is not json"));
    assert_eq!(out.stdout, "");
}

#[test]
fn t24_response_without_text_block() {
    for body in ["{\"content\":[{\"type\":\"tool_use\"}]}", "{\"content\":[]}"] {
        let env = Env::new();
        let m = Mock::start(Mode::Json(200, body.into()));
        env.cfg(&m.url);
        let out = env.run(&["--stdin"], Some("hi"), &[]);
        assert_eq!(out.code, 1, "{body}");
        assert!(out.stderr.contains("no text"), "{}", out.stderr);
        assert_eq!(out.stdout, "");
    }
}

#[test]
fn t25_unknown_auth_value() {
    let env = Env::new();
    let m = Mock::ok("x");
    env.write_conf("config.toml", &format!("[api]\nurl = \"{}\"\nkey = \"{KEY}\"\nauth = \"basic\"\n", m.url));
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("auth"), "{}", out.stderr);
    assert_eq!(m.count(), 0);
}

#[test]
fn t26_timeout_range() {
    for t in ["0", "601"] {
        let env = Env::new();
        let m = Mock::ok("x");
        env.write_conf("config.toml", &format!("[api]\nurl = \"{}\"\nkey = \"{KEY}\"\ntimeout_secs = {t}\n", m.url));
        let out = env.run(&["--stdin"], Some("hi"), &[]);
        assert_eq!(out.code, 1, "timeout {t}");
        assert!(out.stderr.contains("timeout_secs"), "{}", out.stderr);
        assert_eq!(m.count(), 0);
    }
}

#[test]
fn t27_redirects_are_not_followed() {
    let env = Env::new();
    let target = Mock::ok("must not be reached");
    let redirector = Mock::start(Mode::Redirect(target.url.clone()));
    env.cfg(&redirector.url);
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.to_lowercase().contains("redirect"), "{}", out.stderr);
    assert_eq!(redirector.count(), 1);
    assert_eq!(target.count(), 0, "a redirect target must never receive the request");
}

#[test]
fn t28_reserved_and_invalid_headers_are_rejected() {
    for h in ["Authorization", "X-API-Key", "Content-Type", "Host", "anthropic-version", "Cookie", "Proxy-Authorization"] {
        let env = Env::new();
        let m = Mock::ok("x");
        env.write_conf(
            "config.toml",
            &format!("[api]\nurl = \"{}\"\nkey = \"{KEY}\"\n[api.headers]\n\"{h}\" = \"x\"\n", m.url),
        );
        let out = env.run(&["--stdin"], Some("hi"), &[]);
        assert_eq!(out.code, 1, "{h}");
        assert!(out.stderr.to_lowercase().contains("header"), "{}", out.stderr);
        assert_eq!(m.count(), 0, "{h}");
    }
    let env = Env::new();
    let m = Mock::ok("x");
    env.write_conf(
        "config.toml",
        &format!("[api]\nurl = \"{}\"\nkey = \"{KEY}\"\n[api.headers]\nX-Ok = \"a\\nb\"\n", m.url),
    );
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 1);
    assert_eq!(m.count(), 0);
}

#[test]
fn t29_connection_refused_is_sanitized() {
    let env = Env::new();
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    drop(l);
    env.cfg(&format!("http://127.0.0.1:{port}/v1/messages"));
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 1);
    assert!(out.stderr.contains("request failed"), "{}", out.stderr);
    assert!(!out.stderr.contains("127.0.0.1") && !out.stderr.contains(&port.to_string()), "{}", out.stderr);
}

#[test]
fn t30_permissive_existing_files_are_repaired() {
    let env = Env::new();
    let m = Mock::ok("x");
    let cfg = env.write_conf("config.toml", &format!("[api]\nurl = \"{}\"\nkey = \"{KEY}\"\n", m.url));
    let ctx = env.write_conf("context.md", "ctx\n");
    fs::set_permissions(&cfg, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(&ctx, fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(env.conf(), fs::Permissions::from_mode(0o755)).unwrap();
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(mode(&env.conf()), 0o700);
    assert_eq!(mode(&cfg), 0o600);
    assert_eq!(mode(&ctx), 0o600);
}

#[test]
fn t31_symlinked_sensitive_files_are_refused() {
    // learn: symlinked context.md
    let env = Env::new();
    let m = Mock::ok("# new context\n- x\n");
    env.cfg(&m.url);
    env.write_conf("history.jsonl", "{\"timestamp\":\"unix:1\",\"before\":\"a\",\"after\":\"b\"}\n");
    let victim = env.root.join("victim_ctx.txt");
    fs::write(&victim, "VICTIM-ORIG").unwrap();
    symlink(&victim, env.conf().join("context.md")).unwrap();
    let out = env.run(&["learn"], None, &[]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "VICTIM-ORIG");
    // init: dangling symlink at config.toml must not be written through
    let env2 = Env::new();
    fs::create_dir_all(env2.conf()).unwrap();
    let ghost = env2.root.join("ghost_target.toml");
    symlink(&ghost, env2.conf().join("config.toml")).unwrap();
    let out2 = env2.run(&["init"], None, &[]);
    assert_eq!(out2.code, 1, "{}", out2.stderr);
    assert!(!ghost.exists(), "init must not create the symlink target");
}

#[cfg(target_os = "macos")]
#[test]
fn t31b_now_with_symlinked_history_keeps_victim_untouched() {
    let env = Env::new();
    let m = Mock::ok("Result");
    env.cfg(&m.url);
    install_clipboard_fakes(&env, "clip");
    let victim = env.root.join("victim_hist.txt");
    fs::write(&victim, "VICTIM-ORIG").unwrap();
    symlink(&victim, env.conf().join("history.jsonl")).unwrap();
    let out = env.run(&["--now"], None, &[]);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.to_lowercase().contains("symlink"), "{}", out.stderr);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "VICTIM-ORIG");
}

#[test]
fn t32_no_real_tool_was_touched_by_basic_flows() {
    let env = Env::new();
    let m = Mock::ok("x");
    env.cfg(&m.url);
    assert_eq!(env.run(&["--stdin"], Some("hi"), &[]).code, 0);
assert_eq!(env.run(&["check"], None, &[]).code, 0);
    assert_eq!(m.count(), 2, "stdin and check must each send exactly one request");
    env.assert_no_sentinel();
}

#[test]
fn t33_toml_parse_errors_do_not_echo_the_key_line() {
    let env = Env::new();
    env.write_conf(
        "config.toml",
        &format!("[api]\nurl = \"https://llm.example.test/v1/messages\"\nkey = \"{KEY}\" garbage\n"),
    );
    let out = env.run(&["--stdin"], Some("hi"), &[]);
    assert_eq!(out.code, 1);
    let all = format!("{}{}", out.stdout, out.stderr);
    assert!(all.to_lowercase().contains("config"), "{all}");
    assert!(!all.contains(KEY), "parse error leaked the key line: {all}");
    env.assert_no_sentinel();
}

#[cfg(target_os = "macos")]
#[test]
fn t34_setup_refuses_a_symlinked_log() {
    let env = Env::new();
    env.cfg("https://llm.example.test/v1/messages");
    fake_skhd(&env);
    let victim = env.root.join("victim_log.txt");
    fs::write(&victim, "VICTIM-ORIG").unwrap();
    fs::create_dir_all(env.conf()).unwrap();
    symlink(&victim, env.conf().join("quill.log")).unwrap();
    let out = env.run(&["setup"], None, &[]);
    assert_eq!(out.code, 1, "{}", out.stderr);
    assert!(out.stderr.to_lowercase().contains("symlink"), "{}", out.stderr);
    assert_eq!(fs::read_to_string(&victim).unwrap(), "VICTIM-ORIG");
}
