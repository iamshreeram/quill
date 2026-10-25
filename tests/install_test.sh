#!/usr/bin/env bash
# shellcheck disable=SC2016,SC2329  # fake tool bodies are intentionally single-quoted; cleanup runs via trap
# Acceptance tests for install.sh (spec: .ram-status/quill-modes-20251025-2255/DESIGN.md Rev 4, Part B).
# Hermetic: fake release server on 127.0.0.1 (python3), fake release archives containing a shell-script
# "quill", sealed PATH (symlinked POSIX tools + test fakes), per-test HOME. Never touches the real HOME,
# real PATH tools such as git/cargo/brew/skhd, or the network.
# Usage: tests/install_test.sh            (run from anywhere)
set -u

HERE="$(cd "$(dirname "$0")" && pwd)"
INSTALL_SH="$HERE/../install.sh"
ROOT="$(mktemp -d "${TMPDIR:-/tmp}/quill-install-test.XXXXXX")"
PASS=0
FAILED=0
FAILED_NAMES=""
SERVER_PID=""

cleanup() {
  [ -n "$SERVER_PID" ] && kill "$SERVER_PID" 2>/dev/null
  rm -rf "$ROOT"
}
trap cleanup EXIT

command -v python3 >/dev/null 2>&1 || { echo "SKIP: python3 required for the fake release server"; exit 0; }
[ -f "$INSTALL_SH" ] || { echo "FAIL: $INSTALL_SH does not exist"; exit 1; }

TOOLS="curl tar shasum sha256sum uname mktemp mv cp chmod mkdir rm cat sed awk grep head tail tr cut dirname basename id sh bash env sort date ls ln wc printf test find readlink getconf cmp tee touch stat ldconfig which expr uniq xargs od"

# ---------- fixtures ----------
REL="$ROOT/releases"        # served by the fake server: download/<tag>/<assets>, LATEST
TARGETS="aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu aarch64-unknown-linux-gnu"

make_fake_quill() { # <path> <version> <marker>
  cat > "$1" <<EOF
#!/bin/sh
# marker:$3
[ -n "\${QUILL_TEST_CALLS:-}" ] && echo "\$*" >> "\$QUILL_TEST_CALLS"
case "\$1" in
  --version|-V) echo "quill $2" ;;
  init) mkdir -p "\$HOME/.config/quill"; echo "# fake" > "\$HOME/.config/quill/config.toml" ;;
esac
exit 0
EOF
  chmod 755 "$1"
}

sha_of() {
  if command -v shasum >/dev/null 2>&1; then shasum -a 256 "$1" | awk '{print $1}'; else sha256sum "$1" | awk '{print $1}'; fi
}

make_release() { # <version> <marker>   builds all target archives + SHA256SUMS
  local v="$1" marker="$2" d="$REL/download/v$1" t w
  rm -rf "$d"; mkdir -p "$d"
  : > "$d/SHA256SUMS"
  for t in $TARGETS; do
    w="$ROOT/stage/quill-$v-$t"; rm -rf "$w"; mkdir -p "$w"
    make_fake_quill "$w/quill" "$v" "$marker"
    echo "MIT" > "$w/LICENSE"; echo "readme" > "$w/README.md"; echo "# sample" > "$w/config.sample.toml"
    tar -czf "$d/quill-$v-$t.tar.gz" -C "$ROOT/stage" "quill-$v-$t"
    echo "$(sha_of "$d/quill-$v-$t.tar.gz")  quill-$v-$t.tar.gz" >> "$d/SHA256SUMS"
  done
  echo "$v" > "$REL/LATEST"
}

