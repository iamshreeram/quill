//! Offline grammar engine (DESIGN.md Rev 4, section A1).
//!
//! Pure module: no filesystem, network, process or environment access, and no printing.
//! Built on `harper-core` (native Rust linter). A conservative safety policy wraps the linter so
//! that URLs, emails, code, flags, identifiers and unknown names are never rewritten.

use std::str::FromStr;

use harper_core::linting::{Lint, LintGroup, LintKind, Suggestion};
use harper_core::parsers::PlainEnglish;
use harper_core::spell::FstDictionary;
use harper_core::{remove_overlaps, Dialect as HarperDialect, Document, Span};

/// Maximum input length in characters accepted by [`correct`].
pub const MAX_CHARS: usize = 100_000;

/// Maximum number of correction passes per call.
const MAX_PASSES: usize = 3;

const TOO_LONG: &str = "text too long for offline mode (max 100000 chars)";

/// English dialect used for spelling and agreement rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Dialect {
    #[default]
    American,
    British,
    Australian,
    Canadian,
    Indian,
}

impl FromStr for Dialect {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "american" => Ok(Dialect::American),
            "british" => Ok(Dialect::British),
            "australian" => Ok(Dialect::Australian),
            "canadian" => Ok(Dialect::Canadian),
            "indian" => Ok(Dialect::Indian),
            _ => Err(format!(
                "unknown dialect '{s}' (expected american, british, australian, canadian or indian)"
            )),
        }
    }
}

impl From<Dialect> for HarperDialect {
    fn from(d: Dialect) -> Self {
        match d {
            Dialect::American => HarperDialect::American,
            Dialect::British => HarperDialect::British,
            Dialect::Australian => HarperDialect::Australian,
            Dialect::Canadian => HarperDialect::Canadian,
            Dialect::Indian => HarperDialect::Indian,
        }
    }
}

/// Result of a successful [`correct`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Correction {
    pub text: String,
    pub fixes: usize,
}

/// Lint kinds that may produce an automatic edit.
///
/// Final list (from harper-core 2.13.0 `LintKind`, 21 variants):
///   ALLOWED: Capitalization, Punctuation, Formatting, Repetition, Agreement, Grammar, Typo,
///            Miscellaneous (needed for a/an), Spelling (further restricted in `spelling_ok`).
///   DENIED (everything else): BoundaryError, Eggcorn, Enhancement, Malapropism, Nonstandard,
///            Readability, Redundancy, Regionalism, Style, Usage, WordChoice, WordOrder.
const ALLOWED_KINDS: &[LintKind] = &[
    LintKind::Capitalization,
    LintKind::Punctuation,
    LintKind::Formatting,
    LintKind::Repetition,
    LintKind::Agreement,
    LintKind::Grammar,
    LintKind::Typo,
    LintKind::Miscellaneous,
    LintKind::Spelling,
];

/// Rule families allowed to produce edits (DESIGN Rev 9), keyed by harper rule name.
/// Everything not listed is off. NEVER add NominalWants, SplitWords, word-swap, expansion,
/// tense or negation rules.
const ALLOWED_RULES: &[&str] = &[
    // spelling; each edit further restricted by rule (c) (Rev 7)
    "SpellCheck",
    // "Fixes especially common misspellings of the word `the`" (teh->the)
    "The",
    // sentence-initial capital letter; restricted by rule (b') (Rev 8)
    "SentenceCapitalization",
    // i -> I / i'm -> I'm; restricted by rule (b') (Rev 8)
    "CapitalizePersonalPronouns",
    // a / an article agreement
    "AnA",
    // "the the" -> "the"
    "RepeatedWords",
    // at most one space between words (double spaces)
    "Spaces",
    // comma spacing (space before comma, missing space after comma)
    "CommaFixes",
    // "I has" -> "I have" (pronoun/verb agreement)
    "PronounVerbAgreement",
    // "he are" -> "he is" (pronoun/be agreement)
    "PronounInflectionBe",
    // "I are" -> "I am"
    "IAmAgreement",
    // "I does" -> "I do"
    "IDo",
];

const NEGATIVE_CONTRACTIONS: &[&str] = &[
    "cant", "dont", "wont", "isnt", "arent", "wasnt", "werent", "hasnt", "havent", "hadnt",
    "doesnt", "didnt", "couldnt", "wouldnt", "shouldnt", "mustnt",
];

const NEGATIVE_WORDS: &[&str] = &["not", "no", "never", "neither", "nor", "without", "cannot"];

/// Correct `input` offline. Up to three passes; each pass re-lints the text produced by the
/// previous one. Only edits that satisfy the safety policy are applied.
pub fn correct(input: &str, dialect: Dialect) -> Result<Correction, String> {
    if input.chars().count() > MAX_CHARS {
        return Err(TOO_LONG.to_string());
    }
    if input.trim().is_empty() {
        return Ok(Correction {
            text: input.to_string(),
            fixes: 0,
        });
    }

    // Built once per call; the dictionary is shared by Arc.
    let mut linter = LintGroup::new_curated(FstDictionary::curated(), dialect.into());
    let mut text = input.to_string();
    let mut fixes = 0;

    for _ in 0..MAX_PASSES {
        let chars: Vec<char> = text.chars().collect();
        let (out_chars, applied) = one_pass(&chars, &mut linter);
        if applied == 0 {
            break;
        }
        text = out_chars.iter().collect();
        fixes += applied;
    }

    Ok(Correction { text, fixes })
}

