Revision 4: CRITIC FINDINGS evidence filled in (no design change)
Revision 3: added section 'Interface assumed by test_eval.py' (clarification only, no design change)
Revision 2: addresses critic round 1 (C1-C7): isolated HOME/env, explicit denominators, byte-exact I/O + trim policy, README-rule-based scope labels, richer fake-binary fixtures, workers<=4, loader vs composition validation.

# Goal
Measure how well `quill --stdin --offline` (harper-core, quill 0.6.0 at ~/.local/bin/quill) corrects grammar using a frozen 200-sentence labelled benchmark and a stdlib-only Python script. No network/AI/clipboard/history. Honest metrics; distance-based "improvement" is a PROXY, not a grammaticality judgement.

# Location (new dir only; user's quill/ clone and ~/.config/quill are NOT touched or read by the run)
/Users/s0v06yb/ram/projects/rust/quill-eval/
  dataset.tsv   200 rows, 5 TAB-separated fields: id, category, scope, input, expected. LF endings, no header.
  AUDIT.md      author's per-category README rule mapping + one-error audit, written BEFORE any engine output is seen
  eval.py       python3 stdlib only
  test_eval.py  unittest (written BEFORE eval.py)
  results/      generated: results.jsonl, REPORT.md, home/ (isolated HOME)

# Dataset (frozen before any real-engine output is looked at; golds are never revised based on engine output; report records dataset SHA-256)
scope=in (120): agreement(pronoun-verb)20, a_an15, repeated12, spacing12, capital_start12, pronoun_i12, transposition15 (README rule a: rearranged letters), spelling12 (README rule c: >=7 letters, 1 edit, same first letter), contractions10 (README rule b: only adds an apostrophe)
scope=out (37, not claimed): spelling_limited3 (violate rule c on purpose: <7 letters), verb_form12, homophone12, structure10
scope=none (43; expected==input): clean28, protected15 (URLs, emails, inline code, paths, flags, identifiers, ALLCAPS, names, mid-sentence lowercase tech words; NO JSON rows)
Each error row: exactly ONE intended error, ONE canonical expected. Loader rule: input/expected contain no TAB, CR or LF, and have NO leading/trailing whitespace (quill trims both ends of its output by documented policy, so boundary whitespace is excluded from the benchmark; internal double spaces are kept). Multi-line/CRLF behaviour is covered only by direct fixtures through run_one (TSV cannot hold line breaks).

# Isolation (C1)
Resolve the quill executable to an absolute path FIRST (shutil.which or given path; not found/not executable -> exit 3). Children run with an EXPLICIT env only: {PATH=/usr/bin:/bin, HOME=<out-dir>/home (created empty; no config => default american dialect, offline), LANG=en_US.UTF-8}; every QUILL_* and XDG_* variable is absent. Report records: resolved path, `--version` output, binary SHA-256, dataset SHA-256, dialect=default(american), UTC time.

# run_one(quill_bin, text_bytes_or_str, timeout, env) -> RunResult  (C3, C5)
Popen([quill_bin,"--stdin","--offline"], stdin/stdout/stderr pipes, binary mode, start_new_session=True). Writes the UTF-8 bytes of the input unchanged (no newline added, no text-mode conversion). stdout decoded STRICT UTF-8; exactly one trailing "\n" (or "\r\n") removed -> `output`. Timeout: killpg the process group, reap, status=timeout. Invalid UTF-8 or exit!=0 -> status=exec_error. RunResult: status in {ok, exec_error, timeout}, output (None unless ok), exit_code, stderr (first 200 chars, decoded with replacement), latency_ms.

# Classification (C2) -- exhaustive and mutually exclusive
lev = CHARACTER-level Levenshtein (code points). d_in=lev(input,expected); d_out=lev(output,expected) (null when not ok).
Row kind: ERR if expected!=input, CLEAN if expected==input (must match scope: ERR iff scope!=none; enforced by loader).
Outcome: exec_error (status!=ok) | for ERR rows: fixed (output==expected) -> missed (output==input) -> partial (d_out<d_in) -> harmful (everything else: changed, not equal to expected, d_out>=d_in) | for CLEAN rows: preserved (output==input) | false_positive.
Sets: attempted=all rows; ok=attempted minus exec_error; S_err = ERR rows with status ok; S_clean = CLEAN rows with status ok.
Rates (each reported with numerator/denominator; value is null/"N/A" when denominator==0; JSON never contains NaN/Infinity):
 fix_rate=fixed/|S_err|; improved_rate=(fixed+partial)/|S_err|; miss_rate=missed/|S_err|; harmful_rate=harmful/|S_err|; false_positive_rate=false_positive/|S_clean|; exec_error_rate=exec_error/attempted; error_reduction=1-sum(d_out over S_err)/sum(d_in over S_err) (null if S_err empty; can be negative); helpful_change_precision=(fixed+partial)/(fixed+partial+harmful+false_positive) (null if 0).
