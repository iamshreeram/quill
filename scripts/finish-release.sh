#!/usr/bin/env bash
# Idempotently finish a GitHub release for an existing tag.
# Usage: GITHUB_REPOSITORY=owner/repo scripts/finish-release.sh <tag> <assets_dir> <notes_file>
#   published release exists        -> exit 0, no other gh calls
#   draft exists                    -> upload assets (--clobber), update notes, publish
#   no release                      -> create draft (--verify-tag) with assets + notes, publish
# Exit: 0 ok, 1 failure (gh error, no assets), 2 usage / invalid input.
set -euo pipefail
LC_ALL=C
shopt -s nullglob dotglob

if [ "$#" -ne 3 ]; then
  echo "usage: GITHUB_REPOSITORY=owner/repo finish-release.sh <tag> <assets_dir> <notes_file>" >&2
  exit 2
fi
tag="$1"
assets_dir="$2"
notes_file="$3"

repo="${GITHUB_REPOSITORY:-}"
if [ -z "$repo" ]; then
  echo "error: GITHUB_REPOSITORY must be set (owner/repo)" >&2
  exit 2
fi
if [[ ! "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "error: tag '$tag' is not of the form vX.Y.Z" >&2
  exit 2
fi
# Notes file and assets dir are validated only before a mutating gh call: an already
# published release is a pure no-op (state a) and needs neither.
validate_inputs() {
  if [ ! -f "$notes_file" ] || [ ! -r "$notes_file" ]; then
    echo "error: notes file '$notes_file' is not a readable file" >&2
    exit 2
  fi
  if [ ! -d "$assets_dir" ]; then
    echo "error: assets directory '$assets_dir' does not exist" >&2
    exit 2
  fi
}

# Collect every regular file directly inside assets_dir, sorted (glob output is sorted under LC_ALL=C).
collect_assets() {
  validate_inputs
  assets=()
  local f
  for f in "$assets_dir"/*; do
    if [ -f "$f" ]; then
      assets+=("$f")
    fi
  done
  if [ "${#assets[@]}" -eq 0 ]; then
    echo "error: no release assets found in '$assets_dir'" >&2
    exit 1
  fi
}

publish() {
  if ! gh release edit "$tag" --draft=false --latest -R "$repo"; then
    echo "error: failed to publish release $tag (it remains a draft; rerun to retry)" >&2
    exit 1
  fi
  echo "published release $tag"
}

# Release state: stdout of isDraft; any non-zero exit means the release does not exist.
if draft_state="$(gh release view "$tag" -R "$repo" --json isDraft --jq .isDraft 2>/dev/null)"; then
  case "$draft_state" in
    false)
      echo "release $tag already published"
      exit 0
      ;;
    true)
      collect_assets
      if ! gh release upload "$tag" "${assets[@]}" --clobber -R "$repo"; then
        echo "error: failed to upload assets to draft release $tag" >&2
        exit 1
      fi
      if ! gh release edit "$tag" --notes-file "$notes_file" -R "$repo"; then
        echo "error: failed to update notes of draft release $tag" >&2
        exit 1
      fi
      publish
      ;;
    *)
      echo "error: unexpected isDraft value '$draft_state' for $tag" >&2
      exit 1
      ;;
  esac
else
  collect_assets
  if ! gh release create "$tag" "${assets[@]}" --draft --verify-tag --title "$tag" \
      --notes-file "$notes_file" -R "$repo"; then
    echo "error: failed to create draft release $tag (is the tag pushed to origin?)" >&2
    exit 1
  fi
  publish
fi