/// One lint/apply pass over `chars`. Returns the edited text and the number of applied edits.
fn one_pass(chars: &[char], linter: &mut LintGroup) -> (Vec<char>, usize) {
    let text: String = chars.iter().collect();
    let prot = protected_ranges(chars);
    let doc = Document::new_curated(&text, &PlainEnglish);
    // Rule-name allowlist (DESIGN Rev 9): only vetted mechanical rule families run.
    let mut lints: Vec<Lint> = linter
        .organized_lints(&doc)
        .into_iter()
        .filter(|(name, _)| ALLOWED_RULES.contains(&name.as_str()))
        .flat_map(|(_, found)| found)
        .collect();
    lints.retain(|l| lint_is_safe(l, chars, &prot));
    remove_overlaps(&mut lints);
    // Right-to-left so earlier offsets stay valid.
    lints.sort_by_key(|l| std::cmp::Reverse((l.span.start, l.span.end)));

    apply_guarded(chars, &lints)
}

/// Applies `lints` (sorted by descending span start) under the end-of-pass invariant (A1 (a)).
/// If applying all edits breaks a protected token, each edit is tested alone and only the edits
/// that keep the invariant are applied together. The pass is discarded if that still fails.
fn apply_guarded(chars: &[char], lints: &[Lint]) -> (Vec<char>, usize) {
    let (all, n) = apply_edits(chars, lints, |_| true);
    if keeps_protected(chars, &all) {
        return (all, n);
    }
    let keep: Vec<bool> = (0..lints.len())
        .map(|i| {
            let (alone, _) = apply_edits(chars, lints, |j| j == i);
            keeps_protected(chars, &alone)
        })
        .collect();
    let (subset, m) = apply_edits(chars, lints, |i| keep[i]);
    if keeps_protected(chars, &subset) {
        (subset, m)
    } else {
        (chars.to_vec(), 0)
    }
}

/// Applies the lints selected by `keep`, right-to-left. Returns the text and the applied count.
fn apply_edits(chars: &[char], lints: &[Lint], keep: impl Fn(usize) -> bool) -> (Vec<char>, usize) {
    let mut out = chars.to_vec();
    let mut applied = 0;
    for (i, lint) in lints.iter().enumerate() {
        if !keep(i) {
            continue;
        }
        if let Some(sugg) = lint.suggestions.first() {
            sugg.apply(Span::new(lint.span.start, lint.span.end), &mut out);
            applied += 1;
        }
    }
    (out, applied)
}

/// Invariant (Rev 6): every protected token of `input` appears, in order, among the protected
/// tokens of `output`. New protected tokens are allowed.
fn keeps_protected(input: &[char], output: &[char]) -> bool {
    is_subsequence(&protected_tokens(input), &protected_tokens(output))
}

fn is_subsequence(needle: &[String], hay: &[String]) -> bool {
    let mut it = hay.iter();
    needle.iter().all(|n| it.any(|h| h == n))
}

/// Safety policy (a)-(e) for a single lint. Uses only the FIRST suggestion.
fn lint_is_safe(
    lint: &harper_core::linting::Lint,
    chars: &[char],
    prot: &[(usize, usize)],
) -> bool {
    // (b) kind allowlist
    if !ALLOWED_KINDS.contains(&lint.lint_kind) {
        return false;
    }
    // (d) must have a suggestion
    let Some(sugg) = lint.suggestions.first() else {
        return false;
    };
    let (s, e) = (lint.span.start, lint.span.end);
    if s > e || e > chars.len() {
        return false;
    }
    let (repl, s, e): (Vec<char>, usize, usize) = match sugg {
        Suggestion::ReplaceWith(v) => (v.clone(), s, e),
        Suggestion::InsertAfter(v) => (v.clone(), e, e),
        Suggestion::Remove => (Vec::new(), s, e),
    };
    // (b') Capitalization restriction (DESIGN Rev 8)
    if lint.lint_kind == LintKind::Capitalization {
        match sugg {
            Suggestion::ReplaceWith(v) if capitalization_ok(chars, s, e, v) => {}
            _ => return false,
        }
    }
    // (G1)/(G2) structural guards on every edit (DESIGN Rev 9).
    if !no_word_split(&chars[s..e], &repl) || !negation_guard(&chars[s..e], &repl) {
        return false;
    }
    // (c) spelling restriction
    if lint.lint_kind == LintKind::Spelling && !spelling_ok(&chars[s..e], sugg) {
        return false;
    }
    // (a) + (e)
    edit_ok(s, e, &repl, chars, prot)
}

