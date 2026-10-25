#!/usr/bin/env bash
# Package one built binary into a release archive and update SHA256SUMS.
# Usage: scripts/package.sh <target> <version> <binary-path> <outdir>
# Produces <outdir>/quill-<version>-<target>.tar.gz (or .zip for *windows* targets)
# whose top directory is quill-<version>-<target>/ and contains the binary, LICENSE,
# README.md and config.sample.toml; then updates <outdir>/SHA256SUMS.
set -euo pipefail

ROOT="${QUILL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

if [ $# -ne 4 ] || [ -z "$1" ] || [ -z "$2" ] || [ -z "$3" ] || [ -z "$4" ]; then
  echo "usage: package.sh <target> <version> <binary-path> <outdir>" >&2
  exit 2
fi
target="$1"
version="$2"
binary="$3"
outdir="$4"

if [ ! -f "$binary" ]; then
  echo "error: binary not found: $binary" >&2
  exit 1
fi

sha256_of() {
  if command -v shasum >/dev/null 2>&1; then
    shasum -a 256 "$1" | awk '{ print $1 }'
  else
    sha256sum "$1" | awk '{ print $1 }'
  fi
}

mkdir -p "$outdir"
outdir="$(cd "$outdir" && pwd)"

name="quill-$version-$target"
stage="$(mktemp -d "${TMPDIR:-/tmp}/quill-package.XXXXXX")"
sums_tmp=""
cleanup() {
  rm -rf "$stage"
  [ -z "$sums_tmp" ] || rm -f "$sums_tmp"
}
trap cleanup EXIT

mkdir "$stage/$name"
if [[ "$target" == *windows* ]]; then
  bin_name="quill.exe"
  archive="$name.zip"
else
  bin_name="quill"
  archive="$name.tar.gz"
fi
cp "$binary" "$stage/$name/$bin_name"
chmod 755 "$stage/$name/$bin_name"
cp "$ROOT/LICENSE" "$ROOT/README.md" "$ROOT/config.sample.toml" "$stage/$name/"

if [ "$archive" = "$name.zip" ]; then
  if command -v zip >/dev/null 2>&1; then
    rm -f "$outdir/$archive"
    (cd "$stage" && zip -qr "$outdir/$archive" "$name")
  else
    python3 - "$stage" "$name" "$outdir/$archive" <<'PY'
import os, sys, zipfile
stage, name, out = sys.argv[1], sys.argv[2], sys.argv[3]
with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as zf:
    base = os.path.join(stage, name)
    for dirpath, dirnames, filenames in os.walk(base):
        dirnames.sort()
        for filename in sorted(filenames):
            path = os.path.join(dirpath, filename)
            zf.write(path, os.path.relpath(path, stage))
PY
  fi
else
  tar -C "$stage" -czf "$outdir/$archive" "$name"
fi

# Update SHA256SUMS: drop any existing line for this archive, append the new one,
# keep the file sorted by filename and free of duplicates.
hash="$(sha256_of "$outdir/$archive")"
sums="$outdir/SHA256SUMS"
touch "$sums"
sums_tmp="$(mktemp "${TMPDIR:-/tmp}/quill-sums.XXXXXX")"
{
  awk -v f="$archive" '$2 != f' "$sums"
  printf '%s  %s\n' "$hash" "$archive"
} | LC_ALL=C sort -k2,2 > "$sums_tmp"
cat "$sums_tmp" > "$sums"

echo "packaged $outdir/$archive"
echo "sha256   $hash  $archive"
