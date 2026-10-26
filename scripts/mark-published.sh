#!/usr/bin/env bash
# Record the durable "release completed" marker refs/published/<tag> on origin,
# pointing at the tag's commit. Idempotent.
# Usage: scripts/mark-published.sh <tag>      (repo root: $QUILL_REPO_ROOT or current git toplevel)
# Exit: 0 ok, 1 missing tag / git or push failure, 2 usage error.
set -euo pipefail

if [ "$#" -ne 1 ]; then
  echo "usage: mark-published.sh <tag>" >&2
  exit 2
fi
tag="$1"
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: tag '$tag' is not of the form vX.Y.Z" >&2
  exit 2
fi

root="${QUILL_REPO_ROOT:-}"
if [ -z "$root" ]; then
  if ! root="$(git rev-parse --show-toplevel 2>/dev/null)"; then
    echo "error: not inside a git repository (set QUILL_REPO_ROOT)" >&2
    exit 1
  fi
fi

if ! commit="$(git -C "$root" rev-parse --verify --quiet "refs/tags/$tag^{commit}")"; then
  echo "error: tag $tag not found in $root" >&2
  exit 1
fi

ref="refs/published/$tag"
if ! remote_listing="$(git -C "$root" ls-remote origin "$ref")"; then
  echo "error: cannot query origin for $ref (is origin reachable?)" >&2
  exit 1
fi
remote_sha="$(printf '%s\n' "$remote_listing" | awk -v ref="$ref" '$2 == ref { print $1; exit }')"
if [ "$remote_sha" = "$commit" ]; then
  echo "marked $tag (already at $commit)"
  exit 0
fi

if ! git -C "$root" push origin "$commit:$ref"; then
  echo "error: failed to push $ref to origin (marker not written)" >&2
  exit 1
fi
echo "marked $tag"
