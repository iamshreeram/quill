#!/usr/bin/env bash
# Prepare a release: set <X.Y.Z> in Cargo.toml [package], the quill entry of Cargo.lock and CHANGELOG.md.
# Usage: scripts/bump-version.sh <X.Y.Z> [--dry-run] [--prev-tag TAG] [--date YYYY-MM-DD]
#   --prev-tag  default: highest stable tag (none = whole history)
#   --date      default: today (UTC)
#   Only the next minor (as printed by next-version.sh) is accepted.
# CHANGELOG: a non-empty [Unreleased] list of bullets is kept as the release notes; otherwise
#   the notes are generated from git subjects since PREV.
# Exit: 0 bumped (or would bump), 1 precondition failed (nothing changed),
#       2 usage error, 3 nothing to release (nothing changed).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="${QUILL_REPO_ROOT:-$here/..}"
if [ -d "$root" ]; then
  root="$(cd "$root" && pwd -P)"
fi

usage() {
  echo "usage: bump-version.sh <X.Y.Z> [--dry-run] [--prev-tag TAG] [--date YYYY-MM-DD]" >&2
  exit 2
}

fail() {
  echo "bump-version: $*" >&2
  exit 1
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

# ---- arguments ----
version=""
dry_run=0
prev_tag=""
prev_given=0
dt=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    --dry-run)
      dry_run=1
      shift
      ;;
    --prev-tag)
      [ "$#" -ge 2 ] || usage
      prev_tag="$2"
      prev_given=1
      shift 2
      ;;
    --date)
      [ "$#" -ge 2 ] || usage
      dt="$2"
      shift 2
      ;;
    --*)
      echo "bump-version: unknown flag: $1" >&2
      usage
      ;;
    *)
      [ -z "$version" ] || {
        echo "bump-version: unexpected argument: $1" >&2
        usage
      }
      version="$1"
      shift
      ;;
  esac
done

[ -n "$version" ] || usage
if [ "$prev_given" -eq 1 ] && [ -z "$prev_tag" ]; then
  usage
fi
if [ -z "$dt" ]; then
  dt="$(date -u +%Y-%m-%d)"
fi
date_re='^[0-9]{4}-[0-9]{2}-[0-9]{2}$'
[[ "$dt" =~ $date_re ]] || usage

# ---- preconditions (no writes yet) ----
is_plain_stable "$version" || fail "'$version' is not a plain stable X.Y.Z version"

if ! git -C "$root" rev-parse --show-toplevel >/dev/null 2>&1; then
  fail "'$root' is not a git repository"
fi
# --show-cdup is empty exactly at the repository root (path strings differ on Git for Windows: /d/a vs D:/a)
if [ -n "$(git -C "$root" rev-parse --show-cdup)" ]; then
  fail "'$root' is not the root of a git repository"
fi

expected="$(QUILL_REPO_ROOT="$root" bash "$here/next-version.sh")" \
  || fail "cannot determine the next version"
if [ "$version" != "$expected" ]; then
  fail "$version is not the next version ($expected); only the next minor is accepted"
fi

for name in Cargo.toml Cargo.lock CHANGELOG.md; do
  [ -f "$root/$name" ] || fail "$name is missing"
done

if awk -v v="$version" 'index($0, "## [" v "]") == 1 { found = 1 } END { exit !found }' "$root/CHANGELOG.md"; then
  fail "CHANGELOG.md already has a ## [$version] heading"
fi

if [ "$prev_given" -eq 1 ]; then
  git -C "$root" rev-parse -q --verify "refs/tags/$prev_tag" >/dev/null \
    || fail "tag '$prev_tag' not found"
else
  stable="$(highest_stable_tag)"
  prev_tag=""
  if [ -n "$stable" ]; then
    prev_tag="v$stable"
  fi
fi

has_unreleased=0
if grep -Fxq '## [Unreleased]' "$root/CHANGELOG.md"; then
  has_unreleased=1
fi

has_bullets=0
if awk '
  $0 == "## [Unreleased]" { inu = 1; next }
  inu && /^## \[/ { inu = 0 }
  inu && /^[ \t]*[-*] / { found = 1 }
  END { exit !found }
' "$root/CHANGELOG.md"; then
  has_bullets=1
fi

# ---- staging area (outside the repo; all new content is built here first) ----
work="$(mktemp -d "${TMPDIR:-/tmp}/quill-bump.XXXXXX")"
tmp_toml=""
tmp_lock=""
tmp_cl=""
cleanup() {
  rm -rf "$work"
  if [ -n "$tmp_toml" ]; then rm -f "$tmp_toml"; fi
  if [ -n "$tmp_lock" ]; then rm -f "$tmp_lock"; fi
  if [ -n "$tmp_cl" ]; then rm -f "$tmp_cl"; fi
}
trap cleanup EXIT