/// (b') Capitalization edits (DESIGN Rev 8). Applied only when the edit merely upper-cases the
/// first letter AND either the token is `i` or an i-contraction (i'm, i'll, i've, i'd), or the
/// token is sentence-initial (start of text, or the previous non-whitespace char is `.`, `!` or
/// `?`, or it starts a line that follows a blank line). Mid-sentence words such as yaml, docker
/// or postgres are never capitalized.
fn capitalization_ok(chars: &[char], s: usize, e: usize, v: &[char]) -> bool {
    upper_first_only(&chars[s..e], v) && (is_i_form(&chars[s..e]) || sentence_initial(chars, s))
}

/// True when `v` equals `token` with only its first letter upper-cased.
fn upper_first_only(token: &[char], v: &[char]) -> bool {
    let (Some(&t0), Some(&v0)) = (token.first(), v.first()) else {
        return false;
    };
    let mut up = t0.to_uppercase();
    v.len() == token.len()
        && token[1..] == v[1..]
        && up.next() == Some(v0)
        && up.next().is_none()
        && t0 != v0
}

fn is_i_form(token: &[char]) -> bool {
    let s: String = token.iter().collect();
    s == "i" || ["i'm", "i'll", "i've", "i'd"].contains(&s.as_str())
}

/// Sentence-initial: start of text, after `.`/`!`/`?`, or the first word of a paragraph.
fn sentence_initial(chars: &[char], s: usize) -> bool {
    match chars[..s].iter().rposition(|c| !c.is_whitespace()) {
        None => true,
        Some(p) if matches!(chars[p], '.' | '!' | '?') => true,
        Some(_) => starts_paragraph(chars, s),
    }
}

/// True when `s` begins a line (only indentation before it) and the previous line is blank.
fn starts_paragraph(chars: &[char], s: usize) -> bool {
    let ls = line_start(chars, s);
    if ls == 0 || !chars[ls..s].iter().all(|c| c.is_whitespace()) {
        return false;
    }
    let prev_end = ls - 1;
    let prev_start = line_start(chars, prev_end);
    chars[prev_start..prev_end]
        .iter()
        .all(|c| c.is_whitespace())
}

/// G1 (DESIGN Rev 9): a non-empty purely alphabetic/apostrophe span must not be replaced by text
/// containing whitespace (no word splitting, e.g. wont->w ont, asap->as soon as possible).
fn no_word_split(orig: &[char], repl: &[char]) -> bool {
    let word_like = !orig.is_empty() && orig.iter().all(|c| c.is_alphabetic() || *c == '\'');
    !(word_like && repl.iter().any(|c| c.is_whitespace()))
}

/// G2 (DESIGN Rev 9): negative contractions may only receive the apostrophe-insertion form
/// (wont->won't, dont->don't) and never another word; an edit must not drop a negation word.
fn negation_guard(orig: &[char], repl: &[char]) -> bool {
    let ws = words(orig);
    if ws
        .iter()
        .any(|w| NEGATIVE_CONTRACTIONS.contains(&w.as_str()))
    {
        return ws.len() == 1
            && orig.len() == ws[0].chars().count()
            && is_one_apostrophe_insert(orig, repl);
    }
    negation_count(orig) == 0 || negation_count(repl) > 0
}

fn negation_count(text: &[char]) -> usize {
    words(text)
        .iter()
        .filter(|w| NEGATIVE_WORDS.contains(&w.as_str()) || w.ends_with("n't"))
        .count()
}

/// Lowercased alphabetic/apostrophe words of `text`.
fn words(text: &[char]) -> Vec<String> {
    text.split(|c| !(c.is_alphabetic() || *c == '\''))
        .filter(|w| !w.is_empty())
        .map(|w| w.iter().collect::<String>().to_lowercase())
        .collect()
}

/// (c) Spelling edits (DESIGN Rev 7). A Spelling lint is applied only when ALL hold:
/// the token is lowercase alphabetic (plus apostrophes), len >= 3, and the first suggestion is
/// purely alphabetic (plus apostrophes), AND one of:
///   (A) the suggestion is a permutation of the token's letters (teh->the, hte->the);
///   (B) token len >= 4 and the suggestion is the token with exactly one apostrophe inserted
///       (dont->don't);
///   (C) token len >= 7, Levenshtein distance <= 1, suggestion len >= token len, same first
///       letter (grammer->grammar, occured->occurred).
/// Everything else (slang such as thx/pls/tmrw, names, short words like docker->docket) is left
/// untouched. Capitalized unknown words are never corrected (likely names).
fn spelling_ok(token: &[char], sugg: &Suggestion) -> bool {
    let Suggestion::ReplaceWith(v) = sugg else {
        return false;
    };
    let token_is_word = token
        .iter()
        .all(|c| *c == '\'' || (c.is_alphabetic() && c.is_lowercase()));
    let sugg_is_word = v.iter().all(|c| *c == '\'' || c.is_alphabetic());
    if !(token_is_word && sugg_is_word && token.len() >= 3) {
        return false;
    }
    let permutation = is_permutation(token, v);
    let one_apostrophe = token.len() >= 4 && is_one_apostrophe_insert(token, v);
    let near_typo = token.len() >= 7
        && levenshtein(token, v) <= 1
        && v.len() >= token.len()
        && token.first() == v.first();
    permutation || one_apostrophe || near_typo
}

