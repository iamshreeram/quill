# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.9.0] - 2026-10-29

### Changed
- fix(windows): run quill.exe through Start-Process so the release smoke test, installer and CI can read its output
- test(windows): start the Windows PowerShell 5.1 install check from a clean module path
- test(windows): widen the capture guard ($x=&$exe, $(& ...), @(& ...)) and test the guard itself

## [0.8.0] - 2026-10-29

### Changed
- feat(setup): list required macOS permissions and report a missing one instead of failing silently
- fix(hotkey): one run at a time, clipboard retry, result banners and an appended log
- fix(hotkey): compile and run the hotkey tests only on unix; sweep stale temp files; cap log bytes
- fix(hotkey): never replace the log with an empty trimmed file

## [0.7.0] - 2026-10-29

### Changed
- test: spec for loosened allow-list, frequency-ranked spelling, irregular table (red)
- feat(offline): enable vetted Harper rules by name, frequency-ranked spelling, irregular-verb table
- fix(offline): restrict frequency deletion to doubled letters; require listed insertions; docs and data NOTICE
- docs: add .ram-status design/status records

## [0.6.0] - 2026-10-26

### Added
- Self-healing release workflow (recovery and completion marker), `[skip release]` commit marker, and dry-run mode.

### Changed
- Releases are now fully automatic: every push or merge to main builds and publishes OS-specific binaries and bumps the minor version (no manual release script).

## [0.5.0] - 2026-10-26

### Added
- Offline grammar mode powered by `harper-core`, for checking and correcting text without any network access.
- Automatic provider selection: the configured API is used first, then a local Ollama server, then offline mode.
- `quill status` command to show which provider and mode would be used.
- `--mode` and `--offline` flags, plus `[general]`, `[ollama]` and `[offline]` configuration sections.
- `curl | bash` installer (`install.sh`) with checksum verification, and `install.ps1` for Windows.
- Offline spelling fixes are deliberately conservative: only clear misspellings are corrected, and names, URLs, paths, flags and code-like tokens are left untouched.
- Release pipeline: CI with lint, multi-platform tests and shell-script checks, and a tag-driven release workflow that builds binaries for Linux (x86_64 and aarch64), macOS (Intel and Apple Silicon) and Windows, and publishes them with `SHA256SUMS`.

### Changed
- `quill init` now writes the new `[general]`, `[ollama]` and `[offline]` sections and replaces only the `[api]` `url` and `key` values from `QUILL_API_URL` and `QUILL_API_KEY`.
- `quill check` now accepts any well-formed Messages reply; a thinking-only reply from a local model proves the endpoint works.
- `quill check` uses a larger token budget for Ollama, so local reasoning models get room to answer.

## [0.4.0] - 2026-10-25

### Added
- Initial public release.
