#!/usr/bin/env bash
# Print the next release version (MAJOR.(MINOR+1).0, no leading "v").
# base = the higher of the highest stable tag (vX.Y.Z) and the plain X.Y.Z in Cargo.toml [package].
# Usage: scripts/next-version.sh          (QUILL_REPO_ROOT overrides the repo root)
# Exit: 0 ok, 2 not a git repo root or no base version.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="${QUILL_REPO_ROOT:-$here/..}"
if [ -d "$root" ]; then
  root="$(cd "$root" && pwd -P)"
fi

die() {
  echo "next-version: $*" >&2
  exit 2
}

is_plain_stable() {
  local re='^[0-9]+\.[0-9]+\.[0-9]+$'
  [[ "$1" =~ $re ]]
}

# Highest stable tag as a bare X.Y.Z (empty when there is none).
highest_stable_tag() {
  git -C "$root" tag -l 'v*' \
    | grep -E '^v[0-9]+\.[0-9]+\.[0-9]+$' \
    | sed 's/^v//' \
    | sort -t. -k1,1n -k2,2n -k3,3n \
    | tail -n 1 || true
}

# Plain X.Y.Z version of the [package] table in Cargo.toml (empty when absent).
cargo_version() {
  awk '
    /^\[/ { in_package = ($0 ~ /^\[package\][ \t]*$/); next }
    in_package && /^version[ \t]*=/ {
      s = $0; sub(/^[^"]*"/, "", s); sub(/".*$/, "", s); print s; exit
    }
  ' "$root/Cargo.toml" 2>/dev/null || true
}

if ! git -C "$root" rev-parse --show-toplevel >/dev/null 2>&1; then
  die "'$root' is not a git repository"
fi
# --show-cdup is empty exactly at the repository root (path strings differ on Git for Windows: /d/a vs D:/a)
if [ -n "$(git -C "$root" rev-parse --show-cdup)" ]; then
  die "'$root' is not the root of a git repository"
fi

stable_tag="$(highest_stable_tag)"
cargo_ver="$(cargo_version)"

candidates=""
if [ -n "$stable_tag" ]; then
  candidates="$stable_tag"
fi
if is_plain_stable "$cargo_ver"; then
  candidates="$candidates
$cargo_ver"
fi

base="$(printf '%s\n' "$candidates" | sed '/^$/d' | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)"
if [ -z "$base" ]; then
  die "no base version: no stable v<X.Y.Z> tag and no plain version in Cargo.toml"
fi

major="${base%%.*}"
rest="${base#*.}"
minor="${rest%%.*}"
printf '%s.%s.0\n' "$major" "$((10#$minor + 1))"