/// True when `v` is `token` with exactly one apostrophe inserted (and `token` has none).
fn is_one_apostrophe_insert(token: &[char], v: &[char]) -> bool {
    if token.contains(&'\'') || v.len() != token.len() + 1 {
        return false;
    }
    let positions: Vec<usize> = v
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == '\'')
        .map(|(i, _)| i)
        .collect();
    match positions.as_slice() {
        [i] => {
            let mut stripped = v.to_vec();
            stripped.remove(*i);
            stripped == token
        }
        _ => false,
    }
}

/// True when `a` and `b` contain the same multiset of chars.
fn is_permutation(a: &[char], b: &[char]) -> bool {
    let mut x = a.to_vec();
    let mut y = b.to_vec();
    x.sort_unstable();
    y.sort_unstable();
    x == y
}

/// (a) protected spans and (e) whitespace/indentation rules for an edit replacing `chars[s..e]`
/// with `repl`.
fn edit_ok(s: usize, e: usize, repl: &[char], chars: &[char], prot: &[(usize, usize)]) -> bool {
    // (e) no line breaks or tabs in the replaced text or the replacement.
    let has_forbidden = |c: &char| matches!(c, '\n' | '\r' | '\t');
    if chars[s..e].iter().any(has_forbidden) || repl.iter().any(has_forbidden) {
        return false;
    }
    // (e) never touch line indentation.
    let ls = line_start(chars, s);
    if s < indent_end(chars, ls) {
        return false;
    }
    // (a) protected spans: no overlap; touching only allowed for whitespace-only edits.
    let whitespace_only =
        chars[s..e].iter().all(|c| c.is_whitespace()) && repl.iter().all(|c| c.is_whitespace());
    for &(ps, pe) in prot {
        if s < pe && ps < e {
            return false;
        }
        let touches = e == ps || s == pe;
        if touches && !whitespace_only {
            return false;
        }
    }
    true
}

/// Protected char ranges (sorted, merged) for `chars`.
/// Covers fenced code blocks, indented code lines (after a blank line), inline code spans,
/// and identifier-like whitespace-delimited tokens (URLs, emails, flags, paths, identifiers).
fn protected_ranges(chars: &[char]) -> Vec<(usize, usize)> {
    let lines = line_bounds(chars);
    let mut fence_mask = vec![false; chars.len()];
    let mut ranges: Vec<(usize, usize)> = Vec::new();

    // Fenced code blocks: from an opening fence line to the matching closing fence (or EOF).
    let mut i = 0;
    while i < lines.len() {
        if is_fence_line(chars, lines[i]) {
            let mut last = i + 1;
            while last < lines.len() && !is_fence_line(chars, lines[last]) {
                last += 1;
            }
            let last = last.min(lines.len() - 1);
            let (start, end) = (lines[i].0, lines[last].1);
            ranges.push((start, end));
            for m in fence_mask[start..end].iter_mut() {
                *m = true;
            }
            i = last + 1;
        } else {
            i += 1;
        }
    }

    // Indented code: a non-blank line starting with 4 spaces or a tab, right after a blank line.
    for k in 1..lines.len() {
        let (s, e) = lines[k];
        if s >= e || fence_mask[s] {
            continue;
        }
        let (ps, pe) = lines[k - 1];
        let prev_blank = chars[ps..pe].iter().all(|c| c.is_whitespace());
        let indented = chars[s..e].starts_with(&[' ', ' ', ' ', ' ']) || chars[s] == '\t';
        if prev_blank && indented {
            ranges.push((s, e));
        }
    }

    // Inline code: matching backtick pairs outside fenced blocks.
    let mut i = 0;
    while i < chars.len() {
        if !fence_mask[i] && chars[i] == '`' {
            let mut j = i + 1;
            while j < chars.len() && !fence_mask[j] && chars[j] != '`' {
                j += 1;
            }
            if j < chars.len() && !fence_mask[j] {
                ranges.push((i, j + 1));
                i = j + 1;
                continue;
            }
        }
        i += 1;
    }

    // Identifier-like whitespace-delimited tokens.
    let mut k = 0;
    while k < chars.len() {
        if chars[k].is_whitespace() {
            k += 1;
            continue;
        }
        let start = k;
        while k < chars.len() && !chars[k].is_whitespace() {
            k += 1;
        }
        if token_is_protected(&chars[start..k]) {
            ranges.push((start, k));
        }
    }

    // Sort and merge overlapping/adjacent ranges.
    ranges.sort();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (s, e) in ranges {
        match merged.last_mut() {
            Some(last) if s <= last.1 => {
                if e > last.1 {
                    last.1 = e;
                }
            }
            _ => merged.push((s, e)),
        }
    }
    merged
}

