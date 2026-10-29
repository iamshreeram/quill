PHASE: done (validator PASS; PR CI green on windows-latest; merge + first real release pending with the owner)
Task: fix Windows release smoke failure + install.ps1 latest-tag/version capture + CI hiding it; branch fix/windows-release-smoke; design .ram-status/windows-release-smoke-20261029T170218Z/DESIGN.md Revision 3
1. design + critic review (3 rounds, round 3 re-run once: first call saw a stale file) -- lead -- done @2026-10-29T17:14:23Z
2. tests first: tests/release_scripts_test.sh r09 + r10 -- lead -- done @2026-10-29T17:14:23Z
3. scripts/windows-fixture-server.py -- ram-implementer -- done @2026-10-29T17:14:23Z
4. scripts/lib-native.ps1 + smoke-windows.ps1 + check-windows-install.ps1 -- ram-implementer -- done @2026-10-29T17:14:23Z
5. install.ps1 + .github/workflows/ci.yml + release.yml -- ram-implementer -- done @2026-10-29T17:14:23Z
6. integrate, local checks, PR, Windows CI as the acceptance gate -- lead -- pending
7. independent validation -- ram-validator -- pending
REVIEW: round=1 by=critic verdict=REJECT blockers=0 majors=1 ts=2026-10-29T17:14:23Z
REVIEW: round=2 by=critic verdict=REJECT blockers=0 majors=1 ts=2026-10-29T17:14:23Z
REVIEW: round=3 by=critic verdict=APPROVE blockers=0 majors=0 ts=2026-10-29T17:14:23Z
OUTCOME: step=3 agent=ram-implementer model=claude-haiku-5-5 first_pass=yes corrections=0 result=done ts=2026-10-29T17:21:22Z
OUTCOME: step=4 agent=ram-implementer model=claude-haiku-5-5 first_pass=no corrections=1 result=done ts=2026-10-29T17:21:22Z
OUTCOME: step=5 agent=ram-implementer model=claude-haiku-5-5 first_pass=yes corrections=0 result=done ts=2026-10-29T17:21:22Z
NOTE: step 4 one lead-side fix after review ($StdinText [string] coercion); Windows execution unverified until CI
REVIEW: round=1 by=validator verdict=PASS blockers=0 majors=0 ts=2026-10-29T17:40:23Z
OUTCOME: step=6 agent=lead model=n/a first_pass=no corrections=2 result=done ts=2026-10-29T17:40:23Z
NOTE: 2 CI-driven fixes (PSModulePath harness artifact; StdinText coercion) + validator minors: r09 regex widened with fixtures, next release is v0.9.0 not v0.8.1. Open minors (not blocking): fixture sends only relative Location; port-pick race; publish path with a Windows artifact proven only by the next real release.
