Revision 3: exact artifacts frozen (A5) after critic APPROVE of Rev 2; no behaviour change
Revision 2: critic round 1 (C1 notify AppleScript needs `end run`, osacompile test; C2 unrecognised -1719 must stop, not continue)
Revision 1: initial

# Goal
Make quill tell the user (a) UP FRONT which macOS permissions the hotkey needs and (b) AT RUN TIME which one is missing, instead of failing silently. Branch feat/permission-messages off main (quill 0.7.0). PR only, never push main (push to main auto-releases).

# Problem (verified in source)
src/modules/setup.rs `render_hotkey_script` (~line 30) generates ~/.config/quill/hotkey.sh. The Edit>Copy step is `osascript -e 'tell application "System Events"' -e 'try' -e 'click menu item "Copy" ...' -e 'end try' -e 'end tell'`: the `try` swallows EVERY error, so when skhd lacks Accessibility (or Automation for System Events) the copy silently does nothing and `quill --now` then rewrites a STALE clipboard. The only permission text today is a 2-line "Accessibility" hint in `quill setup` (setup.rs ~260), install.sh ~279 and README.

# Permissions (macOS) to document and detect
1 Accessibility -> skhd (System Settings > Privacy & Security > Accessibility). Needed by skhd to receive the global shortcut and for System Events UI scripting (click Edit > Copy); child osascript is attributed to skhd.
2 Automation -> skhd controlling System Events (System Settings > Privacy & Security > Automation). macOS prompts the first time the shortcut runs; user must click Allow.
3 Notifications (optional) -> Script Editor / osascript banners (System Settings > Notifications). Only affects whether failure banners are visible; everything is also written to ~/.config/quill/quill.log.
Not needed: clipboard (pbcopy/pbpaste), ~/.config/quill (user-owned), local Ollama on 127.0.0.1. AI mode needs outbound HTTPS to the configured API URL (no OS permission). If the shortcut still does nothing after 1+2: skhd may also need Input Monitoring (hedge wording: "if it still does nothing").

# Changes
C1 hotkey.sh (render_hotkey_script keeps its signature `(binary: &str, log_path: &str) -> String`): 
  - add `notify() { osascript -e 'on run argv' -e 'display notification (item 1 of argv) with title "Quill"' -- "$1" >/dev/null 2>&1; }` (message passed as argv, never interpolated into AppleScript source).
  - copy step becomes ONE osascript call capturing stdout+stderr: `out=$(osascript -e 'try' -e 'tell application "System Events"' -e 'click menu item "Copy" of menu "Edit" of menu bar 1 of (first application process whose frontmost is true)' -e 'end tell' -e 'return "ok"' -e 'on error errMsg number errNum' -e 'return "ERR " & errNum & ": " & errMsg' -e 'end try' 2>&1)`.
  - classify with `case "$out"`: Accessibility if it contains `-25211`, or contains both `-1719` and `assistive`; Automation if it contains `-1743` or `Not authorized to send Apple events`. (Bare -1719 is NOT a permission error: it is "Invalid index" for apps without an Edit>Copy item.) Check Automation before Accessibility is irrelevant (disjoint patterns).
  - on Accessibility or Automation: append a fixed message line to the log (`printf '%s\n' "[quill] <msg>" >> <log>`), call notify with the same fixed message, `exit 1` WITHOUT running quill (so a stale clipboard is never rewritten).
  - any other result (including "ok" and benign errors like an app without an Edit menu) behaves exactly as today: `sleep 0.1`, then `quill --now 2><log>` with the existing "Quill failed - see ~/.config/quill/quill.log" notification on non-zero exit.
  - fixed messages (no apostrophes, double quotes, backslashes, $ or backticks so they are safe inside single quotes):
    ACC: Quill: skhd needs Accessibility permission. Open System Settings > Privacy & Security > Accessibility and enable skhd, then try again.
    AUT: Quill: skhd is not allowed to control System Events. Open System Settings > Privacy & Security > Automation, enable System Events under skhd, then try again.
  - keep: `#!/bin/sh`, `umask 077`, valid POSIX sh even when the binary/log path contains a space or apostrophe (shell_quote), no "/tmp" in the script, the text `quill.log`.
