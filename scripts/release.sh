#!/usr/bin/env bash
# Cut a release: bump the version, move CHANGELOG [Unreleased] under the new version,
# commit and create an annotated tag. NEVER pushes.
# Usage: scripts/release.sh <patch|minor|major|X.Y.Z[-pre]> [--dry-run]
set -euo pipefail

ROOT="${QUILL_REPO_ROOT:-$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)}"

die() { echo "error: $*" >&2; exit 1; }

usage() {
  echo "usage: release.sh <patch|minor|major|X.Y.Z[-pre]> [--dry-run]" >&2
  exit 2
}

# ---- arguments (--dry-run may appear in any position) ----
dry_run=0
spec=""
for arg in "$@"; do
  case "$arg" in
    --dry-run) dry_run=1 ;;
    -*) usage ;;
    *)
      [ -z "$spec" ] || usage
      spec="$arg"
      ;;
  esac
done
[ -n "$spec" ] || usage

# ---- semver helpers ----
semver_re='^[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.]+)?$'

# cmp_num A B -> prints -1, 0 or 1
cmp_num() {
  if (( $1 > $2 )); then echo 1
  elif (( $1 < $2 )); then echo -1
  else echo 0
  fi
}

# compare_versions A B -> prints -1, 0 or 1 using SemVer precedence
compare_versions() {
  local a="$1" b="$2" ca cb c i x y pa pb
  local -a ia ib
  local a1 a2 a3 b1 b2 b3
  ca="${a%%-*}"
  cb="${b%%-*}"
  IFS=. read -r a1 a2 a3 <<< "$ca"
  IFS=. read -r b1 b2 b3 <<< "$cb"
  for pair in "$a1:$b1" "$a2:$b2" "$a3:$b3"; do
    c="$(cmp_num "${pair%%:*}" "${pair##*:}")"
    if [ "$c" != 0 ]; then echo "$c"; return; fi
  done
  pa=""; pb=""
  [[ "$a" == *-* ]] && pa="${a#*-}"
  [[ "$b" == *-* ]] && pb="${b#*-}"
  if [ -z "$pa" ] && [ -z "$pb" ]; then echo 0; return; fi
  if [ -z "$pa" ]; then echo 1; return; fi
  if [ -z "$pb" ]; then echo -1; return; fi
  IFS=. read -r -a ia <<< "$pa"
  IFS=. read -r -a ib <<< "$pb"
  i=0
  while [ "$i" -lt "${#ia[@]}" ] && [ "$i" -lt "${#ib[@]}" ]; do
    x="${ia[$i]}"; y="${ib[$i]}"
    if [[ "$x" =~ ^[0-9]+$ ]] && [[ "$y" =~ ^[0-9]+$ ]]; then
      c="$(cmp_num "$x" "$y")"
    elif [[ "$x" =~ ^[0-9]+$ ]]; then c=-1
    elif [[ "$y" =~ ^[0-9]+$ ]]; then c=1
    elif [[ "$x" < "$y" ]]; then c=-1
    elif [[ "$x" > "$y" ]]; then c=1
    else c=0
    fi
    if [ "$c" != 0 ]; then echo "$c"; return; fi
    i=$((i + 1))
  done
  if [ "${#ia[@]}" -eq "${#ib[@]}" ]; then echo 0
  elif [ "${#ia[@]}" -lt "${#ib[@]}" ]; then echo -1
  else echo 1
  fi
}

# Replace a file's contents in place (keeps its mode): cat tmp > file.
replace_contents() {
  cat "$2" > "$1"
  rm -f "$2"
}

cd "$ROOT"

# ---- current version from Cargo.toml [package] ----
current="$(awk '
  /^\[/ { in_package = ($0 == "[package]") }
  in_package && /^version[[:space:]]*=/ { sub(/^[^"]*"/, ""); sub(/".*$/, ""); print; exit }
' Cargo.toml)"
[ -n "$current" ] || die "could not read [package] version from Cargo.toml"

# ---- precondition 1: clean working tree ----
[ -z "$(git status --porcelain)" ] || die "working tree is not clean; commit or stash first"

# ---- precondition 2: on branch main ----
branch="$(git rev-parse --abbrev-ref HEAD)"
[ "$branch" = "main" ] || die "must be on branch main (currently on '$branch')"

# ---- precondition 3: valid, strictly greater version ----
case "$spec" in
  patch|minor|major)
    core="${current%%-*}"
    IFS=. read -r maj min pat <<< "$core"
    case "$spec" in
      patch) pat=$((pat + 1)) ;;
      minor) min=$((min + 1)); pat=0 ;;
      major) maj=$((maj + 1)); min=0; pat=0 ;;
    esac
    new="$maj.$min.$pat"
    ;;
  *)
    [[ "$spec" =~ $semver_re ]] || die "'$spec' is not patch, minor, major or a SemVer version"
    new="$spec"
    ;;
