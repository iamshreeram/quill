# Changelog

All notable changes to this project are documented in this file.
The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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