C2 `quill setup` (macOS) output: replace the 2 Accessibility lines with a checklist printed to stderr (same stream as today): heading "Permissions quill needs (macOS asks; nothing is granted automatically):" then numbered items 1-3 above with the exact System Settings paths, a line "No permission is needed for the clipboard, ~/.config/quill or local Ollama.", a line "If the shortcut does nothing: check these, then ~/.config/quill/quill.log (re-run `quill setup` after upgrading to refresh hotkey.sh)". Must contain the words Accessibility, Automation, System Events, skhd, Notifications, quill.log.
C3 install.sh (macOS step where skhd is present, ~line 279): replace the single Accessibility line with a short 3-line version (Accessibility for skhd, Automation skhd->System Events, optional Notifications) and "see `quill setup` / README for details". Must stay shellcheck-clean (`shellcheck -s sh install.sh`), `sh`-portable.
C4 README: new "Permissions (macOS)" subsection in the macOS setup section (the 3 items, what is not needed, what the user sees when something is missing, how to refresh hotkey.sh) and update the "Hotkey does nothing" troubleshooting bullet. Do not change anything else.
Non-goals: no change to `quill status` output (exact-output tests), no auto-paste feature, no TCC database access, no detection from inside the quill binary.

# Tests (lead writes first)
T1 unit tests in setup.rs `mod tests`: run the generated script under /bin/sh with fake `osascript` and fake quill in a temp dir (PATH=fakebin:/usr/bin:/bin, HOME=temp): (a) osascript prints `ok` -> fake quill invoked once, no notification, exit 0; (b) prints `ERR -25211: ... not allowed assistive access.` -> quill NOT invoked, notification args contain "Accessibility" and "skhd", log contains "Accessibility", exit != 0; (c) `ERR -1719: ... assistive access` -> same as (b); (d) `ERR -1743: Not authorized to send Apple events to System Events.` -> quill NOT invoked, notification contains "Automation" and "System Events", log contains "Automation", exit != 0; (e) `ERR -1719: Can't get menu item Copy ... Invalid index.` (no 'assistive') -> quill invoked (benign), no permission notification; (f) quill fake exits 1 -> notification contains "Quill failed"; (g) script is valid sh (`sh -n`), contains umask 077, quill.log, no "/tmp" literal, still valid when binary path has a space and an apostrophe; (h) messages contain none of ' " \ $ `.
T2 tests/cli.rs (macOS-gated like t17): `quill setup` stderr contains Accessibility, Automation, System Events, skhd, Notifications, quill.log and the "No permission is needed" line; existing t17 still passes.
T3 tests/install_test.sh: if it already exercises the macOS skhd branch, add an assertion for "Automation"; otherwise leave and the lead verifies by reading the diff + running the existing test + shellcheck.
Commands: `cargo fmt --all -- --check`; `cargo clippy --locked --all-targets -- -D warnings`; `cargo test --locked --no-fail-fast` (expect existing 56+36+37 plus new); `shellcheck -s sh install.sh` (if installed); `bash tests/install_test.sh`. Toolchain: PATH="$HOME/.cargo/bin:$PATH" CARGO_BUILD_JOBS=4 (rustc 1.99.1).

# Constraints / blast radius
Files: src/modules/setup.rs, install.sh, README.md, tests (setup.rs tests mod, tests/cli.rs, maybe tests/install_test.sh). Existing users keep the OLD hotkey.sh until `quill setup` is re-run (the curl installer re-runs it). Rollback: revert PR.