start_server() {
  mkdir -p "$REL"
  cat > "$ROOT/server.py" <<'PY'
import http.server, os, sys
root = sys.argv[1]
class H(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **k):
        super().__init__(*a, directory=root, **k)
    def route(self, head):
        p = self.path.split('?')[0].rstrip('/')
        if p.endswith('/latest'):
            tag = 'v' + open(os.path.join(root, 'LATEST')).read().strip()
            if os.path.exists(os.path.join(root, 'BADTAG')):
                tag = 'not-a-version'
            self.send_response(302); self.send_header('Location', '/tag/' + tag)
            self.send_header('Content-Length', '0'); self.end_headers(); return True
        if p.startswith('/tag/'):
            self.send_response(200); self.send_header('Content-Length', '0'); self.end_headers(); return True
        if '/v7.7.7/' in p:
            self.send_response(500); self.send_header('Content-Length', '0'); self.end_headers(); return True
        return False
    def do_GET(self):
        if not self.route(False): super().do_GET()
    def do_HEAD(self):
        if not self.route(True): super().do_HEAD()
    def log_message(self, *a): pass
srv = http.server.ThreadingHTTPServer(('127.0.0.1', 0), H)
open(sys.argv[2], 'w').write(str(srv.server_address[1]))
srv.serve_forever()
PY
  python3 "$ROOT/server.py" "$REL" "$ROOT/port" &
  SERVER_PID=$!
  local i=0
  while [ ! -s "$ROOT/port" ] && [ $i -lt 100 ]; do sleep 0.05; i=$((i + 1)); done
  [ -s "$ROOT/port" ] || { echo "FAIL: fake server did not start"; exit 1; }
  BASE="http://127.0.0.1:$(cat "$ROOT/port")"
}

# ---------- per-test sandbox ----------
T=""       # sandbox dir
H=""       # HOME
SYS=""     # symlinked POSIX tools
FAKES=""   # test fakes dir (first on PATH)
CALLS=""   # file collecting fake quill / tool calls
OUT=""; ERR=""; CODE=0

new_test() { # <name> [noshasum]
  CUR="$1"
  T="$ROOT/t-$1"; rm -rf "$T"; mkdir -p "$T/home" "$T/sys" "$T/fakes"
  H="$T/home"; SYS="$T/sys"; FAKES="$T/fakes"; CALLS="$T/calls.log"; : > "$CALLS"
  local tool p
  for tool in $TOOLS; do
    case "${2:-}:$tool" in noshasum:shasum|noshasum:sha256sum) continue ;; esac
    p="$(command -v "$tool" 2>/dev/null)" && [ -x "$p" ] && ln -s "$p" "$SYS/$tool"
  done
}

fake_tool() { # <name> <script-body>   logs invocation to CALLS as "<name> <args>"
  printf '#!/bin/sh\necho "%s $*" >> "%s"\n%s\n' "$1" "$CALLS" "$2" > "$FAKES/$1"
  chmod 755 "$FAKES/$1"
}

run_install() { # [VAR=val ...]   -> OUT ERR CODE   (stdin = /dev/null)
  OUT="$T/out.txt"; ERR="$T/err.txt"
  env -i HOME="$H" PATH="$FAKES:$SYS" SHELL="${TEST_SHELL:-/bin/zsh}" TMPDIR="$T" \
    QUILL_BASE_URL="$BASE" QUILL_NONINTERACTIVE=1 QUILL_TEST_CALLS="$CALLS" \
    QUILL_UNAME_S=Darwin QUILL_UNAME_M=arm64 \
    "$@" sh "$INSTALL_SH" </dev/null >"$OUT" 2>"$ERR"
  CODE=$?
}

all_output() { cat "$OUT" "$ERR"; }
fail() { echo "  FAIL [$CUR]: $*"; echo "  --- stdout"; sed 's/^/  | /' "$OUT" 2>/dev/null | head -30; echo "  --- stderr"; sed 's/^/  | /' "$ERR" 2>/dev/null | head -30; FAILED=$((FAILED + 1)); FAILED_NAMES="$FAILED_NAMES $CUR"; return 1; }
check_code() { [ "$CODE" -eq "$1" ] || fail "expected exit $1, got $CODE"; }
check_nonzero() { [ "$CODE" -ne 0 ] || fail "expected non-zero exit"; }
check_out_has() { all_output | grep -qi -- "$1" || fail "output lacks '$1'"; }
check_out_lacks() { if all_output | grep -qi -- "$1"; then fail "output unexpectedly has '$1'"; fi; }
check_file() { [ -e "$1" ] || fail "missing file $1"; }
check_nofile() { [ ! -e "$1" ] || fail "unexpected file $1"; }
check_marker() { grep -q "marker:$2" "$1" 2>/dev/null || fail "$1 does not carry marker $2"; }
check_calls_has() { grep -q -- "$1" "$CALLS" || fail "no recorded call matching '$1'"; }
check_calls_lacks() { if grep -q -- "$1" "$CALLS"; then fail "unexpected recorded call matching '$1'"; fi; }
done_test() { if ! echo "$FAILED_NAMES" | grep -qw -- "$CUR"; then PASS=$((PASS + 1)); echo "ok   $CUR"; else echo "FAIL $CUR"; fi; }

BIN() { echo "$H/.local/bin/quill"; }

