Revision 3: critic round 2 (C1 still open: do not rely on Invoke-WebRequest exception shape) -> install.ps1 latest-tag resolution via System.Net.Http.HttpClient with AllowAutoRedirect=false; default-path installs verified under BOTH pwsh and powershell.exe
Revision 2: critic round 1 (C1 major: default install path via /latest redirect untested and likely broken on pwsh 7; C2 minor: negative tests for the process wrapper) -> shared helper scripts/lib-native.ps1, fixture server scripts/windows-fixture-server.py, install.ps1 latest-tag fix, negative self-tests
Revision 1: initial design
# Windows release smoke failure (quill)

## Goal
Make the experimental Windows target of the Release workflow produce a verified quill-<ver>-x86_64-pc-windows-msvc.zip,
make install.ps1 able to install it, and stop CI from hiding Windows smoke failures. No Rust/product code change.

## Diagnosis (evidence, not guesses)
- Release runs 40253434700, 40829223310, 40902249362 (three different releases, v0.6/v0.7/v0.8): the Windows build job fails in
  step "Smoke test binary (Windows)" with the identical signature `thread main panicked ... failed printing to stdout:
  The pipe is being closed. (os error 232)` followed by `unexpected version: ` (empty). No release ever had a Windows asset.
- Cause: src/main.rs:17 `#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]` => GUI-subsystem exe. PowerShell does
  not read a GUI program's stdout when it is captured (`$v = & $bin --version`); the writer's pipe is closed and Rust println! panics.
- Same pattern exists in install.ps1:44 `$got = (& $exe.FullName --version)` (header says UNTESTED ON WINDOWS) => installer would
  refuse every archive with "binary reports '', expected ...".
- CI (.github/workflows/ci.yml) "Windows smoke test (experimental)" has continue-on-error: true, so it can never fail the job.
- release.yml: build job has continue-on-error: ${matrix.experimental}; publish ships the 4 supported archives and adds the Windows
  zip only when artifact + SHA256SUMS exist (release.yml:254-259). After a successful Windows build the Package step
  (bash scripts/package.sh, zip via `zip` or python3 fallback) and publish's Windows handling have NEVER executed on a runner.

## Changes (files)  [Revision 2]
1. NEW scripts/lib-native.ps1 (dot-sourced): `Invoke-Native -FilePath <exe> -ArgumentList <string[]> [-StdinText <string>] [-TimeoutSec 60]` runs a native program through
   Start-Process with -RedirectStandardOutput/-RedirectStandardError (and -RedirectStandardInput from a BOM-less temp file when StdinText is given), -NoNewWindow -PassThru,
   `$null = $proc.Handle` (cache the handle so ExitCode is populated), WaitForExit(timeout ms); on timeout: Kill, WaitForExit, then throw "timed out after N s" (the child must be gone);
   after exit `$proc.WaitForExit()`; if ExitCode is $null => throw explicitly; returns [pscustomobject]{Code; Out; Err}; temp files always removed in `finally`. Never `$x = & exe`.
2. NEW scripts/smoke-windows.ps1 -Bin <exe> -Version <x.y.z>: dot-sources lib-native.ps1; `--version` must exit 0 and Out.Trim() -eq "quill <Version>"; `--stdin --mode offline`
   with stdin "this is an test" must exit 0 and Out match `a test`. Any failure throws (non-zero exit) with exit code, stdout and stderr in the message.
3. release.yml Windows smoke step: `shell: pwsh`, `./scripts/smoke-windows.ps1 -Bin "target\${{ matrix.target }}\release\quill.exe" -Version $env:VERSION`. Windows stays experimental
   (continue-on-error: matrix.experimental unchanged).
4. install.ps1 (standalone, piped via `irm | iex`, cannot dot-source): (a) replace `$got = (& $exe.FullName --version)` with an inline minimal Start-Process-to-file helper (bounded wait, ExitCode check);
   (b) fix latest-tag resolution WITHOUT relying on Invoke-WebRequest exception shapes: a helper `Get-LatestTag $Base` uses System.Net.Http.HttpClient + HttpClientHandler{AllowAutoRedirect=$false}
       (Add-Type -AssemblyName System.Net.Http for 5.1; TLS1.2 enabled on 5.1), GET "$Base/latest", reads `$resp.Headers.Location` (Uri, may be relative) directly, disposes response/client/handler in `finally`,
       requires a 3xx status with a Location, then tag = last path segment; the existing tag-format validation and error texts are kept. Works on PowerShell 7 and Windows PowerShell 5.1.
   (c) header text updated (exercised in CI on windows-latest).
