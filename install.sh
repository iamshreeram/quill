#!/bin/sh
# install.sh -- install quill (https://github.com/iamshreeram/quill) and run its setup.
#   curl -fsSL https://github.com/iamshreeram/quill/releases/latest/download/install.sh | sh
#
# Knobs: QUILL_VERSION QUILL_INSTALL_DIR QUILL_BASE_URL QUILL_REPO_URL QUILL_NO_SETUP
#        QUILL_YES QUILL_NONINTERACTIVE QUILL_FROM_SOURCE
# Test hooks: QUILL_UNAME_S QUILL_UNAME_M QUILL_GLIBC_VERSION QUILL_SKIP_LIBSSL_CHECK
#
# Every piece of logic lives in a function and main is the last line, so a truncated
# download never executes partial logic. The script never reads stdin; child commands
# get </dev/null so they cannot consume the rest of a piped script.
set -eu

say() { printf '%s\n' "$*"; }
warn() { printf 'warning: %s\n' "$*" >&2; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

# version_ge <have> <need>: exit 0 when dotted version <have> >= <need> (numeric per component)
version_ge() {
  awk -v have="$1" -v need="$2" 'BEGIN {
    n = split(need, nd, "."); h = split(have, hd, ".")
    for (i = 1; i <= n; i++) {
      hv = (i <= h ? hd[i] : 0) + 0
      nv = nd[i] + 0
      if (hv > nv) exit 0
      if (hv < nv) exit 1
    }
    exit 0
  }'
}

is_release_tag() {
  printf '%s\n' "$1" | grep -Eq '^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'
}

setup_env() {
  BASE=${QUILL_BASE_URL:-https://github.com/iamshreeram/quill/releases}
  BASE=${BASE%/}
  REPO_URL=${QUILL_REPO_URL:-https://github.com/iamshreeram/quill.git}
  INSTALL_DIR=${QUILL_INSTALL_DIR:-$HOME/.local/bin}
  FROM_SOURCE=${QUILL_FROM_SOURCE:-0}
  WORK=""
  NEW_TMP=""
  HTTPS_ONLY=1
  case "$BASE" in
    https://*) ;;
    *) HTTPS_ONLY=0; warn "QUILL_BASE_URL is not https: $BASE" ;;
  esac
}

# curl_secure <args>: all network fetches go through here. With an https base, protocol
# restrictions apply to the initial request AND every redirect (no http downgrade).
curl_secure() {
  if [ "$HTTPS_ONLY" = 1 ]; then
    curl --proto '=https' --tlsv1.2 --proto-redir '=https' "$@"
  else
    curl "$@"
  fi
}

cleanup() {
  if [ -n "${NEW_TMP:-}" ]; then rm -f "$NEW_TMP"; fi
  if [ -n "${WORK:-}" ]; then rm -rf "$WORK"; fi
}

prepare_workdir() {
  WORK=$(mktemp -d "${TMPDIR:-/tmp}/quill-install.XXXXXX") || die "could not create a private temp directory"
  trap cleanup EXIT
  trap 'exit 1' INT TERM HUP
}

detect_target() {
  OS=${QUILL_UNAME_S:-$(uname -s)}
  ARCH=${QUILL_UNAME_M:-$(uname -m)}
  case "$OS:$ARCH" in
    Darwin:arm64|Darwin:aarch64) TARGET=aarch64-apple-darwin ;;
    Darwin:x86_64) TARGET=x86_64-apple-darwin ;;
    Linux:x86_64|Linux:amd64) TARGET=x86_64-unknown-linux-gnu ;;
    Linux:aarch64|Linux:arm64) TARGET=aarch64-unknown-linux-gnu ;;
    *) die "unsupported platform: $OS/$ARCH. Build from source with 'cargo install --git https://github.com/iamshreeram/quill --locked', or use install.ps1 on Windows." ;;
  esac
}

