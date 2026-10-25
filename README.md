# Quill

Quill is a lightweight system-wide text transformer. You copy text, press a shortcut, and the text on your clipboard is replaced with a rewritten version that you can paste. The rewrite is produced by an LLM behind an Anthropic-Messages-compatible HTTP endpoint that you configure. Quill also keeps optional local writing-style context, learned from your before/after pairs, that is sent along with each request.

## How it works

1. Select text and copy it (Cmd+C on macOS, Ctrl+C elsewhere).
2. Press the transform shortcut (Cmd+Shift+C on macOS, Ctrl+Shift+C on Windows). Quill reads the clipboard, sends the text with your instruction and writing context to the endpoint, and writes the result back to the clipboard.
3. Paste the result (Cmd+V / Ctrl+V).

Quill can also read from standard input (`echo 'text' | quill --stdin`), which needs no clipboard.

## Platform support

| Platform | Status | Clipboard | Hotkey |
|---|---|---|---|
| macOS | Supported and tested | `pbpaste` / `pbcopy` | skhd (see `quill setup`) |
| Windows | EXPERIMENTAL: not built or tested by the author, no CI coverage | PowerShell `Get-Clipboard` / `Set-Clipboard` | Fixed to Ctrl+Shift+C; the `[shortcut] windows` setting is informational only |
| Linux | EXPERIMENTAL: builds in CI; the clipboard code is untested on real desktops | `wl-paste`/`wl-copy` on Wayland, `xclip` on X11 (`xsel` as fallback) | Bind your desktop shortcut to `quill --now` |

On Linux you need `wl-clipboard` (Wayland) or `xclip` (X11) installed. On Windows, running `quill` with no arguments starts the hotkey listener.

Continuous integration (see `.github/workflows/ci.yml`) builds and tests on Ubuntu and macOS. Windows is not covered by CI.

## Requirements

