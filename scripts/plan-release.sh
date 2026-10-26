#!/usr/bin/env bash
# Decide what the release workflow does for the current main (HEAD).
# Prints exactly seven key=value lines on stdout:
#   mode= reason= sha= version= tag= prev_tag= date=
# mode is one of: release | recover | reconcile | skip. Keys that do not apply are empty.
# Diagnostics go to stderr. Exit 0 for every decision (skip included), 1 on errors.
# Env:
#   QUILL_REPO_ROOT    repo root override (default: the parent of this script's directory)
#   GITHUB_REPOSITORY  owner/repo for gh (required only when an automation-owned tag needs classifying)
#   PLAN_RECOVER_TAG   owner override from workflow_dispatch (vX.Y.Z, may be empty)
#   QUILL_PLAN_DATE    UTC date to print (default: today, UTC)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
root="${QUILL_REPO_ROOT:-$here/..}"
if [ -d "$root" ]; then
  root="$(cd "$root" && pwd -P)"
fi

BOT_EMAIL='41898282+github-actions[bot]@users.noreply.github.com'
TAG_RE='^v[0-9]+\.[0-9]+\.[0-9]+$'
plan_date="${QUILL_PLAN_DATE:-$(date -u +%F)}"

die() {
  echo "plan-release: $*" >&2
  exit 1
}

# emit <mode> <reason> <sha> <version> <tag> <prev_tag>
emit() {
  printf 'mode=%s\nreason=%s\nsha=%s\nversion=%s\ntag=%s\nprev_tag=%s\ndate=%s\n' \
    "$1" "$2" "$3" "$4" "$5" "$6" "$plan_date"
}

skip() {
  emit skip "$1" "" "" "" "" ""
  exit 0
}

# cargo_package_version <rev>: first `version = "x"` in the [package] table of Cargo.toml at <rev>.
cargo_package_version() {
  git -C "$root" show "$1:Cargo.toml" | awk '
    /^\[/ { in_package = ($0 ~ /^\[package\][ \t]*$/); next }
    in_package && /^version[ \t]*=/ && !done {
      s = $0; sub(/^[^"]*"/, "", s); sub(/".*$/, "", s); print s; done = 1
    }
  '
}

# is_automation_owned <tag>: tag's commit is the bot's release commit.
is_automation_owned() {
  local subject email
  subject="$(git -C "$root" log -1 --format=%s "refs/tags/$1^{commit}")"
  email="$(git -C "$root" log -1 --format=%ae "refs/tags/$1^{commit}")"
  [ "$subject" = "chore(release): $1 [skip ci]" ] && [ "$email" = "$BOT_EMAIL" ]
}

# ---- 2. owner override (dispatch input recover_tag) ----
recover_tag="${PLAN_RECOVER_TAG:-}"
if [ -n "$recover_tag" ]; then
  [[ "$recover_tag" =~ $TAG_RE ]] || die "PLAN_RECOVER_TAG must look like v<X.Y.Z>"
  git -C "$root" rev-parse -q --verify "refs/tags/$recover_tag" >/dev/null \
    || die "tag $recover_tag does not exist locally"
  override_sha="$(git -C "$root" rev-parse "refs/tags/$recover_tag^{commit}")"
  override_ver="$(cargo_package_version "refs/tags/$recover_tag^{commit}")"
  [ "$override_ver" = "${recover_tag#v}" ] \
    || die "Cargo.toml version at $recover_tag is '$override_ver', expected ${recover_tag#v}"
  emit recover override "$override_sha" "${recover_tag#v}" "$recover_tag" ""
  exit 0
fi

# ---- 1. highest local stable tag (T) ----
highest="$(git -C "$root" tag -l 'v*' | { grep -E "$TAG_RE" || true; } \
  | sed 's/^v//' | sort -t. -k1,1n -k2,2n -k3,3n | tail -n 1)"
T=""
if [ -n "$highest" ]; then
  T="v$highest"
fi

# ---- 3. automation-owned recovery / reconciliation of T ----
if [ -n "$T" ] && is_automation_owned "$T"; then
  marker="$(git -C "$root" ls-remote origin "refs/published/$T")" \
    || die "git ls-remote origin refs/published/$T failed"
  if [ -z "$marker" ]; then
    [ -n "${GITHUB_REPOSITORY:-}" ] || die "GITHUB_REPOSITORY is required to classify $T"
    t_sha="$(git -C "$root" rev-parse "refs/tags/$T^{commit}")"
    if published="$(gh release view "$T" -R "$GITHUB_REPOSITORY" --json isDraft --jq .isDraft)" \
      && [ "$published" = false ]; then
      emit reconcile missing-marker "$t_sha" "${T#v}" "$T" ""
    else
      emit recover unpublished "$t_sha" "${T#v}" "$T" ""
    fi
    exit 0
  fi
fi

# ---- 4. skip decisions ----
head_subject="$(git -C "$root" log -1 --format=%s HEAD)"
head_message="$(git -C "$root" log -1 --format=%B HEAD)"
case "$head_subject" in
  'chore(release):'*) skip release-commit ;;
esac
case "$head_message" in
  *'[skip release]'*) skip skip-marker ;;
esac

if [ -n "$T" ]; then
  range="refs/tags/$T..HEAD"
else
  range="HEAD"
fi
non_release="$(git -C "$root" log --format=%s "$range" \
  | { grep -v '^chore(release):' || true; } | wc -l | tr -d ' ')"
if [ "$non_release" -eq 0 ]; then
  skip no-changes
fi

if [ -n "$T" ]; then
  changed="$(git -C "$root" diff --name-only "refs/tags/$T" HEAD)"
else
  changed="$(git -C "$root" ls-tree -r --name-only HEAD)"
fi
docs_only=yes
while IFS= read -r path; do
  [ -n "$path" ] || continue
  case "$path" in
    *.md|LICENSE|.gitignore|*/LICENSE|*/.gitignore) ;;
    *) docs_only=no ;;
  esac
done <<< "$changed"
if [ "$docs_only" = yes ]; then
  skip docs-only
fi

# ---- 5. release ----
version="$(QUILL_REPO_ROOT="$root" "$here/next-version.sh")" || die "next-version.sh failed"
tag="v$version"
if git -C "$root" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  die "tag $tag already exists locally"
fi
remote_tag="$(git -C "$root" ls-remote --tags origin "refs/tags/$tag")" \
  || die "git ls-remote --tags origin failed"
if [ -n "$remote_tag" ]; then
  die "tag $tag already exists on origin"
fi
head_sha="$(git -C "$root" rev-parse HEAD)"
emit release changes "$head_sha" "$version" "$tag" "$T"