mode="generated"
if [ "$has_bullets" -eq 1 ]; then
  mode="bullets"
else
  notes="$work/notes.md"
  range="HEAD"
  if [ -n "$prev_tag" ]; then
    range="$prev_tag..HEAD"
  fi
  git -C "$root" log --reverse --no-merges --format=%s "$range" > "$work/subjects" \
    || fail "cannot read git history for $range"
  # Subjects are data: only awk/tr text filters, never eval or shell expansion.
  # Oldest first (--reverse). LF is kept in tr: it is the record separator.
  awk '!/^chore\(release\):/' "$work/subjects" \
    | tr -d '\000-\011\013-\037\177' \
    | tr '`' "'" \
    | awk '
        length($0) > 200 { $0 = substr($0, 1, 200) }
        /^[ \t]*$/ { next }
        seen[$0]++ { next }
        { n++; if (n <= 50) print }
      ' > "$notes"
  count="$(wc -l < "$notes" | tr -d ' ')"
  if [ "$count" -eq 0 ]; then
    echo "bump-version: nothing to release: no [Unreleased] bullets and no commits since ${prev_tag:-the start}" >&2
    exit 3
  fi
fi

# ---- build the new file contents into $work (validated; repo untouched) ----
awk -v ver="$version" '
  /^\[/ { in_package = ($0 ~ /^\[package\][ \t]*$/); print; next }
  in_package && !done && /^version[ \t]*=/ { sub(/"[^"]*"/, "\"" ver "\""); done = 1 }
  { print }
  END { exit (done ? 0 : 4) }
' "$root/Cargo.toml" > "$work/Cargo.toml" \
  || fail "Cargo.toml has no version line in [package]"

awk -v ver="$version" '
  /^\[\[package\]\]/ { inq = 0; print; next }
  /^name = "quill"[ \t]*$/ { nq++; inq = 1 }
  inq && !done && /^version[ \t]*=/ { sub(/"[^"]*"/, "\"" ver "\""); done = 1; inq = 0 }
  { print }
  END { exit ((nq == 1 && done) ? 0 : 4) }
' "$root/Cargo.lock" > "$work/Cargo.lock" \
  || fail "Cargo.lock must contain exactly one [[package]] named quill with a version"

awk -v ver="$version" -v dt="$dt" -v mode="$mode" -v notes="${notes:-/dev/null}" -v has_u="$has_unreleased" '
  function emit_block(   i, first, last, line) {
    print "## [Unreleased]"
    print ""
    print "## [" ver "] - " dt
    print ""
    if (mode == "bullets") {
      first = 0; last = 0
      for (i = 1; i <= nbody; i++) {
        if (body[i] !~ /^[ \t]*$/) { if (!first) first = i; last = i }
      }
      for (i = first; first && i <= last; i++) print body[i]
    } else {
      print "### Changed"
      while ((getline line < notes) > 0) print "- " line
      close(notes)
    }
    print ""
  }
  BEGIN { state = 0; nbody = 0 }
  state == 0 {
    if (has_u == 1 && $0 == "## [Unreleased]") { state = 1; next }
    if (has_u != 1 && index($0, "## [") == 1) { emit_block(); print; state = 2; next }
    print; next
  }
  state == 1 {
    if (index($0, "## [") == 1) { emit_block(); print; state = 2; next }
    body[++nbody] = $0; next
  }
  { print }
  END { if (state == 0 || state == 1) emit_block() }
' "$root/CHANGELOG.md" > "$work/CHANGELOG.md" \
  || fail "cannot rewrite CHANGELOG.md"

if [ "$dry_run" -eq 1 ]; then
  echo "would bump $version"
  exit 0
fi

# ---- stage next to each target (same filesystem), keep file modes, then move into place ----
tmp_toml="$(mktemp "$root/Cargo.toml.XXXXXX")"
cp -p "$root/Cargo.toml" "$tmp_toml"
cat "$work/Cargo.toml" > "$tmp_toml"

tmp_lock="$(mktemp "$root/Cargo.lock.XXXXXX")"
cp -p "$root/Cargo.lock" "$tmp_lock"
cat "$work/Cargo.lock" > "$tmp_lock"

tmp_cl="$(mktemp "$root/CHANGELOG.md.XXXXXX")"
cp -p "$root/CHANGELOG.md" "$tmp_cl"
cat "$work/CHANGELOG.md" > "$tmp_cl"

mv "$tmp_toml" "$root/Cargo.toml"
tmp_toml=""
mv "$tmp_lock" "$root/Cargo.lock"
tmp_lock=""
mv "$tmp_cl" "$root/CHANGELOG.md"
tmp_cl=""

echo "bumped $version"