5. NEW scripts/windows-fixture-server.py (stdlib only, cross-platform so it can be unit-tested locally on macOS): serves --root over http on 127.0.0.1:--port, answers `GET /latest` with
   `302 Location: /tag/v<--version>`, everything else is a static file (404 for missing); logs to stderr; exits on SIGTERM/kill.
6. ci.yml windows legs of the `test` job (all blocking; ci.yml contains no continue-on-error anywhere):
   a. pwsh: version from Cargo.toml; `./scripts/smoke-windows.ps1 -Bin ./target/debug/quill.exe -Version <ver>`.
   b. bash (same shell as the release Package step; avoids WSL bash.exe): `bash scripts/package.sh x86_64-pc-windows-msvc <ver> target/debug/quill.exe dist`.
   c. pwsh NEW scripts/check-windows-install.ps1 -Dist dist -Version <ver> with three parts:
      (i) wrapper SELF-TESTS (C2): nonzero exit + stderr captured (`cmd.exe /c "echo boom 1>&2 & exit 3"` => Code 3, Err has boom, no throw); timeout (`ping -n 30 127.0.0.1`, -TimeoutSec 2 => throws
          'timed out' AND the ping process is no longer running); wrong version (`smoke-windows.ps1 -Bin target/debug/quill.exe -Version 9.9.9` in a child pwsh must exit non-zero and mention expected/9.9.9);
          missing exe => throws.
      (ii) archive checks: zip exists, SHA256SUMS has a `<hash>  <zip>` line whose hash equals Get-FileHash, Expand-Archive, extracted quill.exe passes smoke-windows.ps1.
      (iii) end-to-end installs against the fixture server (port from TcpListener(0), readiness-polled, killed in finally, server layout <root>/download/v<ver>/{zip,SHA256SUMS}),
          a 2x2 matrix: shell in {pwsh, powershell.exe (5.1)} x {explicit QUILL_VERSION=<ver>, QUILL_VERSION unset (Remove-Item Env:QUILL_VERSION in the child) => default /latest redirect path}. Each install runs in a CHILD shell
          (`<shell> -NoProfile -ExecutionPolicy Bypass -Command "Get-Content -Raw '<install.ps1>' | Invoke-Expression"`, the advertised script-text mode) with QUILL_BASE_URL=http://127.0.0.1:<port>,
          QUILL_INSTALL_DIR=<fresh temp dir>; child exit code must be 0; afterwards the installed quill.exe must pass scripts/smoke-windows.ps1 (run by the parent pwsh).
          install.ps1 must therefore stay Windows-PowerShell-5.1 compatible (no `?:`, `??`, `&&`).
7. tests/release_scripts_test.sh NEW r09 static guard (existing `scripts` CI job, ubuntu+macos, bash 3.2 safe): lib-native.ps1 and smoke-windows.ps1 exist; they use Start-Process and
   RedirectStandardOutput; no `= & ` / `= (& ` capture of a native exe in any *.ps1 or workflow; release.yml and ci.yml reference scripts/smoke-windows.ps1; ci.yml has no continue-on-error;
   install.ps1 reads `Headers.Location`. NEW r10: scripts/windows-fixture-server.py unit test run locally (python3): /latest => 302 with the right Location, file => 200 with content,
   missing => 404.

## Acceptance checks
- A1 PR CI: `test (windows-latest)` runs steps 4a-4c and is green (blocking). This is the only place PowerShell can run (no pwsh locally).
- A1b (C1, rev 3) the default install path under BOTH pwsh and powershell.exe;  the default install path (QUILL_VERSION unset) and the explicit-version path both succeed on windows-latest against the fixture (C1); wrapper self-tests (C2) pass.
- A2 `bash tests/release_scripts_test.sh` includes r09 and r10 and passes; `bash tests/install_test.sh`, `bash tests/auto_release_test.sh`, `sh -n install.sh` still pass.
- A3 YAML parses (yq); ci/release workflows otherwise unchanged (git diff).
- A4 After merge the next real release must show `build x86_64-pc-windows-msvc: success` and a windows zip + SHA256SUMS entry in the release assets
  (a release only happens on a releasable commit to main; this PR itself will be a `fix:` => release v0.9.0 (next-version.sh bumps the minor) is triggered by the merge).