Shown overall, per category, per scope(in/out/none), always beside attempted/ok/exec_error counts. Latency median/p95/max over ok rows (null if none).

# CLI / exit codes (C6, C7)
python3 eval.py [--quill-bin PATH] [--dataset PATH=./dataset.tsv] [--workers N (int 1..4, default 4)] [--timeout SEC (finite >0, default 20)] [--out-dir DIR=./results]
0 completed, no exec_error | 1 completed, >=1 exec_error | 2 invalid CLI args (workers outside 1..4, timeout <=0/nan/inf; clear diagnostic on stderr) | 3 quill binary not found/not executable | 4 dataset invalid (field count!=5, empty/duplicate id, scope not in {in,out,none}, ERR/CLEAN vs scope mismatch, forbidden chars, boundary whitespace). Generic loader does NOT require 200 rows; the fixed composition (200 rows, per-category and per-scope counts above) is asserted by test_eval.py against the frozen dataset.
Results ordered by id; at most `workers` quill processes in flight at once. Never uses --now/clipboard.
results.jsonl fields: id, category, scope, input, expected, output, status, exit_code, stderr, outcome, d_in, d_out, latency_ms. REPORT.md: header (provenance above), headline table, per-category, per-scope, listings of every false_positive/harmful/exec_error row and the first 10 missed rows per category, and a "limitations" section (synthetic single-error sentences, single canonical gold, conservative-by-design engine, n=200 indicative only).