esac
[ "$(compare_versions "$new" "$current")" = 1 ] \
  || die "new version $new must be strictly greater than current version $current"
tag="v$new"

# ---- precondition 4: tag does not exist ----
if git rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  die "tag $tag already exists"
fi

# ---- precondition 5: CHANGELOG [Unreleased] has content ----
[ -f CHANGELOG.md ] || die "CHANGELOG.md is missing"
unreleased_body="$(awk '
  index($0, "## [Unreleased]") == 1 { found = 1; next }
  found && /^## \[/ { exit }
  found { print }
' CHANGELOG.md)"
if ! awk 'index($0, "## [Unreleased]") == 1 { f = 1 } END { exit !f }' CHANGELOG.md; then
  die "CHANGELOG.md has no '## [Unreleased]' heading"
fi
if [[ ! "$unreleased_body" =~ [^[:space:]] ]]; then
  die "CHANGELOG.md '## [Unreleased]' section is empty; add release notes first"
fi

today="$(date +%Y-%m-%d)"

if [ "$dry_run" -eq 1 ]; then
  echo "dry run: no files, commits or tags will be changed"
  echo "current version: $current"
  echo "new version:     $new  (tag $tag)"
  echo "would update:    Cargo.toml [package] version, Cargo.lock quill entry"
  echo "would update:    CHANGELOG.md: [Unreleased] -> [$new] - $today"
  echo "would commit:    chore(release): $tag"
  echo "would create:    annotated tag $tag"
  echo "would NOT push anything"
  exit 0
fi

# ---- effects ----
tmp="$(mktemp "${TMPDIR:-/tmp}/quill-release.XXXXXX")"

awk -v v="$new" '
  /^\[/ { in_package = ($0 == "[package]") }
  in_package && !done && /^version[[:space:]]*=/ { print "version = \"" v "\""; done = 1; next }
  { print }
' Cargo.toml > "$tmp"
replace_contents Cargo.toml "$tmp"

[ -f Cargo.lock ] || die "Cargo.lock is missing"
tmp="$(mktemp "${TMPDIR:-/tmp}/quill-release.XXXXXX")"
awk -v v="$new" '
  /^\[\[package\]\]/ { is_quill = 0; done = 0 }
  /^name = "quill"[[:space:]]*$/ { is_quill = 1 }
  is_quill && !done && /^version[[:space:]]*=/ { print "version = \"" v "\""; done = 1; next }
  { print }
' Cargo.lock > "$tmp"
replace_contents Cargo.lock "$tmp"

tmp="$(mktemp "${TMPDIR:-/tmp}/quill-release.XXXXXX")"
awk -v v="$new" -v d="$today" '
  !done && index($0, "## [Unreleased]") == 1 {
    print
    print ""
    print "## [" v "] - " d
    done = 1
    next
  }
  { print }
' CHANGELOG.md > "$tmp"
replace_contents CHANGELOG.md "$tmp"

git add Cargo.toml Cargo.lock CHANGELOG.md
git commit -q -m "chore(release): $tag"
git tag -a "$tag" -m "quill $tag"

echo "released $tag (commit and annotated tag created; nothing pushed)"
echo "next step: git push origin main --follow-tags"