# ---------- tests ----------
start_server
make_release 9.9.9 NEW

i01_happy_path_and_path_hint_and_no_rc_edits() {
  new_test i01
  run_install QUILL_VERSION=9.9.9
  check_code 0
  check_file "$(BIN)"
  [ "$("$(BIN)" --version)" = "quill 9.9.9" ] || fail "installed binary version"
  check_marker "$(BIN)" NEW
  [ -x "$(BIN)" ] || fail "binary not executable"
  check_out_has 'export PATH='
  for rc in .zshrc .bashrc .bash_profile .profile .zprofile .config/fish/config.fish; do check_nofile "$H/$rc"; done
  check_calls_has '^--version'   # staged binary verified before install (fake quill logs raw args)
  check_calls_has '^status'
  done_test
}

i01b_fish_hint_and_piped_script() {
  new_test i01b
  OUT="$T/out.txt"; ERR="$T/err.txt"
  env -i HOME="$H" PATH="$FAKES:$SYS" SHELL=/usr/bin/fish TMPDIR="$T" QUILL_BASE_URL="$BASE" \
    QUILL_NONINTERACTIVE=1 QUILL_TEST_CALLS="$CALLS" QUILL_UNAME_S=Darwin QUILL_UNAME_M=arm64 QUILL_VERSION=9.9.9 \
    bash < "$INSTALL_SH" >"$OUT" 2>"$ERR"
  CODE=$?
  check_code 0
  check_file "$(BIN)"
  check_out_has 'fish_add_path'
  done_test
}

i02_checksum_mismatch_aborts_and_preserves_existing_binary() {
  new_test i02
  make_release 9.9.9 OLD
  run_install QUILL_VERSION=9.9.9
  check_code 0; check_marker "$(BIN)" OLD
  make_release 9.9.9 NEW
  # corrupt the checksum of the macOS arm64 archive
  sed 's/^[0-9a-f]\{8\}/00000000/' "$REL/download/v9.9.9/SHA256SUMS" > "$T/sums" && cp "$T/sums" "$REL/download/v9.9.9/SHA256SUMS"
  fake_tool git 'exit 0'; fake_tool cargo 'exit 0'
  run_install QUILL_VERSION=9.9.9
  check_nonzero
  check_marker "$(BIN)" OLD
  check_out_has 'checksum'
  check_calls_lacks '^git '; check_calls_lacks '^cargo '
  make_release 9.9.9 NEW
  done_test
}

i03_latest_is_resolved_to_one_exact_tag() {
  new_test i03
  make_release 9.9.9 NEW
  run_install
  check_code 0
  [ "$("$(BIN)" --version)" = "quill 9.9.9" ] || fail "latest did not install 9.9.9"
  # garbage redirect target is refused
  touch "$REL/BADTAG"
  rm -rf "$H/.local"
  run_install
  check_nonzero
  check_nofile "$(BIN)"
  rm -f "$REL/BADTAG"
  # v-prefixed explicit version is accepted and equivalent
  run_install QUILL_VERSION=v9.9.9
  check_code 0
  done_test
}

i04_unsupported_platforms_fail_clearly() {
  new_test i04
  run_install QUILL_VERSION=9.9.9 QUILL_UNAME_S=Plan9 QUILL_UNAME_M=x86_64
  check_nonzero; check_out_has 'unsupported'; check_nofile "$(BIN)"
  run_install QUILL_VERSION=9.9.9 QUILL_UNAME_S=Linux QUILL_UNAME_M=riscv64 QUILL_GLIBC_VERSION=2.39 QUILL_SKIP_LIBSSL_CHECK=1
  check_nonzero; check_out_has 'unsupported'; check_nofile "$(BIN)"
  done_test
}