/// Identifier-like test for a whitespace-delimited token (see DESIGN A1 (a)).
fn token_is_protected(tok: &[char]) -> bool {
    if tok.first() == Some(&'-') {
        return true; // flags such as --flag
    }
    if tok
        .iter()
        .any(|c| c.is_numeric() || matches!(c, '_' | '/' | '\\' | '@' | '#' | '$' | '='))
    {
        return true;
    }
    let text: String = tok.iter().collect();
    if text.contains("::")
        || text.contains("->")
        || text.contains("://")
        || text.to_ascii_lowercase().starts_with("www.")
    {
        return true;
    }
    // a '.' between two alphanumerics (file.rs, a.b, example.com)
    if tok
        .windows(3)
        .any(|w| w[1] == '.' && w[0].is_alphanumeric() && w[2].is_alphanumeric())
    {
        return true;
    }
    // camelCase / PascalCase / mixed case: an uppercase letter after the first character
    if tok.iter().skip(1).any(|c| c.is_uppercase()) {
        return true;
    }
    // ALLCAPS of length >= 2
    let letters: Vec<char> = tok.iter().copied().filter(|c| c.is_alphabetic()).collect();
    letters.len() >= 2 && letters.iter().all(|c| c.is_uppercase())
}

/// The protected token strings of a text, in order. Used by the end-of-pass invariant.
fn protected_tokens(chars: &[char]) -> Vec<String> {
    protected_ranges(chars)
        .iter()
        .map(|&(s, e)| chars[s..e].iter().collect())
        .collect()
}

/// (start, end) char index of each line, end exclusive and excluding the '\n'.
fn line_bounds(chars: &[char]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (i, &c) in chars.iter().enumerate() {
        if c == '\n' {
            out.push((start, i));
            start = i + 1;
        }
    }
    out.push((start, chars.len()));
    out
}

fn is_fence_line(chars: &[char], (s, e): (usize, usize)) -> bool {
    let text: String = chars[s..e].iter().collect();
    text.trim_start().starts_with("```")
}

/// Index of the first char of the line containing `pos`.
fn line_start(chars: &[char], pos: usize) -> usize {
    chars[..pos]
        .iter()
        .rposition(|&c| c == '\n')
        .map_or(0, |p| p + 1)
}

/// Index of the first char after the leading spaces/tabs of the line starting at `ls`.
fn indent_end(chars: &[char], ls: usize) -> usize {
    let mut i = ls;
    while i < chars.len() && (chars[i] == ' ' || chars[i] == '\t') {
        i += 1;
    }
    i
}