# Sets TAG (vX.Y.Z[-pre]) and VER (X.Y.Z[-pre]); one exact tag for the whole run.
resolve_tag() {
  if [ -n "${QUILL_VERSION:-}" ]; then
    req=${QUILL_VERSION#v}
    TAG=v$req
  else
    latest_url=$(curl_secure -fsSL -o /dev/null -w '%{url_effective}' "$BASE/latest" </dev/null) \
      || die "could not reach $BASE/latest to find the latest release"
    TAG=${latest_url##*/}
  fi
  is_release_tag "$TAG" || die "could not resolve a release tag (got '$TAG'); set QUILL_VERSION=X.Y.Z to choose one explicitly"
  VER=${TAG#v}
}

preflight_linux() {
  glibc=${QUILL_GLIBC_VERSION:-}
  if [ -z "$glibc" ]; then
    glibc=$(getconf GNU_LIBC_VERSION 2>/dev/null | awk '{ print $2 }')
  fi
  if ! version_ge "$glibc" 2.35; then
    die "glibc >= 2.35 is required for the prebuilt binary (found: ${glibc:-unknown}). Build from source instead: re-run with QUILL_FROM_SOURCE=1, or cargo install --git https://github.com/iamshreeram/quill --tag $TAG --locked"
  fi
  if [ "${QUILL_SKIP_LIBSSL_CHECK:-0}" != 1 ]; then
    ldc=$(command -v ldconfig 2>/dev/null || echo /sbin/ldconfig)
    if ! "$ldc" -p 2>/dev/null </dev/null | grep -q 'libssl\.so\.3'; then
      die "libssl.so.3 (OpenSSL 3 runtime) not found, needed by the prebuilt binary. Install it (e.g. apt-get install libssl3), or build from source: re-run with QUILL_FROM_SOURCE=1, or cargo install --git https://github.com/iamshreeram/quill --tag $TAG --locked"
    fi
  fi
}

require_sha_tool() {
  if command -v shasum >/dev/null 2>&1 || command -v sha256sum >/dev/null 2>&1; then
    return 0
  fi
  die "neither shasum nor sha256sum is available; cannot verify the sha256 checksum of the download"
}

sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{ print $1 }'
  else
    sha256sum "$1" | awk '{ print $1 }'
  fi
}

# fetch <url> <dest>: sets FETCH_CODE (HTTP code, 000 when no HTTP happened); returns 0 on success.
fetch() {
  FETCH_RC=0
  FETCH_CODE=$(curl_secure -fsSL -o "$2" -w '%{http_code}' "$1" </dev/null 2>/dev/null) || FETCH_RC=$?
  [ "$FETCH_RC" -eq 0 ]
}

# Returns 0 with ARCHIVE verified; returns 1 only when the archive is HTTP 404 (source fallback).
download_and_verify() {
  name="quill-$VER-$TARGET.tar.gz"
  ARCHIVE="$WORK/$name"
  if ! fetch "$BASE/download/$TAG/$name" "$ARCHIVE"; then
    if [ "$FETCH_CODE" = 404 ]; then
      return 1
    fi
    die "download of $name failed (HTTP $FETCH_CODE). Not falling back to a source build."
  fi
  if ! fetch "$BASE/download/$TAG/SHA256SUMS" "$WORK/SHA256SUMS"; then
    die "could not download SHA256SUMS for $TAG (HTTP $FETCH_CODE); refusing to install an unverified binary"
  fi
  expected=$(awk -v f="$name" '$2 == f { print $1; exit }' "$WORK/SHA256SUMS")
  [ -n "$expected" ] || die "no checksum line for $name in SHA256SUMS; nothing installed"
  actual=$(sha256_of "$ARCHIVE")
  if [ "$actual" != "$expected" ]; then
    die "checksum mismatch for $name (expected $expected, got $actual); nothing installed, existing binary untouched"
  fi
  return 0
}

# assert_version <binary>: the binary must report exactly "quill <VER>"
assert_version() {
  got=$("$1" --version 2>/dev/null </dev/null) || got=""
  if [ "$got" != "quill $VER" ]; then
    die "binary reports '${got:-nothing}', expected 'quill $VER'; existing install left untouched"
  fi
}

extract_and_check() {
  STAGE="$WORK/stage"
  mkdir -p "$STAGE"
  tar -xzf "$ARCHIVE" -C "$STAGE" </dev/null || die "could not extract $ARCHIVE"
  STAGED="$STAGE/quill-$VER-$TARGET/quill"
  [ -f "$STAGED" ] || die "archive does not contain quill-$VER-$TARGET/quill"
  assert_version "$STAGED"
}

# install_binary <staged-binary>: atomic replace of $INSTALL_DIR/quill (0755)
install_binary() {
  mkdir -p "$INSTALL_DIR" || die "cannot create $INSTALL_DIR"
  NEW_TMP=$(mktemp "${INSTALL_DIR}/.quill.XXXXXX") || die "could not create a temp file in $INSTALL_DIR"
  cp "$1" "$NEW_TMP" || die "could not copy the binary into $INSTALL_DIR"
  chmod 0755 "$NEW_TMP"
  mv -f "$NEW_TMP" "$INSTALL_DIR/quill"
  NEW_TMP=""
  BIN="$INSTALL_DIR/quill"
}

rustup_help() { # <reason>
  printf 'error: cannot build quill %s from source: %s\n' "$TAG" "$1" >&2
  printf '%s\n' "  Install Rust with rustup:" >&2
  printf '%s\n' "    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
  printf '%s\n' "  Homebrew's cargo can shadow rustup. If you use rustup, run:" >&2
  printf '%s\n' "    PATH=\$HOME/.cargo/bin:\$PATH and re-run this installer." >&2
  exit 1
}

build_from_source() {
  missing=""
  for tool in git cargo rustc; do
    if ! command -v "$tool" >/dev/null 2>&1; then
      missing="$missing $tool"
    fi
  done
  if [ -n "$missing" ]; then
    rustup_help "missing tools:$missing"
  fi
  SRC="$WORK/src"
  git clone --depth 1 --branch "$TAG" "$REPO_URL" "$SRC" </dev/null \
    || die "git clone of $TAG from $REPO_URL failed"
  msrv=$(sed -n 's/^rust-version *= *"\([^"]*\)".*/\1/p' "$SRC/Cargo.toml" | head -n 1)
  have=$(rustc --version </dev/null | awk '{ print $2 }')
  if [ -n "$msrv" ] && ! version_ge "$have" "$msrv"; then
    rustup_help "rustc ${have:-unknown} is older than the required $msrv"
  fi
  (cd "$SRC" && cargo build --release --locked </dev/null) || die "cargo build failed"
  assert_version "$SRC/target/release/quill"
  install_binary "$SRC/target/release/quill"
}

path_hint() {
  case ":$PATH:" in
    *":$INSTALL_DIR:"*) return 0 ;;
  esac
  say ""
  say "$INSTALL_DIR is not on your PATH. Add it yourself (this installer does not edit shell config):"
  # shellcheck disable=SC2016  # the literal $PATH is what the user must paste
  case "${SHELL:-}" in
    */fish) printf '  fish_add_path %s\n' "$INSTALL_DIR" ;;
    *) printf '  export PATH="%s:$PATH"\n' "$INSTALL_DIR" ;;
  esac
}