# REVISION 2 AMENDMENTS (supersede conflicting text above)
A1 (C1) AppleScript sources become `pub(crate) const` line arrays in setup.rs and are used BOTH to render hotkey.sh and by a macOS-only test that compiles them with `/usr/bin/osacompile -o <tmpdir>/x.scpt -e <line> ...` (compile only, never executed): NOTIFY_APPLESCRIPT = ["on run argv", "display notification (item 1 of argv) with title \"Quill\"", "end run"] (invoked as `osascript -e ... -e ... -e ... -- "$1"`), COPY_APPLESCRIPT = the try / tell System Events / click menu item Copy ... / return "ok" / on error errMsg number errNum / return "ERR " & errNum & ": " & errMsg / end try lines. If osacompile is missing the test prints a notice and passes (so Linux CI is unaffected; the test is cfg(target_os = "macos")).
A2 (C2) Classification of the copy-step output `out` (checked in this order):
  1 Automation denied: contains `-1743` or `Not authorized to send Apple events`  -> STOP with AUT message.
  2 Accessibility denied: contains `-25211`, or (contains `-1719` AND contains `assistive`)  -> STOP with ACC message.
  3 Benign menu failure (app without Edit>Copy): `-1728`, or (contains `-1719` AND (contains `Invalid index` or `Can't get`))  -> continue exactly as today (keeps the copy-manually-then-press-shortcut workflow).
  4 Any other `-1719` (unrecognised, e.g. a localised Accessibility denial)  -> STOP with GEN message: "Quill: could not copy the selection (System Events error -1719). Check Accessibility and Automation permission for skhd in System Settings > Privacy & Security, then try again."  Trade-off (documented in README): on non-English macOS an app without an Edit>Copy item also lands here; workaround = copy with Cmd+C and run `quill --now` from a terminal.
  5 `ok`, or any other result/number -> continue as today (log nothing extra).
  STOP = append the fixed message to the log, notify with the same fixed message, exit 1 without running quill.
A3 Extra tests: (i) localised `ERR -1719: System Events hat einen Fehler festgestellt: osascript darf nicht auf Hilfsgeraete zugreifen.` -> stops, GEN message in notification and log, quill not invoked, exit != 0; (j) `ERR -1728: Can't get menu item Copy of menu Edit` -> continues; (k) `ERR -9999: something unexpected` -> continues, no permission notification; (l) the notification argument is passed via argv: a fake osascript records its args and the test asserts the message appears as the LAST argument after `--` and not inside any -e line; (m) osacompile test for NOTIFY and COPY sources (macOS only).
A4 Priority note for docs: the failure users actually hit is most often Automation (-1743) after clicking "Don't Allow" on the first prompt (Accessibility is already required for skhd to receive the shortcut at all). README/setup text should say how to re-enable: System Settings > Privacy & Security > Automation > skhd > System Events; or `tccutil reset AppleEvents` (resets prompts for all apps; mention as last resort).
# CRITIC FINDINGS
C1 major (notify AppleScript missing end run; need osacompile validation) -> RESOLVED in design Rev 2 (A1); evidence pending: osacompile test
C2 major (localised -1719 permission denial must not fall through to running quill) -> RESOLVED in design Rev 2 (A2 rule 4, A3 (i)); evidence pending: localised fixture test

# REVISION 3: A5 EXACT ARTIFACTS (implement literally)
## setup.rs public/pub(crate) items (names are fixed by the tests already in setup.rs `mod tests`)
pub(crate) const NOTIFY_APPLESCRIPT: &[&str] = &["on run argv", "display notification (item 1 of argv) with title \"Quill\"", "end run"];
pub(crate) const COPY_APPLESCRIPT: &[&str] = &["try", "tell application \"System Events\"", "click menu item \"Copy\" of menu \"Edit\" of menu bar 1 of (first application process whose frontmost is true)", "end tell", "return \"ok\"", "on error errMsg number errNum", "return \"ERR \" & errNum & \": \" & errMsg", "end try"];
pub(crate) const MSG_ACCESSIBILITY: &str = "Quill: skhd needs Accessibility permission. Open System Settings > Privacy & Security > Accessibility and enable skhd, then try again.";
pub(crate) const MSG_AUTOMATION: &str = "Quill: skhd is not allowed to control System Events. Open System Settings > Privacy & Security > Automation, enable System Events under skhd, then try again.";
pub(crate) const MSG_COPY_FAILED: &str = "Quill: could not copy the selection (System Events error -1719). Check Accessibility and Automation permission for skhd in System Settings > Privacy & Security, then try again.";
(They are only referenced by render_hotkey_script and tests: add #[allow(dead_code)] only if clippy demands, never to the test-only osacompile test.)
## Generated hotkey.sh (render_hotkey_script(binary, log_path) keeps its signature). Build it with format!/push_str; every AppleScript line, message, path goes through the existing shell_quote(); -e arguments are joined with " \\\n  " (backslash-newline-two spaces) so the script stays readable.
#!/bin/sh
# Auto-generated by `quill setup`; re-run it to regenerate.
umask 077
notify() {
  osascript -e 'on run argv' -e 'display notification (item 1 of argv) with title "Quill"' -e 'end run' -- "$1" >/dev/null 2>&1
}
stop_with() {
  printf '%s\n' "[quill] $1" >> <shell_quote(log_path)>
  notify "$1"
  exit 1
}
# Copies the selected text via Edit > Copy of the frontmost app, then transforms it.
out=$(osascript -e 'try' \
  -e 'tell application "System Events"' \
  ... one `-e <shell_quote(line)>` per COPY_APPLESCRIPT line ... 2>&1)
case "$out" in
  *-1743*|*"Not authorized to send Apple events"*) stop_with <shell_quote(MSG_AUTOMATION)> ;;
  *-25211*) stop_with <shell_quote(MSG_ACCESSIBILITY)> ;;
  *-1719*assistive*|*assistive*-1719*) stop_with <shell_quote(MSG_ACCESSIBILITY)> ;;
  *-1728*) ;;
  *-1719*"Invalid index"*|*"Invalid index"*-1719*|*-1719*"Can't get"*|*"Can't get"*-1719*) ;;
  *-1719*) stop_with <shell_quote(MSG_COPY_FAILED)> ;;
