#!/usr/bin/env bash
# Print the body (without the heading) of a CHANGELOG.md version section.
# Usage: scripts/changelog-section.sh <version>      e.g. 0.4.0 or Unreleased
# Exit: 0 ok, 1 heading missing, 2 usage error.
set -euo pipefail

ROOT="${QUILL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

version="${1:-}"
if [ -z "$version" ]; then
  echo "usage: changelog-section.sh <version>" >&2
  exit 2
fi
if [ ! -f "$ROOT/CHANGELOG.md" ]; then
  echo "error: CHANGELOG.md is missing" >&2
  exit 1
fi

awk -v h="## [$version]" '
  index($0, h) == 1 { found = 1; next }
  found && /^## \[/ { exit }
  found {
    if (!started && $0 ~ /^[[:space:]]*$/) next
    started = 1
    if ($0 ~ /^[[:space:]]*$/) { pending = pending "\n"; next }
    printf "%s", pending
    pending = ""
    print
  }
  END { exit !found }
' "$ROOT/CHANGELOG.md" || {
  echo "error: CHANGELOG.md has no '## [$version]' heading" >&2
  exit 1
}