- A Rust toolchain (stable), to build from source.
- An Anthropic-Messages-compatible endpoint and an API key.
- macOS only, for the global hotkey: [skhd](https://github.com/koekeishiya/skhd), for example `brew install koekeishiya/formulae/skhd`.

## Install

```bash
git clone https://github.com/iamshreeram/quill.git
cd quill
cargo build --release
```

The binary is `target/release/quill`. Either put it on your `PATH`, for example:

```bash
cp target/release/quill ~/.local/bin/
```

or install it with Cargo:

```bash
cargo install --path .
```

## Quick start

1. Create the config and prompt directory. If you set `QUILL_API_URL` and `QUILL_API_KEY` in the environment first, `init` writes them into the new config:

   ```bash
   quill init
   ```

2. Edit `~/.config/quill/config.toml`: set `key` (and `url`, if you use a gateway).

3. Verify the endpoint:

   ```bash
   quill check
   ```

4. On macOS, install the hotkey, then use it: select text, press Cmd+C, then Cmd+Shift+C, then Cmd+V.

   ```bash
   quill setup
   ```

Other ways to run it:

```bash
echo 'Please send me the file asap thx' | quill --stdin
quill --now -p formal
```

## Commands

| Command | Description |
|---|---|
| `quill init` | Create the config file, the `prompts/` directory and sample profiles. Fails if the config already exists. |
| `quill setup` | macOS: write `hotkey.sh` and the managed skhd block (see below). |
| `quill check` | Validate the config and send one small test request. Prints `OK: <model> responded` on success. |
| `quill --stdin [-p NAME]` | Read UTF-8 text from stdin, print the transformed text to stdout. Does not touch the clipboard or history. |
| `quill --now` / `quill -n [-p NAME]` | Transform the clipboard text and write the result back to the clipboard. Records a history pair. |
| `-p NAME` / `--prompt NAME` | Use the prompt profile `NAME` for this run. |
| `quill learn` | Analyze `history.jsonl`, update `context.md`, and clear the history. |
| `quill --help` (`-h`, `help`) | Print usage. Running `quill` with no arguments also prints usage on macOS and Linux. |
| `quill --version` (`-V`) | Print `quill <version>`. |

Exit codes: `0` on success, `1` on any error.

## Configuration

### Config file location

| Platform | Path |
|---|---|
| macOS, Linux | `~/.config/quill/config.toml` |
| Windows | `%USERPROFILE%\.config\quill\config.toml` |

The config directory also holds `context.md`, `history.jsonl`, `quill.log`, `hotkey.sh` (macOS) and `prompts/`.

### Example config

This shows every field with its default. The only field without a usable default is `key`, which must be set to a real key.

```toml
[api]
# Full Messages-API endpoint. Anthropic-compatible gateways work too.
url = "https://api.anthropic.com/v1/messages"
# Required. Empty, whitespace-only or placeholder values are rejected.
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

| Variable | Overrides |
|---|---|
| `QUILL_API_URL` | `api.url` |
| `QUILL_API_KEY` | `api.key` |
| `QUILL_MODEL` | `api.model` |
| `QUILL_PROMPT` | Prompt profile name (see priority below) |

Environment values win over the config file. If the environment supplies the key, a config file is not required.

Note: launchd and skhd processes do not see your shell environment. For the hotkey, put the settings in the config file.

### Auth and headers

- `auth = "x-api-key"` sends `x-api-key: <key>`. `auth = "bearer"` sends `Authorization: Bearer <key>`. Any other value is a config error that names `auth`.
- `[api.headers]` adds extra headers. Header names are case-insensitive. Reserved headers are rejected at config load, including `authorization`, `x-api-key`, `anthropic-version`, `content-type`, `host` and `cookie`. Header values must not contain CR, LF or NUL.
- `api.url` must use `https`. Plain `http` is accepted only for `127.0.0.1`, `localhost` and `::1`. URLs with embedded credentials are rejected.

## Prompt profiles

A profile is a plain text file `~/.config/quill/prompts/<name>.txt` that holds one instruction. `quill init` creates sample profiles in that directory, and you can add your own.

```bash
echo "Translate to Spanish. Return ONLY the translation." > ~/.config/quill/prompts/spanish.txt
quill --now -p spanish
```

Profile names may contain only letters, digits, `_` and `-`. Other names, including path separators, are rejected. An unknown profile name is an error (exit 1).

Priority, highest first:

1. `--prompt NAME` / `-p NAME`
2. `QUILL_PROMPT`
3. `prompt_override` in `config.toml`
4. `instruction` in `config.toml`

## Writing context and learning

- `history.jsonl` stores before/after pairs from `--now` runs, one JSON object per line.
- `quill learn` reads the history, updates `context.md` with a summary of your writing style, and clears `history.jsonl`. With no history, it reports that and exits 1.
- `context.md` is sent with each request as writing-style context. To stop using it, empty the file (Quill recreates a default file if it is missing, so deleting it does not turn the feature off).

Both files are local and private (see Privacy and security).

## macOS setup

`quill setup` does the following:

- Writes `~/.config/quill/hotkey.sh`. It copies the selection (System Events, Edit > Copy), waits briefly, then runs `quill --now` with stderr going to `~/.config/quill/quill.log`. If that run fails, it shows a notification that points to the log.
- Adds a managed block to `~/.skhdrc`, between these markers:

  ```
  # >>> quill managed block v1 >>>
  cmd + shift - c : '<HOME>/.config/quill/hotkey.sh'
  # <<< quill managed block v1 <<<
  ```

  Only this block is ever modified. Every other line in `~/.skhdrc` is preserved. If the markers are unbalanced, duplicated or nested, `quill setup` exits 1 and leaves the file unchanged.
- Runs `skhd --restart-service` to load the change (a failure there is ignored).

skhd must be installed and on your `PATH`. If it is not, `quill setup` prints install instructions and still writes its files. skhd needs Accessibility permission (System Settings > Privacy & Security > Accessibility) to send the copy keystroke.

Failures are logged to `~/.config/quill/quill.log`.

## Privacy and security

- The selected text, your instruction and your writing context are sent to the endpoint you configure. Choose an endpoint you trust.
- `history.jsonl` keeps before/after text pairs locally until `quill learn` clears them.
- On Unix, files and directories are created private: the config directory is `0700`, and `config.toml`, `history.jsonl`, `context.md` and `quill.log` are `0600`. Quill repairs permissive modes on files it owns and notes the change on stderr. It refuses to write through symlinks for sensitive files.
- Never commit `config.toml`. It contains your key. The repository `.gitignore` excludes it, along with the other runtime files.
- TLS certificates are verified by default. `accept_invalid_certs = true` disables verification and is unsafe; use it only for local debugging.
- Plain `http` is allowed only for loopback hosts.
- Redirects are not followed. A 3xx response is an error and no redirected request is sent.
- Errors from the request path (HTTP status, timeouts, TLS, malformed responses) are generated locally and never include your key, the URL, request headers or response bodies. Config parse errors report only the line number and the parser's message, never the source line.

## Troubleshooting

- **HTTP 401 (`API error: HTTP 401`)**: the key is wrong or is sent in the wrong way. Check `key` and `auth`. Run `quill check`.
- **Timeout**: the endpoint is slow or unreachable. Raise `timeout_secs` (maximum 600) or check the URL.
- **Hotkey does nothing (macOS)**: confirm skhd has Accessibility permission and that it is running. Then check `~/.config/quill/quill.log`.
- **Config errors**: run `quill check`. It names the offending field (for example `api.url`, `auth` or `timeout_secs`) without printing secret values.

## Development

```bash
cargo build --locked
cargo test --locked
```

`cargo test` runs a local mock server. It never reads or writes your real config, history or clipboard.

## License

MIT. See [LICENSE](LICENSE).