## Corner cases
- PowerShell `$proc.ExitCode` null without cached handle; WaitForExit(ms) vs -Wait; empty stdout file; BOM-less stdin; Start-Process with -ArgumentList array.
- `bash` inside pwsh may be WSL's bash.exe => packaging is its own `shell: bash` step.
- python on windows-latest is `python` (not necessarily python3); http.server readiness race; port collision => pick port by TcpListener(0) in the script.
- install.ps1 `throw` must fail the CI step; temp dirs cleaned.
- GUI-subsystem exe launched with -NoNewWindow must not pop a console or hang when stdin is a file.

## Constraints
No push to main; feature branch + PR; commit author iamshreeram <shreeram.v@live.com>; no Rust changes; POSIX/bash scripts unaffected; never weaken tests.

## Blast radius / rollback
Workflows + one PowerShell installer; revert the PR. Windows remains experimental in release.yml, so a Windows regression cannot block publishing the 4 supported archives.
The CI change makes the Windows test leg blocking: intended (it is the check that was silently disabled).

## CRITIC FINDINGS
[C1] severity: major status: STILL OPEN (round 2) -> addressed in Revision 3 (install.ps1 4b HttpClient AllowAutoRedirect=false, e2e 6c-iii 2x2 matrix); evidence pending: Windows CI.
  Round-2 critic: problem: `Invoke-WebRequest -MaximumRedirection 0` can report `MaximumRedirectExceeded` rather than an HttpResponseException carrying Response. fix: resolve /latest with a non-following HttpClient, read Headers.Location directly, dispose everything, support PS7+5.1. check: unset-QUILL_VERSION fixture install under both pwsh and powershell.exe.
  NOTE: the first round-3 call reviewed a stale Revision 2 (lead dispatched it before the file edit succeeded) => that round is void and was re-run.
  Round-1 block (verbatim):
where: install.ps1 latest-tag resolution; DESIGN.md Changes 4c
problem: The end-to-end check always sets QUILL_VERSION, bypassing the documented default installation path. That path reads `$_.Exception.Response.Headers['Location']`; in PowerShell 7 the exception response uses HttpResponseHeaders, whose Location is a property rather than the Windows PowerShell-style string-indexed header collection, so latest-tag resolution can fail before downloading anything.
why it matters: CI can pass and v0.8.1 can publish a valid Windows archive while the advertised `irm .../latest/download/install.ps1 | iex` invocation still refuses to install it.
fix: Extend the local HTTP fixture with a `/latest` redirect to `/tag/v<ver>` and run an additional installation with QUILL_VERSION unset, using the advertised script-text execution mode. Fix latest-tag resolution to extract the redirect reliably on PowerShell 7, preserving Windows PowerShell compatibility if it remains supported.
check: On windows-latest, both the explicit-version installation and the unset-QUILL_VERSION installation succeed against the fixture, and each installed executable passes smoke-windows.ps1.
[C2] severity: minor status: RESOLVED (critic round 2) ; Revision 2 (lib-native.ps1 contract + 6c-i self-tests)
where: DESIGN.md Acceptance checks and Changes 1/3
problem: All executable acceptance checks exercise successful children. They do not demonstrate that the new wrappers reject a nonzero or null exit code, enforce the timeout, or preserve useful diagnostics.
why it matters: These are the central reliability properties of the replacement process launcher; a green happy-path run alone does not validate them.
fix: Add Windows-only wrapper tests using controlled child processes for nonzero exit with stderr and timeout, plus a wrong-version invocation against quill.exe. Require null ExitCode to fail explicitly, and verify timeout cleanup before removing temporary files.
check: Windows CI asserts that each negative case fails as expected, reports its distinguishing diagnostic, and leaves no timed-out child running.
