#!/usr/bin/env bash
# Verify that a release tag matches Cargo.toml and has a CHANGELOG.md section.
# Usage: scripts/check-version.sh <tag>        e.g. v0.4.0 or v1.0.0-rc.1
# Exit: 0 ok, 1 invalid tag / mismatch, 2 usage error.
set -euo pipefail

ROOT="${QUILL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

tag="${1:-}"
if [ -z "$tag" ]; then
  echo "usage: check-version.sh <tag>" >&2
  exit 2
fi
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$ ]]; then
  echo "error: tag '$tag' is not of the form vX.Y.Z or vX.Y.Z-pre" >&2
  exit 1
fi
version="${tag#v}"

cargo_version="$(awk '
  /^\[/ { in_package = ($0 == "[package]") }
  in_package && /^version[[:space:]]*=/ { sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit }
' "$ROOT/Cargo.toml")"

if [ "$cargo_version" != "$version" ]; then
  echo "error: tag version $version does not match Cargo.toml version ${cargo_version:-<none>}" >&2
  exit 1
fi

if [ ! -f "$ROOT/CHANGELOG.md" ]; then
  echo "error: CHANGELOG.md is missing" >&2
  exit 1
fi
if ! awk -v h="## [$version]" 'index($0, h) == 1 { found = 1 } END { exit !found }' "$ROOT/CHANGELOG.md"; then
  echo "error: CHANGELOG.md has no '## [$version]' heading" >&2
  exit 1
fi

echo "ok: $tag matches Cargo.toml and CHANGELOG.md"