/// Classic Levenshtein distance over chars.
fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j] + cost).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fix(s: &str) -> String {
        correct(s, Dialect::American).expect("correct").text
    }

    const SAMPLE: &str = "this is an test  of the grammer engine,it dont work good . i has two cat";

    #[test]
    fn grammar_basic_fixes() {
        let out = fix(SAMPLE);
        assert!(
            out.contains("This is a test of the grammar engine, it"),
            "{out}"
        );
        assert!(out.contains(". I have"), "{out}");
        assert!(!out.contains("  "), "double space left: {out}");
    }

    #[test]
    fn grammar_already_correct_unchanged() {
        let s = "This is a perfectly fine sentence.\n";
        let c = correct(s, Dialect::American).unwrap();
        assert_eq!(c.text, s);
        assert_eq!(c.fixes, 0);
    }

    #[test]
    fn grammar_crlf_tabs_blank_lines_preserved() {
        let input =
            "First line is fine.\r\n\r\nsecond line has a typo teh word.\r\n\tTabbed line is fine.";
        let out = fix(input);
        assert!(out.contains("First line is fine.\r\n\r\n"), "{out:?}");
        assert!(
            out.contains("typo the word.\r\n\tTabbed line is fine."),
            "{out:?}"
        );
    }

    #[test]
    fn grammar_unicode_and_emoji_survive() {
        let out = fix("caf\u{e9} is nice \u{1F642} and teh end");
        assert!(
            out.contains("caf\u{e9} is nice \u{1F642} and the end")
                || out.contains("Caf\u{e9} is nice \u{1F642} and the end"),
            "{out}"
        );
    }

    #[test]
    fn grammar_protected_spans_untouched() {
        let input = "check https://example.com/teh_path?x=1, and mail bob@exmple.com then run `teh --flgs` and Quill, snake_case_wrd, CamelCaseWrd, foo.rs plus teh typo.";
        let out = fix(input);
        for protected in [
            "https://example.com/teh_path?x=1",
            "bob@exmple.com",
            "`teh --flgs`",
            "Quill,",
            "snake_case_wrd",
            "CamelCaseWrd",
            "foo.rs",
        ] {
            assert!(out.contains(protected), "lost {protected:?} in: {out}");
        }
        assert!(out.contains("plus the typo."), "{out}");
    }

    #[test]
    fn grammar_unknown_capitalized_name_untouched() {
        let out = fix("Please ask Zyxwvu about teh report.");
        assert!(out.contains("Zyxwvu about the report"), "{out}");
    }

    #[test]
    fn grammar_fenced_code_block_untouched() {
        let input = "Intro text is fine.\n\n```\nlet x = teh;\n```\n\nand here teh typo.";
        let out = fix(input);
        assert!(out.contains("```\nlet x = teh;\n```"), "{out}");
        assert!(out.contains("here the typo"), "{out}");
    }

    #[test]
    fn grammar_multipass_protected_spans_intact() {
        let input = "i  has a  teh problem with https://a.example/x_y and  snake_case_id and `code  here`.  i  has  teh  other  with  https://b.example/q_r  and  `x  y`.";
        let out = fix(input);
        for protected in [
            "https://a.example/x_y",
            "snake_case_id",
            "`code  here`",
            "https://b.example/q_r",
            "`x  y`",
        ] {
            assert!(out.contains(protected), "lost {protected:?} in: {out}");
        }
        assert!(out.contains("I have"), "{out}");
    }

    #[test]
    fn grammar_idempotent() {
        let once = fix(SAMPLE);
        assert_eq!(fix(&once), once);
    }

    #[test]
    fn grammar_too_long_is_error() {
        let big = "a".repeat(MAX_CHARS + 1);
        let err = correct(&big, Dialect::American).unwrap_err();
        assert!(err.contains("too long"), "{err}");
    }

    #[test]
    fn grammar_empty_is_unchanged() {
        for s in ["", "   \n\t "] {
            let c = correct(s, Dialect::American).unwrap();
            assert_eq!(c.text, s);
            assert_eq!(c.fixes, 0);
        }
    }

    #[test]
    fn grammar_slang_is_not_replaced_but_typos_are() {
        let out = fix("Please send me teh file asap, thx. Then ok the plan, pls.");
        assert!(out.contains("thx"), "{out}");
        assert!(out.contains("pls"), "{out}");
        assert!(out.contains("the file"), "{out}");
    }

    #[test]
    fn grammar_len3_permutation_typo_is_fixed() {
        // harper's first suggestion for `hte` is `the`, a letter permutation (DESIGN Rev 5 (c)).
        let out = fix("I saw hte dog today.");
        assert!(out.contains("the dog"), "{out}");
    }

    #[test]
    fn grammar_spelling_len3_rule() {
        let rep = |s: &str| Suggestion::ReplaceWith(s.chars().collect());
        let chars = |s: &str| s.chars().collect::<Vec<char>>();
        // permutation of letters: allowed
        assert!(spelling_ok(&chars("teh"), &rep("the")));
        // not a permutation (thx -> the): rejected
        assert!(!spelling_ok(&chars("thx"), &rep("the")));
        // length 2 is never allowed
        assert!(!spelling_ok(&chars("ok"), &rep("on")));
        // len >= 4 unchanged behaviour
        assert!(spelling_ok(&chars("wrod"), &rep("word")));
    }

    #[test]
    fn grammar_guard_applies_safe_edits_and_drops_breaking_one() {
        // The Remove breaks the URL token (`https://example.com/page .` -> `.../page.`);
        // the ReplaceWith is safe and must still be applied.
        let s = "Visit https://example.com/page ., then teh end.";
        let chars: Vec<char> = s.chars().collect();
        let url_end = s.find(" .").unwrap();
        let teh = s.find("teh").unwrap();
        let mk = |start: usize, end: usize, sugg: Suggestion| Lint {
            span: Span::new(start, end),
            lint_kind: LintKind::Punctuation,
            suggestions: vec![sugg],
            message: String::new(),
            priority: 0,
        };
        let mut lints = vec![
            mk(url_end, url_end + 1, Suggestion::Remove),
            mk(
                teh,
                teh + 3,
                Suggestion::ReplaceWith("the".chars().collect()),
            ),
        ];
        lints.sort_by_key(|l| std::cmp::Reverse((l.span.start, l.span.end)));
        let (out, applied) = apply_guarded(&chars, &lints);
        let out: String = out.iter().collect();
        assert_eq!(applied, 1);
        assert_eq!(out, "Visit https://example.com/page ., then the end.");
    }

    #[test]
    fn grammar_subsequence_invariant() {
        let v = |xs: &[&str]| xs.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        assert!(is_subsequence(&v(&["a", "c"]), &v(&["a", "b", "c"])));
        assert!(is_subsequence(&v(&["a"]), &v(&["OK", "a"])));
        assert!(!is_subsequence(&v(&["a", "c"]), &v(&["c", "a"])));
        assert!(!is_subsequence(&v(&["a."]), &v(&["a"])));
    }

    #[test]
    fn grammar_ok_to_upper_does_not_roll_back_pass() {
        // harper proposes ok -> OK (ALLCAPS, now protected); that must not discard `teh` -> `the`.
        let out = fix("Then ok the plan. Also teh end.");
        assert!(out.contains("the end"), "{out}");
    }

    #[test]
    fn grammar_real_text_identifiers_are_kept() {
        let out = fix("please run sudo nginx yaml kubectl docker postgres terraform on tmrw, lol, then fix teh grammer.");
        for kept in [
            "sudo nginx yaml kubectl docker postgres terraform",
            "tmrw",
            "lol",
        ] {
            assert!(out.contains(kept), "lost {kept:?} in: {out}");
        }
        assert!(out.contains("fix the grammar"), "{out}");
    }

    #[test]
    fn grammar_contraction_dont_gets_apostrophe() {
        let out = fix("I dont know.");
        assert!(out.contains("don't"), "{out}");
    }

    #[test]
    fn grammar_rule_c_long_typos_only() {
        let rep = |s: &str| Suggestion::ReplaceWith(s.chars().collect());
        let chars = |s: &str| s.chars().collect::<Vec<char>>();
        // 7+ letters, one edit, same first letter: fixed
        assert!(spelling_ok(&chars("grammer"), &rep("grammar")));
        assert!(spelling_ok(&chars("occured"), &rep("occurred")));
        // 6 letters one edit from a real word: untouched (docker -> docket)
        assert!(!spelling_ok(&chars("docker"), &rep("docket")));
        // slang / identifiers: untouched
        assert!(!spelling_ok(&chars("tmrw"), &rep("tomorrow")));
        assert!(!spelling_ok(&chars("yaml"), &rep("yam")));
        // suggestion containing a space is never applied
        assert!(!spelling_ok(&chars("grammer"), &rep("gram mar")));
    }

    #[test]
    fn grammar_capitalization_mid_sentence_is_limited() {
        let out = fix("please run yaml and docker on postgres today.");
        assert!(
            out.contains("run yaml and docker on postgres today"),
            "{out}"
        );
        let out = fix("then i think i'm right.");
        assert!(out.contains("then I think I'm right."), "{out}");
        let out = fix("done. this is fine.\n\nnext line here.");
        assert!(out.contains("Done. This is fine."), "{out}");
        let out = fix("apples, this is fine.");
        assert!(out.contains("apples, this is fine."), "{out}");
    }

    #[test]
    fn grammar_capitalization_helper_rules() {
        let cap = |s: &str, word: &str| {
            let chars: Vec<char> = s.chars().collect();
            let start = s.find(word).unwrap();
            let (st, en) = (start, start + word.chars().count());
            let v: Vec<char> = {
                let mut c = word.chars();
                let first = c.next().unwrap().to_uppercase().collect::<String>();
                (first + c.as_str()).chars().collect()
            };
            capitalization_ok(&chars, st, en, &v)
        };
        assert!(!cap("apples, this is", "this"), "after comma");
        assert!(cap("x. this is", "this"), "after period");
        assert!(cap("x\n\nthis is", "this"), "after blank line");
        assert!(cap("this is", "this"), "start of text");
        assert!(!cap("the yaml file", "yaml"), "mid-sentence yaml");
        assert!(cap("then i think", "i"), "i pronoun");
        assert!(cap("then i'm sure", "i'm"), "i contraction");
    }

    #[test]
    fn grammar_negatives_are_not_rewritten() {
        let out = fix("I wont go. We werent there. They arent ready. He cant stay.");
        for bad in ["want", "rent", "a rent", "can stay", "we are"] {
            assert!(!out.to_lowercase().contains(bad), "{bad:?} in: {out}");
        }
    }

    #[test]
    fn grammar_abbreviations_are_kept() {
        let out = fix("imo this is fine, idk, tbh, brb, ttyl.");
        for kept in ["imo", "idk", "tbh", "brb", "ttyl"] {
            assert!(out.to_lowercase().contains(kept), "lost {kept:?} in: {out}");
        }
    }

    #[test]
    fn grammar_word_split_guard() {
        let c = |s: &str| s.chars().collect::<Vec<char>>();
        assert!(no_word_split(&c("teh"), &c("the")));
        assert!(!no_word_split(&c("asap"), &c("as soon as possible")));
        assert!(!no_word_split(&c("werent"), &c("were nt")));
        // non-word spans may contain spaces (spacing rules)
        assert!(no_word_split(&c("  "), &c(" ")));
        assert!(no_word_split(&c(""), &c(" ")));
    }

    #[test]
    fn grammar_negation_guard() {
        let c = |s: &str| s.chars().collect::<Vec<char>>();
        assert!(!negation_guard(&c("wont"), &c("want")));
        assert!(negation_guard(&c("wont"), &c("won't")));
        assert!(!negation_guard(&c("cant"), &c("can")));
        assert!(negation_guard(&c("dont"), &c("don't")));
        assert!(!negation_guard(&c("werent"), &c("were")));
        // dropping a negation word from a span is refused
        assert!(!negation_guard(&c("not"), &c("")));
        assert!(!negation_guard(&c("don't"), &c("do")));
        assert!(negation_guard(&c("don't"), &c("do not")));
        assert!(negation_guard(&c("hello"), &c("hi")));
    }

    #[test]
    fn grammar_mechanical_recall() {
        let cases = [
            ("this is an mule.", "a mule"),
            ("a apple a day.", "an apple"),
            ("i has two cats.", "I have two cats"),
            ("we have two  apples here.", "two apples here"),
            ("apples , and pears.", "apples, and pears"),
            ("it is the the best.", "is the best"),
            ("I saw teh dog.", "the dog"),
            ("fix this grammer.", "grammar"),
            ("good . i think so.", "good. I think so"),
        ];
        for (input, expected) in cases {
            let out = fix(input);
            assert!(
                out.contains(expected),
                "{input:?} -> {out:?}, wanted {expected:?}"
            );
        }
    }

    #[test]
    fn grammar_dialect_from_str() {
        assert_eq!(Dialect::from_str("British"), Ok(Dialect::British));
        assert_eq!(Dialect::from_str("british"), Ok(Dialect::British));
        assert_eq!(Dialect::from_str("AUSTRALIAN"), Ok(Dialect::Australian));
        assert!(Dialect::from_str("nope").is_err());
        assert_eq!(Dialect::default(), Dialect::American);
    }

    // ---- loosened allow-list / frequency rerank / irregular table (lead-written spec, DESIGN Rev 4) ----

    fn check_cases(cases: &[(&str, &str)]) {
        let mut bad = Vec::new();
        for (input, want) in cases {
            let got = fix(input);
            if got != *want {
                bad.push(format!("{input:?} -> {got:?} (want {want:?})"));
            }
        }
        assert!(bad.is_empty(), "\n{}", bad.join("\n"));
    }

    #[test]
    fn loosened_harper_rules_fix_common_confusions() {
        check_cases(&[
            ("This phone is cheaper then that one.", "This phone is cheaper than that one."),
            ("We could of won the game.", "We could have won the game."),
            ("Its a great idea.", "It's a great idea."),
            ("The company changed it's name.", "The company changed its name."),
            ("Your welcome to stay for dinner.", "You're welcome to stay for dinner."),
            ("Put the boxes over their.", "Put the boxes over there."),
            ("Their going to the park.", "They're going to the park."),
            ("The affect of caffeine is real.", "The effect of caffeine is real."),
            ("I don't want to loose my keys.", "I don't want to lose my keys."),
            ("We are going too the cinema.", "We are going to the cinema."),
            ("She has went to the market.", "She has gone to the market."),
            ("He cant come today.", "He can't come today."),
        ]);
    }

    #[test]
    fn frequency_ranked_spelling_picks_the_common_word() {
        check_cases(&[
            ("We have an apointment tomorrow.", "We have an appointment tomorrow."),
            ("I will arive soon.", "I will arrive soon."),
            ("Tell me wich one you want.", "Tell me which one you want."),
            ("What is your adress?", "What is your address?"),
        ]);
    }

    #[test]
    fn irregular_table_is_context_aware() {
        check_cases(&[
            ("She buyed a car.", "She bought a car."),
            ("He teached me chess.", "He taught me chess."),
            ("They goed home.", "They went home."),
            ("I eated lunch.", "I ate lunch."),
            ("I have eated lunch.", "I have eaten lunch."),
            ("She has teached us.", "She has taught us."),
            ("I've eated lunch.", "I've eaten lunch."),
            ("She's buyed a car.", "She's bought a car."),
            ("We writed a report.", "We wrote a report."),
            ("They had writed it already.", "They had written it already."),
        ]);
    }

    #[test]
    fn ambiguous_contracted_auxiliary_never_defaults_to_past() {
        // unchanged is acceptable; a wrong past form is not
        assert_ne!(fix("She's eated lunch."), "She's ate lunch.");
        assert_ne!(fix("I'd eated lunch."), "I'd ate lunch.");
    }

    #[test]
    fn loosened_rules_leave_correct_text_alone() {
        let clean = [
            "The drug will affect your sleep.",
            "Their dog is near the lake.",
            "The cat licked its paw.",
            "It's been a busy month.",
            "You're welcome to stay.",
            "I will lose my keys.",
            "We went to the market too.",
            "The knight rode a horse.",
            "Run `sudo apt install nginx` now.",
            "Use docker and kubectl for this.",
            "She sang a song.",
            "He taught math.",
            "We should have gone earlier.",
            "Your welcome email was lovely.",
            "Some advise caution.",
            "May the good advise the king.",
            "I have an appointment today.",
            "Check https://example.com/its-a-trap for details.",
            "The fire singed his hair.",
            "Tom ran home after the game.",
        ];
        let cases: Vec<(&str, &str)> = clean.iter().map(|c| (*c, *c)).collect();
        check_cases(&cases);
    }

    #[test]
    fn custom_lint_wins_over_overlapping_harper_lint() {
        let mk = |s: usize, e: usize, kind: LintKind, rep: &str| Lint {
            span: Span::new(s, e),
            lint_kind: kind,
            suggestions: vec![Suggestion::ReplaceWith(rep.chars().collect())],
            message: String::new(),
            priority: 0,
        };
        let harper = vec![
            mk(2, 9, LintKind::Spelling, "teacher"),   // overlaps the custom lint -> dropped
            mk(20, 24, LintKind::Grammar, "XXXX"),     // disjoint -> survives
        ];
        let custom = vec![mk(2, 9, LintKind::Grammar, "taught")];
        let merged = merge_custom_over_harper(harper, custom);
        assert_eq!(merged.len(), 2, "{merged:?}");
        assert!(merged.iter().any(|l| l.span == Span::new(2, 9)
            && l.suggestions == vec![Suggestion::ReplaceWith("taught".chars().collect())]));
        assert!(merged.iter().any(|l| l.span == Span::new(20, 24)));
        assert!(!merged.iter().any(|l| l.suggestions
            == vec![Suggestion::ReplaceWith("teacher".chars().collect())]));
    }
}