# Acceptance (test_eval.py) -- fixtures use fake quill scripts in a tmp dir
unit: lev (identity, empty, insert/delete/sub, non-ASCII); classification boundaries: fixed, missed, partial, equal-distance change => harmful, worse change => harmful, clean preserved, clean mutated => false_positive.
rates: exact numerators/denominators and null values for clean-only, ERR-only, mixed success/failure, all-exec_error datasets; REPORT.md/results.jsonl valid, no NaN.
loader: rejects wrong field count, duplicate id, unknown scope, scope/ERR mismatch, tab/CR/LF, boundary whitespace; accepts a small valid file (no 200-row requirement). Frozen benchmark composition test (200 rows, counts per category/scope above).
end-to-end fakes: identity => fixed 0/preserved 43/missed 120+37=157; oracle (maps input->expected) => fixed 157/preserved 43/false_positive 0; partial fake; harmful fake; clean-mutation fake; exit-1 fake => all exec_error + script exit 1; invalid-UTF-8 fake => exec_error; timeout fake => outcome exec_error/timeout AND the child pid (fake writes it to a file) is no longer alive; missing binary => exit 3; bad dataset => exit 4; bad args (workers 0/5, timeout 0/-1/nan/inf) => exit 2.
isolation: env-dump fake proves HOME==out-dir/home, no QUILL_*/XDG_* vars, PATH explicit. Concurrency: instrumented fake observes <=4 concurrent children with --workers 4 and exactly 1 with --workers 1.
I/O fidelity: direct run_one fixtures with (cat-like fake) input "a  b" , non-ASCII/combining chars, internal CRLF "x\r\ny" => bytes preserved; output newline removal strips exactly one.
Real-run smoke (Phase 5, by lead): before/after listing of sha256+mode of ~/.config/quill/* identical; ~/.config/quill never opened by the run.

# Constraints / blast radius
New files only under quill-eval/ (+ .ram-status). No installs, no network, no system config. <=4 concurrent short-lived quill processes; no polling loops. Rollback: rm -r quill-eval.

# Steps (sequenced: dataset freeze/audit -> tests -> worker -> real run)
1 dataset.tsv + AUDIT.md (lead) 2 test_eval.py (lead) 3 eval.py (ram-implementer, Haiku) 4 real run + analysis (lead) 5 independent validation (ram-validator)

# CRITIC FINDINGS (round 1 verbatim summaries; status after Revision 2)
C1 major HOME/ambient config side effects -> RESOLVED in design (Isolation section); evidence pending: isolation test + real-run before/after hash check
C2 major ambiguous denominators / div-by-zero -> RESOLVED in design (Classification section); evidence pending: rates tests
C3 major whitespace/trim/CRLF/UTF-8 normalisation -> RESOLVED in design (Dataset loader rule, run_one); evidence pending: I/O fidelity tests
C4 major spelling scope labels / JSON protected rows -> RESOLVED in design (spelling vs spelling_limited split; JSON dropped); evidence pending: AUDIT.md
C5 major weak negative-outcome/timeout/invalid-UTF-8 fixtures -> RESOLVED in design (Acceptance section); evidence pending: test_eval.py
C6 minor workers<=4, arg validation -> RESOLVED in design (CLI); evidence pending: tests
C7 minor loader vs composition validation, sequencing, no gold revision, dataset SHA -> RESOLVED in design; evidence pending


# Interface assumed by test_eval.py (Revision 3; the tests are authoritative)
Module-level API in eval.py (imported by path, so the file name `eval.py` is fine):
  lev(a, b) -> int                         character (code point) Levenshtein
  classify(inp, exp, out) -> str           one of fixed|missed|partial|harmful (when exp != inp) or preserved|false_positive (when exp == inp); precedence for ERR rows: fixed -> missed -> partial -> harmful
  class DatasetError(ValueError)
  load_dataset(path) -> list[dict]         keys id, category, scope, input, expected; raises DatasetError for every invalid case in the tests (also for a missing file: DatasetError or OSError accepted by tests, but the CLI maps it to exit 4)
  run_one(quill_bin, text, timeout, env) -> obj with attrs status ('ok'|'exec_error'|'timeout'), output (str|None), exit_code (int|None), stderr (str, <=200 chars), latency_ms (number); text may be str or bytes
  main(argv=None) -> int  (CLI entry; `python3 eval.py ...` exits with it)
Results are written in DATASET FILE ORDER. Files in --out-dir: results.jsonl, REPORT.md, summary.json (valid JSON, never NaN/Infinity: use null), home/ (isolated HOME).
summary.json schema:
  attempted, ok, exec_error (ints); counts: dict with ALL of fixed, partial, missed, harmful, preserved, false_positive, exec_error (zeros included)
  rates: fix_rate, improved_rate, miss_rate, harmful_rate, false_positive_rate, exec_error_rate, helpful_change_precision -> {num, den, value}; error_reduction -> {sum_d_in, sum_d_out, value}; value=null when den==0 (error_reduction: when no successful ERR row)
  latency_ms: {median, p95, max} over ok rows (null when none)
  per_category: {cat: {attempted, ok, exec_error, counts, rates}} ; per_scope: {in|out|none: same shape} (all three scope keys always present)
  provenance: {dataset_sha256, binary_path (resolved absolute), binary_sha256, quill_version (stripped stdout of `<bin> --version` in the isolated env, or "unknown"), dialect ("default (american)"), utc_time (ISO-8601)}
REPORT.md section headings (case-insensitive substrings the tests look for): 'Per category', 'Per scope', 'Limitations'; metric names written literally (fix_rate, ...); failing rows listed with their input text; dataset sha256 printed.
Each failure listing prints input, expected, output (or status/stderr for exec_error).


# CRITIC FINDINGS -- evidence (Revision 4)
C1 RESOLVED  test_eval.IsolationAndConcurrency.test_child_env_is_explicit_and_home_is_isolated; real run: sha256+mode of ~/.config/quill/* identical before/after (/tmp/quill-cfg-before.txt vs after, diff empty)
C2 RESOLVED  test_eval.MixedOutcomes.test_counts_and_rates_exact, test_zero_denominators_are_null_per_category_and_scope, ExecFailures.test_all_exec_errors_give_null_rates_and_exit_1, test_mixed_success_and_failure_excludes_failures_from_denominators
C3 RESOLVED  test_eval.RunOneTests.test_passthrough_is_byte_exact, test_exactly_one_trailing_newline_removed, FidelityThroughCli, LoaderTests.test_invalid_variants (boundary whitespace/CR rejected)
C4 RESOLVED  quill-eval/AUDIT.md (README rule per category; spelling vs spelling_limited split; no JSON rows); FrozenDatasetTests.test_frozen_sha256
C5 RESOLVED  ClassifyTests, RunOneTests.test_invalid_utf8_is_exec_error / test_timeout_kills_and_reaps_child, ExecFailures.test_timeout_through_cli_kills_child, MixedOutcomes (partial/harmful/false_positive)
C6 RESOLVED  ExitCodes.test_bad_args_are_2, test_valid_workers_boundaries_accepted, IsolationAndConcurrency.test_workers_bound_concurrency
C7 RESOLVED  LoaderTests (generic loader, no 200-row requirement) vs FrozenDatasetTests.test_composition; dataset sha256 recorded in results/summary.json provenance; golds not revised after seeing engine output