i05_linux_preflight_refuses_old_glibc_without_touching_existing_install() {
  new_test i05
  make_release 9.9.9 OLD
  run_install QUILL_VERSION=9.9.9
  check_code 0; check_marker "$(BIN)" OLD
  make_release 9.9.9 NEW
  run_install QUILL_VERSION=9.9.9 QUILL_UNAME_S=Linux QUILL_UNAME_M=x86_64 QUILL_GLIBC_VERSION=2.31 QUILL_SKIP_LIBSSL_CHECK=1
  check_nonzero; check_out_has 'glibc'; check_out_has 'cargo install'
  check_marker "$(BIN)" OLD
  # new enough glibc passes; target asset for linux is used
  run_install QUILL_VERSION=9.9.9 QUILL_UNAME_S=Linux QUILL_UNAME_M=x86_64 QUILL_GLIBC_VERSION=2.35 QUILL_SKIP_LIBSSL_CHECK=1
  check_code 0; check_marker "$(BIN)" NEW
  # missing libssl.so.3 is refused when the check is not skipped (sealed PATH has no ldconfig output with libssl.so.3)
  fake_tool ldconfig 'echo "	libc.so.6 (libc6,x86-64) => /lib/x86_64-linux-gnu/libc.so.6"'
  run_install QUILL_VERSION=9.9.9 QUILL_UNAME_S=Linux QUILL_UNAME_M=aarch64 QUILL_GLIBC_VERSION=2.39
  check_nonzero; check_out_has 'libssl'
  done_test
}

i06_missing_asset_without_cargo_gives_rustup_instructions() {
  new_test i06
  run_install QUILL_VERSION=9.8.8
  check_nonzero; check_out_has 'rustup'; check_nofile "$(BIN)"
  done_test
}

i07_source_fallback_builds_the_exact_tag_locked_and_asserts_version() {
  new_test i07
  fake_tool rustc 'echo "rustc 1.99.0 (fake)"'
  fake_tool git 'if [ "$1" = clone ]; then
  for last; do :; done
  mkdir -p "$last"; printf "[package]\nname=\"quill\"\nversion=\"9.8.8\"\nrust-version=\"1.90\"\n" > "$last/Cargo.toml"
fi
exit 0'
  make_fake_quill "$T/tpl-good" 9.8.8 SRC
  make_fake_quill "$T/tpl-bad" 0.0.1 SRC
  fake_tool cargo "if [ \"\$1\" = build ]; then
  mkdir -p target/release
  cp \"\${FAKE_TEMPLATE:-$T/tpl-good}\" target/release/quill
fi
exit 0"
  run_install QUILL_VERSION=9.8.8
  check_code 0
  check_calls_has '^git clone .*--branch v9.8.8'
  check_calls_has '^cargo build .*--locked'
  check_marker "$(BIN)" SRC
  [ "$("$(BIN)" --version)" = "quill 9.8.8" ] || fail "source build version"
  # wrong version built -> refuse and keep the existing binary
  make_release 9.9.9 OLD; run_install QUILL_VERSION=9.9.9; check_marker "$(BIN)" OLD
  run_install QUILL_VERSION=9.8.8 QUILL_FROM_SOURCE=1 FAKE_TEMPLATE="$T/tpl-bad"
  check_nonzero; check_marker "$(BIN)" OLD
  make_release 9.9.9 NEW
  done_test
}

i08_no_source_fallback_on_non_404_failures() {
  new_test i08
  fake_tool git 'exit 0'; fake_tool cargo 'exit 0'; fake_tool rustc 'echo "rustc 1.99.0"'
  run_install QUILL_VERSION=7.7.7      # server answers HTTP 500 for this version
  check_nonzero
  check_calls_lacks '^git '; check_calls_lacks '^cargo '
  check_nofile "$(BIN)"
  done_test
}

i09_rerun_is_idempotent_and_never_overwrites_config() {
  new_test i09
  run_install QUILL_VERSION=9.9.9
  check_code 0
  check_calls_has '^init'                       # config absent -> init called
  : > "$CALLS"
  mkdir -p "$H/.config/quill"; echo "KEEP-ME" > "$H/.config/quill/config.toml"
  run_install QUILL_VERSION=9.9.9
  check_code 0
  check_calls_lacks '^init'                     # config present -> init NOT called
  [ "$(cat "$H/.config/quill/config.toml")" = "KEEP-ME" ] || fail "config.toml was modified"
  done_test
}

i10_macos_dependency_and_setup_flow() {
  new_test i10
  fake_tool brew 'exit 0'
  # missing skhd, non-interactive, no consent -> inform only
  run_install QUILL_VERSION=9.9.9
  check_code 0
  check_calls_lacks '^brew '
  check_out_has 'brew install koekeishiya/formulae/skhd'
  # consent via QUILL_YES -> brew install
  run_install QUILL_VERSION=9.9.9 QUILL_YES=1
  check_code 0
  check_calls_has '^brew install .*skhd'
  # skhd present -> no brew, setup run, service started, accessibility reminder
  : > "$CALLS"
  fake_tool skhd 'exit 0'
  run_install QUILL_VERSION=9.9.9
  check_code 0
  check_calls_lacks '^brew '
  check_calls_has '^setup'
  check_calls_has '^skhd .*--start-service'
  check_out_has 'accessibility'
  # QUILL_NO_SETUP skips setup
  : > "$CALLS"
  run_install QUILL_VERSION=9.9.9 QUILL_NO_SETUP=1
  check_code 0
  check_calls_lacks '^setup'
  done_test
}

