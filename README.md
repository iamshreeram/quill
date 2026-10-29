# Quill

Quill is a lightweight system-wide text transformer. You copy text, press a shortcut, and the text on your clipboard is replaced with a rewritten version that you can paste.

Quill has three ways to rewrite text:

- **AI via an Anthropic-Messages-compatible API** that you configure with a key.
- **AI via a local Ollama model**, so the text stays on your machine (no key needed).
- **An offline grammar fixer** built on `harper-core`. It corrects grammar, punctuation, spacing, capitalization and common typos. It makes no network requests.

Quill picks one of these automatically (see [Modes and provider selection](#modes-and-provider-selection)), or you can force one with `--mode`. Quill also keeps optional local writing-style context, learned from your before/after pairs, that is sent along with each AI request.

## How it works

1. Select text and copy it (Cmd+C on macOS, Ctrl+C elsewhere).
2. Press the transform shortcut (Cmd+Shift+C on macOS, Ctrl+Shift+C on Windows). Quill reads the clipboard, transforms the text with the provider it selects, and writes the result back to the clipboard.
3. Paste the result (Cmd+V / Ctrl+V).

Quill can also read from standard input (`echo 'text' | quill --stdin`), which needs no clipboard.

## Platform support

| Platform | Installer | Prebuilt binary | Clipboard | Hotkey | Status |
|---|---|---|---|---|---|
| macOS (arm64, x86_64) | `install.sh` | Yes | `pbpaste` / `pbcopy` | skhd (see `quill setup`) | Supported and tested by the author |
| Linux (x86_64, aarch64, glibc) | `install.sh` | Yes (needs glibc >= 2.35 and OpenSSL 3) | `wl-paste`/`wl-copy` on Wayland, `xclip` on X11 (`xsel` as fallback) | Bind your desktop shortcut to `quill --now` | EXPERIMENTAL: the clipboard code is untested on real desktops |
| Windows (x86_64) | `install.ps1` | Yes (zip) | PowerShell `Get-Clipboard` / `Set-Clipboard` | Fixed to Ctrl+Shift+C; the `[shortcut] windows` setting is informational only | EXPERIMENTAL: not built or tested by the author; `install.ps1` is untested |

On Linux you need `wl-clipboard` (Wayland) or `xclip` (X11) installed at run time. To build from source you need `pkg-config` plus the OpenSSL development headers (for example `libssl-dev` on Debian/Ubuntu). On Windows, running `quill` with no arguments starts the hotkey listener; the Windows build is a GUI-subsystem program, so `--help`, `--stdin` and error output may not appear in an interactive console.

Continuous integration (see `.github/workflows/ci.yml`) runs lint, builds and tests on Ubuntu, macOS and Windows, and checks the shell scripts. The Windows job is experimental and its smoke step cannot fail the job.

## Requirements

- For the installer: `curl`, and `shasum` or `sha256sum`.
- For source builds: Rust >= 1.90 (stable) and `git`.
- Optional, for AI rewrite: an Anthropic-Messages-compatible endpoint and an API key, or a local [Ollama](https://ollama.com) server with a pulled model. Without either, Quill uses the offline grammar engine, which needs nothing.
- macOS only, for the global hotkey: [skhd](https://github.com/koekeishiya/skhd), for example `brew install koekeishiya/formulae/skhd`.

## Install

### One-line installer (recommended)

```bash
curl -fsSL https://github.com/iamshreeram/quill/releases/latest/download/install.sh | bash
```

The installer does the following:

1. Detects your platform (macOS or Linux, x86_64 or aarch64). Anything else stops with a pointer to the source build.
2. Resolves one exact release tag (the latest release, or `QUILL_VERSION`) and uses it for the whole run.
3. Downloads `quill-<version>-<target>.tar.gz` and `SHA256SUMS` for that tag. It verifies the archive's SHA256 against `SHA256SUMS` and installs nothing if the checksum is missing or does not match.
4. Checks that the binary reports exactly the expected version, then installs it atomically to `~/.local/bin/quill` (or `QUILL_INSTALL_DIR`).
5. Creates the config with `quill init` if `~/.config/quill/config.toml` does not exist. An existing config is left unchanged.
6. Prints a PATH line if the install directory is not on your `PATH`. It never edits your shell rc files.
7. On macOS: offers to install skhd with Homebrew (asks first, see `QUILL_YES`), runs `quill setup`, starts skhd, and reminds you to grant skhd Accessibility permission (System Settings > Privacy & Security > Accessibility).
8. On Linux: tells you to install a clipboard tool (`wl-clipboard` or `xclip`) if none is found, and reminds you to bind a desktop shortcut to `quill --now`. Quill does not set up a Linux hotkey.
9. Runs `quill status` to show the mode and the provider that would be used.

If `~/.local/bin` is not on your `PATH`, add it yourself, for example `export PATH="$HOME/.local/bin:$PATH"` (fish: `fish_add_path ~/.local/bin`).

### Installer environment knobs

| Variable | Default | Effect |
|---|---|---|
| `QUILL_VERSION` | latest release | Install this version, for example `0.4.0` or `v0.4.0`. |
| `QUILL_INSTALL_DIR` | `$HOME/.local/bin` | Directory for the `quill` binary. |
| `QUILL_NO_SETUP` | unset | `1` skips `quill setup` on macOS. |
| `QUILL_YES` | unset | `1` answers yes to prompts (for example, installing skhd with Homebrew). |
| `QUILL_NONINTERACTIVE` | unset | `1` never prompts; every question is answered "no". |
| `QUILL_FROM_SOURCE` | unset | `1` builds from source with `cargo` instead of downloading a binary. |
| `QUILL_BASE_URL` | `https://github.com/iamshreeram/quill/releases` | Release location. A non-https value prints a warning. |
| `QUILL_REPO_URL` | `https://github.com/iamshreeram/quill.git` | Repository cloned for source builds (at the release tag). |

Example, pinning a version and skipping the macOS hotkey setup:

```bash
QUILL_VERSION=0.4.0 QUILL_NO_SETUP=1 bash -c "$(curl -fsSL https://github.com/iamshreeram/quill/releases/latest/download/install.sh)"
```

### Prebuilt binaries

| Target | Archive | Notes |
|---|---|---|
| `aarch64-apple-darwin` | `.tar.gz` | macOS on Apple Silicon. |
| `x86_64-apple-darwin` | `.tar.gz` | macOS on Intel. |
| `x86_64-unknown-linux-gnu` | `.tar.gz` | Needs glibc >= 2.35 and `libssl.so.3` (OpenSSL 3). |
| `aarch64-unknown-linux-gnu` | `.tar.gz` | Same Linux baseline as above. |
| `x86_64-pc-windows-msvc` | `.zip` | Windows, experimental. Use `install.ps1`. |

On Linux, `install.sh` checks the glibc version (`getconf GNU_LIBC_VERSION`) and whether `libssl.so.3` is registered with `ldconfig`. If either check fails, it stops with a message. Install OpenSSL 3 (for example `apt-get install libssl3`) or build from source. The check also applies to `QUILL_FROM_SOURCE=1`.

If a release has no archive for your target (HTTP 404), `install.sh` falls back to a source build. Other download failures stop the installer.

Windows: `install.ps1` is EXPERIMENTAL and was written without a Windows host. Review it before running. It downloads the x86_64 zip for the resolved tag and honours `QUILL_VERSION`, `QUILL_BASE_URL` and `QUILL_INSTALL_DIR` (default `%LOCALAPPDATA%\quill\bin`). Its checksum and PATH handling were not reviewed for this README:

```powershell
irm https://github.com/iamshreeram/quill/releases/latest/download/install.ps1 | iex
```

### Build from source

```bash
git clone https://github.com/iamshreeram/quill.git
cd quill
cargo install --path . --locked
```

This needs Rust >= 1.90 (the `rust-version` in `Cargo.toml`). Homebrew's `cargo` can shadow a rustup-managed toolchain and be too old. If you use rustup, put its directory first:

```bash
PATH=$HOME/.cargo/bin:$PATH cargo install --path . --locked
```

Or build without installing and copy the binary yourself:

```bash
cargo build --release --locked
cp target/release/quill ~/.local/bin/
```

### Security note: piping to a shell

`curl ... | bash` runs whatever the server sends, without a chance to read it. If you prefer to inspect the script first, download it, read it, and then run it:

```bash
curl -fsSL -o install.sh https://github.com/iamshreeram/quill/releases/latest/download/install.sh
less install.sh
bash install.sh
```

The installer still verifies the binary's SHA256 against `SHA256SUMS` from the same release, so the checksum protects against a corrupted or tampered archive, not against a tampered release itself. Verify the release through a source you trust if that matters to you.

## Quick start

1. Optional: create the config. `quill init` is no longer required (Quill works without a config file), but it creates `~/.config/quill/config.toml` and the `prompts/` directory. If you set `QUILL_API_URL` and `QUILL_API_KEY` in the environment first, `init` writes them into the new config:

   ```bash
   quill init
   ```

2. For AI rewrite through an API, set `key` in `~/.config/quill/config.toml` (and `url`, if you use a gateway). Or use a local Ollama model instead (see [Modes and provider selection](#modes-and-provider-selection)).

3. See which provider Quill will use, then verify it:

   ```bash
   quill status
   quill check
   ```

4. On macOS, install the hotkey (the installer does this for you):

   ```bash
   quill setup
   ```

   Then use it: select text, press Cmd+C, then Cmd+Shift+C, then Cmd+V.

Other ways to run it:

```bash
echo 'Please send me the file asap thx' | quill --stdin
quill --now -p formal
quill --stdin --offline
```

## Commands

| Command | Description |
|---|---|
| `quill init` | Create the config file, the `prompts/` directory and sample profiles. Fails if the config already exists. Optional. |
| `quill status` | Show the mode (and where it came from), the API, Ollama and offline state, the provider that would be used, and the hotkey state. Sends no AI request. |
| `quill setup` | macOS: write `hotkey.sh` and the managed skhd block (see below). Linux: prints the shortcut hint. Windows: prints the shortcut. |
| `quill check` | Validate the provider that would be used. For an API or Ollama it sends one small test request and prints `OK: <model> responded`. In offline mode it runs a local self-check and prints `OK: offline grammar engine available` (no network). |
| `quill --stdin [-p NAME]` | Read UTF-8 text from stdin, print the transformed text to stdout. Does not touch the clipboard or history. |
| `quill --now` / `quill -n [-p NAME]` | Transform the clipboard text and write the result back to the clipboard. AI results are recorded as a history pair. |
| `-p NAME` / `--prompt NAME` | Use the prompt profile `NAME` for this run. Ignored in offline mode (with a warning). |
| `--mode auto\|ai\|offline` | Choose the mode for this run. See [Modes and provider selection](#modes-and-provider-selection). |
| `--offline` | Same as `--mode offline`. Giving `--offline` together with a different `--mode` is an error. |
| `quill learn` | Analyze `history.jsonl`, update `context.md`, and clear the history. Needs an AI provider. |
| `quill --help` (`-h`, `help`) | Print usage. Running `quill` with no arguments also prints usage on macOS and Linux. |
| `quill --version` (`-V`) | Print `quill <version>`. |

Only one command may be given at a time. Exit codes: `0` on success, `1` on any error.

## Modes and provider selection

The mode decides which backends a command may use.

| Mode | Uses | Network | Falls back? |
|---|---|---|---|
| `auto` (default) | Configured API, else a usable local Ollama model, else the offline grammar engine | API or Ollama only if one is selected | Yes: to the next option in the order below |
| `ai` | Configured API only | Only the API | No. Errors are reported; nothing runs offline and Ollama is not probed. |
| `offline` | Offline grammar engine only | None | Not applicable |

### Auto order

1. **Configured API key**: if `[api] key` (or `QUILL_API_KEY`) holds a real key, Quill uses the API. The placeholder `YOUR_API_KEY_HERE` and empty values do not count.
2. **Local Ollama**: otherwise Quill probes the Ollama server once (`/api/tags`, with a 400 ms connect timeout and 1.5 s total). It uses Ollama if the server answers and a usable model exists. Usable means a non-embedding model, that is, a name that does not contain `embed` (case-insensitive).
3. **Offline**: otherwise Quill uses the offline grammar engine.

Probing is skipped when `[ollama] enabled = false`. In `offline` mode there is no probe at all.

### How the Ollama model is picked

1. The configured model (`[ollama] model` or `QUILL_OLLAMA_MODEL`), if it is installed. If it is set but not pulled, Ollama is treated as unusable (Quill does not silently pick another model).
2. Otherwise the first model that is currently running (`/api/ps`) and is not an embedding model.
3. Otherwise the first installed model that is not an embedding model.

Ollama requests use Quill's Anthropic-Messages client against `<ollama url>/v1/messages`. The API key is a fixed placeholder that Ollama ignores. Whole-request timeout is at least 120 seconds, because local models can be slow to load.

### What is sent where

- **API mode**: the selected text, your instruction and writing context go to `[api] url`.
- **Ollama mode**: the same content goes to `[ollama] url`, which defaults to your own machine (`http://127.0.0.1:11434`). If you point `QUILL_OLLAMA_URL` at a remote `https://` host, that host receives your text.
- **Offline mode**: nothing leaves your machine. `context.md` is not used, and offline results are not recorded in `history.jsonl`.

### Runtime fallback

In `auto` mode, if an AI request fails at run time, Quill prints `[quill] AI request failed (<reason>); falling back to offline grammar mode.` and runs the offline engine. In `ai` mode the same failure is an error.

### Offline hints and enabling AI

When auto mode falls back to offline because nothing is configured, Quill prints on stderr:

```text
[quill] offline grammar mode: no API key configured and no usable Ollama model found.
[quill] To enable AI rewrite: set [api] key in <config path> (or QUILL_API_KEY), or start Ollama (ollama serve; ollama pull <model>). Run 'quill status' for details.
```

If Ollama is reachable but unusable, a third line gives the reason, for example `model "x" is not pulled (run: ollama pull x)`.

To enable AI, do one of these:

- Set `key` under `[api]` in `config.toml`, or export `QUILL_API_KEY`.
- Run `ollama serve`, then `ollama pull <model>`.

### Precedence

For the mode: **CLI flag > environment > config file > default**.

- `--mode` / `--offline` (CLI)
- `QUILL_MODE` (environment; an invalid value is an error)
- `[general] mode` in `config.toml`
- `auto`

Other settings follow the same idea: environment variables such as `QUILL_API_KEY` or `QUILL_OLLAMA_URL` override the config file. See [Environment overrides](#environment-overrides).

### Per-command behaviour in offline mode

| Command | Offline behaviour |
|---|---|
| `quill check` | Runs a local self-check (`this is teh test.` must become `... the test ...`). No network. |
| `quill learn` | Exits 1: learning needs an AI provider and cannot run in `offline` mode. In `auto` mode with no provider, it also exits 1 and explains why. |
| `quill setup` | Mode-independent: installs or reports the hotkey as usual. |
| `quill status` | Shows `ollama: skipped (mode offline)` and makes no network request. |
| `quill --now` / `--stdin` | Use the grammar engine. Profiles are ignored with a warning. |

### `quill status`

`quill status` never sends an AI request. It reports the mode and its source, the API state, the Ollama probe (in `auto` and `ai` modes), the offline engine and dialect, the provider that would be used, and the macOS hotkey state. Sample output from a sealed temporary HOME (no config file, local Ollama running):

```text
Quill v0.4.0 status
mode: auto (source: default)
config: /tmp/qh-readme/.config/quill/config.toml (not found)
api: not configured (set [api] key in /tmp/qh-readme/.config/quill/config.toml or export QUILL_API_KEY)
ollama: reachable at http://127.0.0.1:11434; models: qwen3:30b, embeddinggemma-2:latest; selected: qwen3:30b
offline: available (harper-core, dialect american)
active: ollama (qwen3:30b)
hotkey: skhd installed; quill block in ~/.skhdrc absent
```

## Offline grammar mode

Offline mode is a conservative grammar and spelling fixer. It runs entirely on your machine.

### What it fixes

- Grammar and agreement (for example, `he go` to `he goes`, `a` / `an`).
- Punctuation and repeated words.
- Spacing and capitalization of sentence starts.
- Common spelling typos, for example `teh` to `the`.

Offline mode also fixes more common confusions and irregular verbs:

- More Harper rules are enabled, each restricted by rule name: `then` to `than` (after a comparative), `could of` to `could have`, `its` / `it's`, `they're` / `their` / `there`, `to` / `too` / `loose` / `lose`, `cant` to `can't`, and `the affect of` to `the effect of`.
- Spelling uses a built-in list of the 10,000 most frequent English words to choose between candidate fixes. A one-letter insertion to a common word is preferred (`arive` to `arrive`, `adress` to `address`; for 4-letter words the result must be in the top 1000, as in `wich` to `which`). A doubled letter may be dropped (`programm` to `program`, `wonderfull` to `wonderful`). Plain letter substitutions and word endings (`wonderfully`) are never preferred.
- A small table fixes regularised past forms and participles: `buyed` to `bought`, `eated` to `ate` (or `eaten` after `have`), `teached` to `taught`.
- `your welcome to` becomes `you're welcome to`, and a sentence-final `their` after `over`, `is`, `go` and similar words becomes `there`.
- These rules keep the same safety guards: protected spans, negative contractions and capitalization limits all still apply.

### How it works

It uses `harper-core`, the same linting engine that powers `harper-wasm`, compiled into Quill as native Rust. There is no network access and no model: the same input always gives the same output. Up to three passes run, each re-checking the previous result. Only edits that pass the safety policy are applied.

### Safety policy

Offline mode changes as little as possible:

- It never edits URLs, email addresses, code (inline code, fenced code blocks, indented code), command flags, paths, identifiers, `::` or `->` expressions, or words that look like identifiers (for example `camelCase`, words with digits).
- It never edits ALLCAPS words, mixed-case words, or capitalized words that are not in the dictionary (likely names).
- Only a fixed, named list of Harper rules is enabled: the mechanical ones (spelling, `teh` to `the`, sentence-start capitalization, the pronoun `I`, `a` / `an`, repeated words, double spaces, comma spacing, pronoun-verb agreement) plus the small set of confusion rules listed under "What it fixes". Every other Harper rule is off, so Quill does not expand abbreviations (`imo`, `asap`), rewrite other word choices, or change tense.
- It never turns a negative contraction written without an apostrophe into another word or splits it: `wont` stays `wont` or becomes `won't`, never `want`; `werent` and `arent` get an apostrophe, never a space. No edit may remove a negation.
- Spelling fixes are deliberately conservative, because a wrong "fix" is worse than a missed typo. A misspelled lowercase word is replaced only when (a) the fix is a rearrangement of the same letters (`teh` to `the`, `recieve` to `receive`), (b) the fix only adds an apostrophe (`dont` to `don't`), or (c) the word has at least seven letters, is one edit away from the fix, and keeps its first letter (`grammer` to `grammar`), or (d) a 4-6 letter word gains one letter to become a common word (see above), or (e) a word of seven or more letters drops one letter of a doubled pair to become a common word (see above). Everything else is left alone, so technical words and slang (`sudo`, `nginx`, `docker`, `kubectl`, `thx`, `pls`, `tmrw`) stay as you typed them.
- Capitalization is only changed for the pronoun `i` (and `i'm`, `i'll`, `i've`, `i'd`) and for the first word of a sentence or paragraph. Words in the middle of a sentence are never capitalized, so `docker` and `yaml` stay lowercase.
- It preserves line endings (LF and CRLF), tabs and indentation. Edits never touch line indentation or cross a line break.
- It is a prose tool. Lines that are not prose can still get comma spacing or spelling fixes (for example the CSV header `name,age,city` becomes `name, age, city`, and strings inside JSON are checked). Use code fences for text that must stay exactly as typed.
- It accepts at most 100000 characters per input. Very long inputs with many fixes can take several seconds.
- Like the AI path, the result is trimmed at both ends (leading indentation of the first line and the final line break are not kept).
- Offline results are never recorded in `history.jsonl`, because history is only used to learn AI writing style.
- `-p` / `--prompt` profiles are ignored in offline mode, with a warning.
- If `quill --now` finds nothing to change, the clipboard is left untouched and Quill prints `[quill] no changes`.

### Dialect

Spelling and agreement rules follow one English dialect, set in the config:

```toml
[offline]
# american | british | australian | canadian | indian
dialect = "american"
```

### Limitations

- It is a grammar checker, not a rewriter. It will not shorten, polish or change the tone of your text. Use an AI mode for that.
- It cannot fix every error, and it can miss context-dependent mistakes.
- Short sentences may not be capitalized. That is a heuristic inside `harper-core`, not something Quill adds.

## Configuration

### Config file location

| Platform | Path |
|---|---|
| macOS, Linux | `~/.config/quill/config.toml` |
| Windows | `%USERPROFILE%\.config\quill\config.toml` |

The config is optional for `auto` and `offline` modes. The config directory also holds `context.md`, `history.jsonl`, `quill.log`, `hotkey.sh` (macOS) and `prompts/`.

### Example config

This shows every field with its default value. The long default texts of `instruction` and `system_prompt` are shortened here; the full text is in [`config.sample.toml`](config.sample.toml). The only field without a usable default is `key`, which must be set to a real key before the API is used. In `ai` mode a missing or placeholder key is an error; in `auto` and `offline` modes it simply means the API is not used.

```toml
[general]
# "auto" (default) | "ai" | "offline". Overridden by QUILL_MODE and by --mode / --offline.
mode = "auto"

[api]
# Full Messages API endpoint. Anthropic-compatible gateways work too.
url = "https://api.anthropic.com/v1/messages"
# Required for the API. Empty, whitespace-only or placeholder values are rejected.
key = "YOUR_API_KEY_HERE"
model = "claude-sonnet-4-5"
anthropic_version = "2023-06-01"
# "x-api-key" (sends the key in the x-api-key header) or "bearer" (Authorization: Bearer).
auth = "x-api-key"
# Whole-request timeout in seconds. Allowed range 1..=600.
timeout_secs = 60
# Unsafe: disables TLS certificate verification. For local debugging only.
accept_invalid_certs = false

# Optional extra headers, for example for a gateway.
[api.headers]
# "X-Example-Header" = "value"

[ollama]
# Local Ollama server (Anthropic-compatible /v1/messages). Used in auto mode when no API key is set.
enabled = true
# Same URL rules as [api] url (plain http only for localhost).
url = "http://127.0.0.1:11434"
# Empty = pick a running (or first installed) non-embedding model automatically.
model = ""

[offline]
# Spelling/grammar dialect: american | british | australian | canadian | indian
dialect = "american"

[transform]
instruction = "Rephrase, humanize, shorten, and articulate the following text politely. Return ONLY the rewritten text."
system_prompt = "You are a text transformation assistant. Return ONLY the transformed text."
max_tokens = 4096
# Name of a prompt profile (prompts/<name>.txt) to use instead of `instruction`. Empty means unused.
prompt_override = ""

[shortcut]
# macOS: skhd syntax.
macos = "cmd + shift - c"
# Windows: informational only; the Windows hotkey is fixed to Ctrl+Shift+C.
windows = "ctrl + shift + c"
```

A copy of this example, with the same structure, is in the repository as [`config.sample.toml`](config.sample.toml).

Unknown fields are ignored and never sent to the endpoint. Older `user_type` and `user_name` fields are no longer used.

### Environment overrides

An environment variable overrides the matching config field. An empty value counts as unset.

| Variable | Overrides | Default when neither is set |
|---|---|---|
| `QUILL_API_URL` | `api.url` | `https://api.anthropic.com/v1/messages` |
| `QUILL_API_KEY` | `api.key` | none (no API) |
| `QUILL_MODEL` | `api.model` | `claude-sonnet-4-5` |
| `QUILL_MODE` | `general.mode` | `auto` (`auto`, `ai` or `offline`; an invalid value is an error) |
| `QUILL_OLLAMA_URL` | `ollama.url` | `http://127.0.0.1:11434` |
| `QUILL_OLLAMA_MODEL` | `ollama.model` | empty (auto-pick) |
| `QUILL_PROMPT` | Prompt profile name (see priority below) | none |

If the environment supplies the key, a config file is not required.

Note: launchd and skhd processes do not see your shell environment. For the hotkey, put the settings in the config file.

### Auth and headers

- `auth = "x-api-key"` sends `x-api-key: <key>`. `auth = "bearer"` sends `Authorization: Bearer <key>`. Any other value is a config error that names `auth`.
- `[api.headers]` adds extra headers. Header names are case-insensitive. Reserved headers are rejected at config load, including `authorization`, `x-api-key`, `anthropic-version`, `content-type`, `host` and `cookie`. Header values must not contain CR, LF or NUL.
- `api.url` must use `https`. Plain `http` is accepted only for `127.0.0.1`, `localhost` and `::1`. URLs with embedded credentials are rejected. The same rules apply to `ollama.url`.

## Prompt profiles

A profile is a plain text file `~/.config/quill/prompts/<name>.txt` that holds one instruction. `quill init` creates sample profiles in that directory, and you can add your own.

```bash
echo "Translate to Spanish. Return ONLY the translation." > ~/.config/quill/prompts/spanish.txt
quill --now -p spanish
```

Profile names may contain only letters, digits, `_` and `-`. Other names, including path separators, are rejected. An unknown profile name is an error (exit 1). Profiles apply to AI modes only.

Priority, highest first:

1. `--prompt NAME` / `-p NAME`
2. `QUILL_PROMPT`
3. `prompt_override` in `config.toml`
4. `instruction` in `config.toml`

## Writing context and learning

- `history.jsonl` stores before/after pairs from AI `--now` runs, one JSON object per line. Offline results are not stored.
- `quill learn` reads the history, updates `context.md` with a summary of your writing style, and clears `history.jsonl`. With no history, it reports that and exits 1.
- `context.md` is sent with each AI request as writing-style context. To stop using it, empty the file (Quill recreates a default file if it is missing, so deleting it does not turn the feature off).

Both files are local and private (see Privacy and security).

## macOS setup

`quill setup` does the following:

- Writes `~/.config/quill/hotkey.sh`. It copies the selection (System Events, Edit > Copy), waits briefly, then runs `quill --now` with stderr going to `~/.config/quill/quill.log`. If that run fails, it shows a notification that points to the log.
- Adds a managed block to `~/.skhdrc`, between these markers:

  ```text
  # >>> quill managed block v1 >>>
  cmd + shift - c : '<HOME>/.config/quill/hotkey.sh'
  # <<< quill managed block v1 <<<
  ```

  Only this block is ever modified. Every other line in `~/.skhdrc` is preserved. If the markers are unbalanced, duplicated or nested, `quill setup` exits 1 and leaves the file unchanged.
- Runs `skhd --restart-service` to load the change (a failure there is ignored).

To change the shortcut, edit `[shortcut] macos` in the config and run `quill setup` again. It only touches its own block.

skhd must be installed and on your `PATH`. If it is not, `quill setup` prints install instructions and still writes its files. skhd needs Accessibility permission (System Settings > Privacy & Security > Accessibility) to send the copy keystroke.

Failures are logged to `~/.config/quill/quill.log`.

## Privacy and security

- The selected text, your instruction and your writing context are sent to the endpoint you configure (API or Ollama). Choose an endpoint you trust. Offline mode sends nothing.
- `history.jsonl` keeps before/after text pairs of AI transforms locally until `quill learn` clears them.
- On Unix, files and directories are created private: the config directory is `0700`, and `config.toml`, `history.jsonl`, `context.md` and `quill.log` are `0600`. Quill repairs permissive modes on files it owns and notes the change on stderr. It refuses to write through symlinks for sensitive files.
- Never commit `config.toml`. It contains your key. The repository `.gitignore` excludes it, along with the other runtime files.
- TLS certificates are verified by default. `accept_invalid_certs = true` disables verification and is unsafe; use it only for local debugging.
- Plain `http` is allowed only for loopback hosts.
- Redirects are not followed. A 3xx response is an error and no redirected request is sent.
- Errors from the request path (HTTP status, timeouts, TLS, malformed responses) are generated locally and never include your key, the URL, request headers or response bodies. Config parse errors report only the line number and a sanitized parser message: the source line is never shown, and messages that would quote a value (for example a type mismatch) are replaced by a generic text.

## Troubleshooting

- **HTTP 401 (`API error: HTTP 401`)**: the key is wrong or is sent in the wrong way. Check `key` and `auth`. Run `quill check`.
- **Timeout**: the endpoint is slow or unreachable. Raise `timeout_secs` (maximum 600) or check the URL. For Ollama, the timeout is at least 120 seconds; a cold model load can still be slow.
- **Offline grammar mode although you expected AI**: run `quill status`. It shows the mode source, whether an API key was found, and the Ollama probe result (not reachable, no usable model, or the model is not pulled).
- **Hotkey does nothing (macOS)**: confirm skhd has Accessibility permission and that it is running (`skhd --start-service`). Then check `~/.config/quill/quill.log`.
- **Config errors**: run `quill check`. It names the offending field (for example `api.url`, `auth`, `timeout_secs`, `general.mode` or `offline.dialect`) without printing secret values.
- **Installer stops with "glibc >= 2.35 is required" or "libssl.so.3 not found"**: install a newer distribution runtime or OpenSSL 3, or build from source (see [Build from source](#build-from-source)).
- **Installer stops with "checksum mismatch"**: nothing was installed and any existing binary is untouched. Download again; if it persists, do not force the install.

## Releases and versioning

- Versions follow [Semantic Versioning](https://semver.org/). The version is the `version` field in the `[package]` section of `Cargo.toml`.
- [`CHANGELOG.md`](CHANGELOG.md) follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Each release has a `## [X.Y.Z] - YYYY-MM-DD` heading. Pending changes sit under `## [Unreleased]`.

### Automatic releases (maintainers)

Releases are automatic. Maintainers never run a release script and never push tags by hand.

- Every push or merge to `main` runs `.github/workflows/release.yml`. Its `plan` job decides whether to release.
- Docs-only changes (`*.md`, `LICENSE`, `.gitignore`) do not release, unless unreleased code changes are already pending.
- Putting `[skip release]` in the head commit message skips the release for that push.
- The version always increases by exactly one minor: `0.5.0` becomes `0.6.0`, then `0.7.0`. It is never a patch or major bump. The base is the higher of the highest stable `vX.Y.Z` tag and the `Cargo.toml` version. Prerelease and non-SemVer tags are ignored.
- Pipeline: the `verify` job runs the tests, then `build` produces binaries for five targets (Linux and macOS, x86_64 and aarch64, plus Windows). The Windows build is experimental and never blocks a release. Then `publish` runs.
- `publish` bumps `Cargo.toml`, `Cargo.lock` and `CHANGELOG.md` in a `chore(release): vX.Y.Z [skip ci]` commit pushed to `main` by `github-actions[bot]`. It tags that commit and creates the GitHub Release with the archives, `SHA256SUMS`, `install.sh` and `install.ps1`. It then writes the completion marker ref `refs/published/vX.Y.Z`.
- Release notes come from the `[Unreleased]` section of `CHANGELOG.md` when it has bullets. Otherwise they come from the commit subjects since the last tag. Write meaningful commit subjects, or curate `[Unreleased]` before merging.
- Recovery: if a run dies after pushing the tag, the next run (or Actions > Release > Run workflow) finishes the same release without bumping the version again.
- Owner override: the `recover_tag` input on Actions > Release > Run workflow finishes a specific tag.
- Deleting a release on purpose after it completed does not recreate it.
- Dry run: the `dry_run` input on Actions > Release > Run workflow builds everything and publishes nothing.
- Starting a new major (for example `1.0.0`): in a normal commit, set the version in `Cargo.toml` and `Cargo.lock` to the new base (for example `1.0.0`). The next automatic release is then `1.1.0`.
- After each automatic release, run `git pull` before your next push: the release commit lands on `main` ahead of your local copy.
- `main` must allow `github-actions[bot]` to push. Use no branch protection that requires pull requests, or add a ruleset bypass for the bot.

Asset names are `quill-<version>-<target>.tar.gz` (Unix) or `quill-<version>-<target>.zip` (Windows). Each archive contains the binary, `LICENSE`, `README.md` and `config.sample.toml` in a directory of the same name.

### Continuous integration

- `.github/workflows/ci.yml` runs on pushes to `main` and on pull requests:
  - **lint** (Ubuntu): `cargo fmt --all -- --check` and `cargo clippy --locked --all-targets -- -D warnings`.
  - **test** (Ubuntu, macOS, Windows): `cargo build --locked` and `cargo test --locked`. The Windows smoke test is experimental and cannot fail the job.
  - **scripts** (Ubuntu, macOS): `shellcheck` on `install.sh`, `scripts/*.sh` and `tests/*.sh`, then the installer tests (`tests/install_test.sh`), the release script tests (`tests/release_scripts_test.sh`) and the auto-release tests (`tests/auto_release_test.sh`).
- `.github/workflows/release.yml` handles releases, as described above.

## Development

```bash
cargo build --locked
cargo test --locked
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

`cargo test` runs a local mock server. It never reads or writes your real config, history or clipboard.

## License

MIT. See [LICENSE](LICENSE).