# ask_yes <question>: QUILL_YES=1 -> yes; non-interactive -> no; else ask on /dev/tty when openable
ask_yes() {
  if [ "${QUILL_YES:-0}" = 1 ]; then
    return 0
  fi
  if [ "${QUILL_NONINTERACTIVE:-0}" = 1 ]; then
    return 1
  fi
  if ! ( : </dev/tty ) 2>/dev/null; then
    return 1
  fi
  printf '%s [y/N] ' "$1" >/dev/tty 2>/dev/null || return 1
  ans=""
  read -r ans </dev/tty || return 1
  case "$ans" in
    y|Y|yes|YES) return 0 ;;
    *) return 1 ;;
  esac
}

config_init() {
  cfg="$HOME/.config/quill/config.toml"
  if [ -e "$cfg" ]; then
    say "Config already exists at $cfg (left unchanged)."
    return 0
  fi
  "$BIN" init </dev/null || warn "'quill init' failed; run it manually later"
}

macos_steps() {
  if ! command -v skhd >/dev/null 2>&1; then
    if command -v brew >/dev/null 2>&1 && ask_yes "Install the skhd hotkey daemon with Homebrew now?"; then
      brew install koekeishiya/formulae/skhd </dev/null || warn "brew install of skhd failed"
    else
      say "The skhd hotkey daemon is missing. Install it with:"
      say "  brew install koekeishiya/formulae/skhd"
      if ! command -v brew >/dev/null 2>&1; then
        say "  (Homebrew not found: get it from https://brew.sh first)"
      fi
    fi
  fi
  if [ "${QUILL_NO_SETUP:-0}" = 1 ]; then
    say "Skipping 'quill setup' (QUILL_NO_SETUP=1)."
  else
    "$BIN" setup </dev/null || warn "'quill setup' failed; run 'quill setup' after fixing the problem"
  fi
  if command -v skhd >/dev/null 2>&1; then
    skhd --start-service </dev/null >/dev/null 2>&1 || true
    say "Grant skhd Accessibility permission: System Settings > Privacy & Security > Accessibility (enable skhd)."
  fi
}

linux_steps() {
  have_clip=""
  for clip in wl-paste xclip xsel; do
    if command -v "$clip" >/dev/null 2>&1; then
      have_clip=$clip
    fi
  done
  if [ -z "$have_clip" ]; then
    say "No clipboard tool found. Install one of them:"
    if command -v apt-get >/dev/null 2>&1; then
      say "  sudo apt-get install wl-clipboard xclip"
    elif command -v dnf >/dev/null 2>&1; then
      say "  sudo dnf install wl-clipboard xclip"
    elif command -v pacman >/dev/null 2>&1; then
      say "  sudo pacman -S wl-clipboard xclip"
    else
      say "  install wl-clipboard (Wayland) or xclip (X11) with your package manager"
    fi
  fi
  say "Bind a desktop hotkey to: quill --now   (this installer does not set up a hotkey on Linux)"
}

finish() {
  "$BIN" status </dev/null || true
  say ""
  say "Done: quill $VER is installed at $BIN"
}

post_install() {
  path_hint
  config_init
  case "$OS" in
    Darwin) macos_steps ;;
    Linux) linux_steps ;;
  esac
  finish
}

main() {
  setup_env
  detect_target
  resolve_tag
  # Linux ABI preflight protects prebuilt binaries only; a source build skips it.
  if [ "$OS" = Linux ] && [ "$FROM_SOURCE" != 1 ]; then
    preflight_linux
  fi
  prepare_workdir
  if [ "$FROM_SOURCE" = 1 ]; then
    build_from_source
  else
    require_sha_tool
    if download_and_verify; then
      extract_and_check
      install_binary "$STAGED"
    else
      say "No release asset for $TAG on $TARGET (HTTP 404); falling back to a source build."
      build_from_source
    fi
  fi
  post_install
}

main "$@"