i11_linux_clipboard_and_hotkey_hints() {
  new_test i11
  run_install QUILL_VERSION=9.9.9 QUILL_UNAME_S=Linux QUILL_UNAME_M=x86_64 QUILL_GLIBC_VERSION=2.39 QUILL_SKIP_LIBSSL_CHECK=1
  check_code 0
  check_out_has 'xclip'
  check_out_has 'quill --now'
  check_calls_lacks '^setup'                    # no skhd flow on Linux
  done_test
}

i12_truncated_script_never_installs() {
  new_test i12
  local total n cut
  total=$(wc -c < "$INSTALL_SH")
  for n in 10 25 50 75 90; do
    cut=$((total * n / 100))
    head -c "$cut" "$INSTALL_SH" > "$T/trunc.sh"
    rm -rf "$H/.local"
    env -i HOME="$H" PATH="$FAKES:$SYS" SHELL=/bin/zsh TMPDIR="$T" QUILL_BASE_URL="$BASE" QUILL_NONINTERACTIVE=1 \
      QUILL_TEST_CALLS="$CALLS" QUILL_UNAME_S=Darwin QUILL_UNAME_M=arm64 QUILL_VERSION=9.9.9 \
      sh "$T/trunc.sh" </dev/null >"$T/out.txt" 2>"$T/err.txt"
    OUT="$T/out.txt"; ERR="$T/err.txt"
    [ ! -e "$(BIN)" ] || fail "truncated script (${n}%) installed a binary"
  done
  done_test
}

i13_custom_install_dir_and_no_path_hint_when_already_on_path() {
  new_test i13
  mkdir -p "$T/custom"
  run_install QUILL_VERSION=9.9.9 QUILL_INSTALL_DIR="$T/custom/bin" PATH="$T/custom/bin:$FAKES:$SYS"
  check_code 0
  check_file "$T/custom/bin/quill"
  check_out_lacks 'export PATH='
  done_test
}

i14_missing_sha_tool_is_an_error() {
  new_test i14 noshasum
  run_install QUILL_VERSION=9.9.9
  check_nonzero; check_out_has 'sha256'; check_nofile "$(BIN)"
  done_test
}

i15_syntax_and_shellcheck() {
  new_test i15
  OUT="$T/out.txt"; ERR="$T/err.txt"; : > "$OUT"; : > "$ERR"
  sh -n "$INSTALL_SH" 2>"$ERR" || fail "sh -n failed"
  bash -n "$INSTALL_SH" 2>"$ERR" || fail "bash -n failed"
  head -1 "$INSTALL_SH" | grep -q '^#!' || fail "missing shebang"
  tail -n 3 "$INSTALL_SH" | grep -q 'main' || fail "main is not invoked on the last lines (truncation safety)"
  if command -v shellcheck >/dev/null 2>&1; then shellcheck -s sh "$INSTALL_SH" >"$OUT" 2>&1 || fail "shellcheck findings"; fi
  done_test
}

i01_happy_path_and_path_hint_and_no_rc_edits
i01b_fish_hint_and_piped_script
i02_checksum_mismatch_aborts_and_preserves_existing_binary
i03_latest_is_resolved_to_one_exact_tag
i04_unsupported_platforms_fail_clearly
i05_linux_preflight_refuses_old_glibc_without_touching_existing_install
i06_missing_asset_without_cargo_gives_rustup_instructions
i07_source_fallback_builds_the_exact_tag_locked_and_asserts_version
i08_no_source_fallback_on_non_404_failures
i09_rerun_is_idempotent_and_never_overwrites_config
i10_macos_dependency_and_setup_flow
i11_linux_clipboard_and_hotkey_hints
i12_truncated_script_never_installs
i13_custom_install_dir_and_no_path_hint_when_already_on_path
i14_missing_sha_tool_is_an_error
i15_syntax_and_shellcheck

echo
echo "passed: $PASS   failed: $FAILED"
[ "$FAILED" -eq 0 ] || { echo "failed tests:$FAILED_NAMES"; exit 1; }
exit 0