esac
sleep 0.1
if ! <shell_quote(binary)> --now 2><shell_quote(log_path)>; then
  notify 'Quill failed - see ~/.config/quill/quill.log'
fi
(notify in the real script is the NOTIFY_APPLESCRIPT lines each as `-e <shell_quote(line)>`; the case arms' ORDER matters: Automation, Accessibility, benign, then generic -1719.)
## `quill setup` stderr (setup_macos), replacing the two "Make sure skhd has Accessibility access" eprintln! lines; keep the preceding eprintln!() blank line:
Permissions quill needs on macOS (macOS asks you; nothing is granted automatically):
  1. Accessibility: enable skhd in System Settings > Privacy & Security > Accessibility.
     (skhd needs it to receive the shortcut and to click Edit > Copy in the frontmost app.)
  2. Automation: the first time you press the shortcut, macOS asks whether skhd may control System Events. Click Allow.
     If you clicked Don't Allow, enable it in System Settings > Privacy & Security > Automation > skhd > System Events.
  3. Notifications (optional): to see Quill's failure and permission banners, allow notifications for Script Editor in System Settings > Notifications.
No permission is needed for the clipboard, ~/.config/quill or a local Ollama. AI mode needs outbound HTTPS to your configured API URL.
If the shortcut does nothing: check the items above, then read ~/.config/quill/quill.log (re-run `quill setup` after upgrading to refresh hotkey.sh).
## install.sh (replace the single `say "Grant skhd Accessibility permission: ..."` line, still inside the `if command -v skhd` block):
say "Permissions needed (macOS asks; details are printed by 'quill setup' and in the README):"
say "  1. Accessibility: enable skhd in System Settings > Privacy & Security > Accessibility."
say "  2. Automation: click Allow when macOS asks whether skhd may control System Events (first shortcut press)."
say "  3. Notifications (optional): allow Script Editor in System Settings > Notifications to see Quill banners."
(tests/install_test.sh already asserts case-insensitive 'accessibility', 'automation', 'system events'.)
## README (only these edits): in the macOS setup section add a "### Permissions (macOS)" subsection: the 3 items above, a "not needed" sentence (clipboard, ~/.config/quill, local Ollama; AI mode needs outbound HTTPS), "What you see when something is missing" (a notification with the same text and the same line in ~/.config/quill/quill.log, and quill does NOT run so a stale clipboard is not rewritten; Automation is the one users usually hit: re-enable under Privacy & Security > Automation > skhd > System Events, or as a last resort `tccutil reset AppleEvents` which resets the prompts for all apps), "If skhd never reacts to the shortcut at all, also check Input Monitoring", the non-English limitation (an unrecognised -1719 stops with a generic message; workaround: press Cmd+C yourself and run `quill --now` in a terminal), and "re-run `quill setup` after upgrading to refresh hotkey.sh (the installer does this)". Update the existing "Hotkey does nothing (macOS)" bullet to point at that subsection. No other README change.
# CRITIC FINDINGS (current)
C1 major RESOLVED (design Rev 2 A1; evidence: setup.rs test hotkey_applescript_sources_compile_with_osacompile)
C2 major RESOLVED (design Rev 2 A2 rule 4; evidence: setup.rs test hotkey_unrecognised_1719_stops_with_generic_message)
