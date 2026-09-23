//! Pure identifier and vocabulary check for Rolf's plane (SPEC-ADE D17).
//!
//! Inputs: the text, a glossary value (registry names, terms, birth sentences,
//! `max_sentence_words`), and the shipped lists. No hooks, no file writes, no
//! glossary persistence.
//!
//! Normalization:
//! - membership is lowercase ASCII
//! - surrounding punctuation is stripped from tokens
//! - a contraction keeps one internal apostrophe
//! - sentences split on `.?!` and newlines
//! - numbers and `N:NN` / `Ns` / `Nms` / `Nm` / `Nh` / `Nd` times pass R4
//! - the name `Rolf` passes R4

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use crate::contracts::HumanMessage;

const WORDS_TXT: &str = include_str!("../plain/words.txt");
const VOCAB_TXT: &str = include_str!("../plain/vocabulary.txt");

static WORDS: LazyLock<BTreeSet<String>> = LazyLock::new(|| parse_list(WORDS_TXT));
static VOCAB: LazyLock<BTreeSet<String>> = LazyLock::new(|| parse_list(VOCAB_TXT));
static VERBS: LazyLock<BTreeSet<&'static str>> =
    LazyLock::new(|| VERB_LIST.iter().copied().collect());

// Names Rolf already uses, without requiring a definition in every message.
const FAMILIAR_NAMES: &[&str] = &[
    "adeherdr",
    "am",
    "april",
    "august",
    "chatgpt",
    "claude",
    "codex",
    "cpu",
    "cvs",
    "december",
    "elicio",
    "february",
    "flyonenomics",
    "friday",
    "gemini",
    "github",
    "gpu",
    "january",
    "jev",
    "july",
    "june",
    "mac",
    "march",
    "may",
    "monday",
    "november",
    "october",
    "oracle",
    "pm",
    "prl",
    "saturday",
    "september",
    "somebody",
    "sunday",
    "thursday",
    "tuesday",
    "venator",
    "wednesday",
];

fn parse_list(text: &str) -> BTreeSet<String> {
    text.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// Shipped everyday list `plain/words.txt`.
fn shipped_words() -> &'static BTreeSet<String> {
    &WORDS
}

/// Plugin nouns `plain/vocabulary.txt` (SPEC-ADE D17 R4).
fn plugin_vocabulary() -> &'static BTreeSet<String> {
    &VOCAB
}

/// Registry names, glossary terms and the sentence-length cap (SPEC-ADE D17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Glossary {
    pub(crate) names: BTreeMap<String, String>,
    pub(crate) terms: BTreeMap<String, String>,
    /// Names that need no birth sentence (including this project's own name).
    pub(crate) familiar_names: BTreeSet<String>,
    pub(crate) max_sentence_words: usize,
}

impl Default for Glossary {
    fn default() -> Self {
        Self {
            names: BTreeMap::new(),
            terms: BTreeMap::new(),
            familiar_names: BTreeSet::new(),
            max_sentence_words: 25,
        }
    }
}

impl Glossary {
    pub(crate) fn max_words(&self) -> usize {
        if self.max_sentence_words == 0 {
            25
        } else {
            self.max_sentence_words
        }
    }

    fn is_registry(&self, token: &str) -> bool {
        self.names.contains_key(token)
    }

    pub(crate) fn is_plain_name(&self, token: &str) -> bool {
        self.is_familiar(token) || shipped_words().contains(&token.to_ascii_lowercase())
    }

    fn is_familiar(&self, token: &str) -> bool {
        let lower = token.to_ascii_lowercase();
        FAMILIAR_NAMES.contains(&lower.as_str()) || self.familiar_names.contains(&lower)
    }

    fn is_known_name(&self, token: &str) -> bool {
        self.is_familiar(token)
            || self
                .names
                .keys()
                .chain(self.terms.keys())
                .any(|name| !name.is_empty() && name.eq_ignore_ascii_case(token))
    }
}

/// D17 rule ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Rule {
    BareName,
    InventedDefinition,
    Identifier,
    UnknownWord,
    LongSentence,
    QuestionForm,
    Envelope,
}

impl Rule {
    pub(crate) fn code(self) -> &'static str {
        match self {
            Self::BareName => "plain_bare_name",
            Self::InventedDefinition => "plain_invented_definition",
            Self::Identifier => "plain_identifier",
            Self::UnknownWord => "plain_unknown_word",
            Self::LongSentence => "plain_long_sentence",
            Self::QuestionForm => "plain_question_form",
            Self::Envelope => "plain_envelope",
        }
    }
}

/// Byte span in the checked text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) start: usize,
    pub(crate) end: usize,
}

/// One failing span, its rule, and the fixed fix text (SPEC-ADE D17).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Violation {
    pub(crate) rule: Rule,
    pub(crate) span: Span,
    pub(crate) fix: String,
}

/// Pass when `violations` is empty.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct CheckResult {
    pub(crate) violations: Vec<Violation>,
}

impl CheckResult {
    pub(crate) fn passed(&self) -> bool {
        self.violations.is_empty()
    }
}

/// Check free text against R1 to R5. R6 and R7 apply only to structured fields.
pub(crate) fn check(text: &str, glossary: &Glossary) -> CheckResult {
    CheckResult {
        violations: check_text(text, glossary),
    }
}

/// The non-empty sentences in a record. A `.` between two letters or digits
/// (`config.toml`, `1.2`) does not end a sentence; other terminators and
/// newlines do. [`check_r5`] keeps its own split because Rolf's prose has no
/// file names.
pub(crate) fn record_sentences(text: &str) -> Vec<&str> {
    let bytes = text.as_bytes();
    let mut sentences = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < bytes.len() {
        let ends = match bytes[i] {
            b'\n' | b'!' | b'?' => true,
            b'.' => {
                let inside_token = i
                    .checked_sub(1)
                    .and_then(|before| bytes.get(before))
                    .is_some_and(u8::is_ascii_alphanumeric)
                    && bytes.get(i + 1).is_some_and(u8::is_ascii_alphanumeric);
                !inside_token
            }
            _ => false,
        };
        if ends {
            let sentence = text[start..=i].trim();
            if !sentence.is_empty() {
                sentences.push(sentence);
            }
            start = i + 1;
        }
        i += 1;
    }
    let sentence = text[start..].trim();
    if !sentence.is_empty() {
        sentences.push(sentence);
    }
    sentences
}

pub(crate) fn sentence_count(text: &str) -> usize {
    record_sentences(text).len()
}

/// Check an `ha ask` question and its choices (R1 to R6).
pub(crate) fn check_ask(question: &str, choices: &[String], glossary: &Glossary) -> CheckResult {
    let mut violations = check_text(question, glossary);
    for choice in choices {
        violations.extend(check_text(choice, glossary));
    }
    violations.extend(
        check_question_form(question, choices, glossary)
            .into_iter()
            .map(|(_, violation)| violation),
    );
    CheckResult { violations }
}

/// Check a typed envelope (R7) and any prose fields it carries (R1 to R5).
pub(crate) fn check_message(message: &HumanMessage, glossary: &Glossary) -> CheckResult {
    let mut violations = Vec::new();
    match message {
        HumanMessage::Say { what, means, .. } => {
            if what.trim().is_empty() {
                violations.push(envelope_violation("what"));
            } else {
                violations.extend(check_text(what, glossary));
            }
            if let Some(means) = means {
                if means.trim().is_empty() {
                    violations.push(envelope_violation("means"));
                } else {
                    violations.extend(check_text(means, glossary));
                }
            }
        }
        HumanMessage::Ask { id, revision } => {
            if id.trim().is_empty() {
                violations.push(envelope_violation("id"));
            }
            if *revision == 0 {
                violations.push(envelope_violation("revision"));
            }
        }
        HumanMessage::Notice { id } => {
            if id.trim().is_empty() {
                violations.push(envelope_violation("id"));
            }
        }
    }
    CheckResult { violations }
}

fn envelope_violation(field: &str) -> Violation {
    Violation {
        rule: Rule::Envelope,
        span: Span { start: 0, end: 0 },
        fix: format!("required envelope field {field} is missing or empty"),
    }
}

fn check_text(text: &str, glossary: &Glossary) -> Vec<Violation> {
    let mut violations = Vec::new();
    violations.extend(check_r1_r2(text, glossary));
    violations.extend(check_r3(text, glossary));
    violations.extend(check_r4(text, glossary));
    violations.extend(check_r5(text, glossary));
    violations
}

fn check_r1_r2(text: &str, glossary: &Glossary) -> Vec<Violation> {
    let mut violations = Vec::new();
    let mut names: Vec<&String> = glossary.names.keys().collect();
    names.sort_by_key(|n| std::cmp::Reverse(n.len()));
    for name in names {
        let sentence = glossary.names.get(name).map(String::as_str).unwrap_or("");
        // A word such as "main" can also be an internal registry name. In
        // ordinary prose it remains an ordinary word, not a forced gloss.
        if glossary.is_plain_name(name) {
            continue;
        }
        for (start, end) in name_spans(text, name) {
            let invented = invented_definition(text, start, end, sentence);
            if invented {
                violations.push(Violation {
                    rule: Rule::InventedDefinition,
                    span: Span { start, end },
                    fix: "definitions must use the recorded description for this name".into(),
                });
                continue;
            }
            if !is_gloss_form(text, start, end, sentence) {
                violations.push(Violation {
                    rule: Rule::BareName,
                    span: Span { start, end },
                    fix: "first use of a technical name needs its recorded description".into(),
                });
            }
        }
    }
    let mut terms: Vec<&String> = glossary.terms.keys().collect();
    terms.sort_by_key(|n| std::cmp::Reverse(n.len()));
    for name in terms {
        if glossary.names.contains_key(name) {
            continue;
        }
        let sentence = glossary.terms.get(name).map(String::as_str).unwrap_or("");
        if sentence.is_empty() || glossary.is_familiar(name) {
            continue;
        }
        for (start, end) in name_spans(text, name) {
            if invented_definition(text, start, end, sentence) {
                violations.push(Violation {
                    rule: Rule::InventedDefinition,
                    span: Span { start, end },
                    fix: "definitions must use the recorded description for this name".into(),
                });
            }
        }
    }
    violations
}

/// Every place `name` stands as a whole token, in any letter case: a
/// capital at the start of a sentence is the same name (A0 review M3).
fn name_spans(text: &str, name: &str) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    if name.is_empty() {
        return spans;
    }
    // ASCII lowercasing keeps every byte offset.
    let lower = text.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let name = name.as_str();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(name) {
        let start = from + rel;
        let end = start + name.len();
        if is_token_boundary(text, start, end) {
            spans.push((start, end));
        }
        from = start + name.len();
    }
    spans
}

fn is_token_boundary(text: &str, start: usize, end: usize) -> bool {
    let before = start
        .checked_sub(1)
        .and_then(|i| text.as_bytes().get(i).copied());
    let after = text.as_bytes().get(end).copied();
    !matches!(before, Some(b) if is_name_char(b)) && !matches!(after, Some(b) if is_name_char(b))
}

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

fn is_gloss_form(text: &str, start: usize, end: usize, sentence: &str) -> bool {
    let bytes = text.as_bytes();
    if start == 0 || end >= bytes.len() || bytes[start - 1] != b'(' || bytes[end] != b')' {
        return false;
    }
    let before = text[..start.saturating_sub(1)].trim_end();
    let expected = sentence.trim().trim_end_matches('.');
    let got = before.trim_end_matches('.');
    !expected.is_empty() && (got.ends_with(expected) || got.eq_ignore_ascii_case(expected))
}

fn invented_definition(text: &str, _start: usize, end: usize, sentence: &str) -> bool {
    let rest = text[end..].trim_start();
    if rest.to_ascii_lowercase().starts_with("is ") || rest.to_ascii_lowercase().starts_with("is\n")
    {
        return true;
    }
    if rest.to_ascii_lowercase().starts_with("means ")
        || rest.to_ascii_lowercase().starts_with("means\n")
    {
        return true;
    }
    if rest.starts_with(':') {
        return true;
    }
    if let Some(inner) = rest.strip_prefix('(').and_then(|r| r.split(')').next()) {
        let inner = inner.trim().trim_end_matches('.');
        let expected = sentence.trim().trim_end_matches('.');
        return inner != expected;
    }
    false
}

fn check_r3(text: &str, glossary: &Glossary) -> Vec<Violation> {
    let mut violations = Vec::new();
    for token in tokens(text) {
        let glossed_code = glossary.names.iter().any(|(name, sentence)| {
            name.eq_ignore_ascii_case(token.raw)
                && is_gloss_form(text, token.start, token.end, sentence)
        });
        if (is_code(token.raw) && !glossed_code && !glossary.is_familiar(token.raw))
            || (!glossed_code
                && !glossary.is_known_name(token.raw)
                && is_identifier_shaped(token.raw))
        {
            violations.push(Violation {
                rule: Rule::Identifier,
                span: Span {
                    start: token.start,
                    end: token.end,
                },
                fix: "codes, flags and paths are not allowed; replace this with words".into(),
            });
        }
    }
    violations
}

fn check_r4(text: &str, glossary: &Glossary) -> Vec<Violation> {
    let extra = glossary_words(glossary);
    let is_admitted = |word: &str| {
        word == "rolf"
            || shipped_words().contains(word)
            || plugin_vocabulary().contains(word)
            || extra.contains(word)
            || glossary.is_familiar(word)
    };
    let mut violations = Vec::new();
    for token in tokens(text) {
        if glossary.is_known_name(token.raw) || is_identifier_shaped(token.raw) {
            continue;
        }
        if is_number(token.raw) || is_time(token.raw) {
            continue;
        }
        let lower = token.raw.replace('\u{2019}', "'").to_ascii_lowercase();
        // A possessive is its word: "the coordinator's pane" (SPEC-ADE item 73).
        let base = lower.strip_suffix("'s").unwrap_or(&lower);
        let raw_base = token.raw.strip_suffix("'s").unwrap_or(token.raw);
        if is_admitted(base) || glossary.is_known_name(raw_base) {
            continue;
        }
        // Numbers with units and ordinals (`10s`, `1st`, `r2`) pass; R3 owns
        // identifier shapes.
        if lower.chars().any(|c| c.is_ascii_digit()) {
            continue;
        }
        // Each part of a joined word is a word: a hyphen or dash hides
        // nothing (A0 review H1). A letter outside ASCII is never admitted.
        let unknown = base
            .split(['-', '\u{2014}', '\u{2013}'])
            .filter(|part| part.chars().any(char::is_alphabetic))
            .any(|part| {
                let part = part.strip_suffix("'s").unwrap_or(part);
                !is_admitted(part) && !glossary.is_known_name(part)
            });
        if unknown {
            violations.push(Violation {
                rule: Rule::UnknownWord,
                span: Span {
                    start: token.start,
                    end: token.end,
                },
                fix: "only everyday words and familiar names are allowed; add a name with `ha term add --name`".into(),
            });
        }
    }
    violations
}

fn check_r5(text: &str, glossary: &Glossary) -> Vec<Violation> {
    let cap = glossary.max_words();
    let mut violations = Vec::new();
    for (start, end) in sentences(text) {
        let slice = &text[start..end];
        let n = tokens(slice)
            .into_iter()
            .filter(|t| t.raw.chars().any(|c| c.is_ascii_alphanumeric()))
            .count();
        if n > cap {
            violations.push(Violation {
                rule: Rule::LongSentence,
                span: Span { start, end },
                fix: format!("split this {n}-word sentence; the limit is {cap} words"),
            });
        }
    }
    violations
}

pub(crate) fn check_question_form<'a>(
    question: &'a str,
    choices: &'a [String],
    glossary: &Glossary,
) -> Vec<(&'a str, Violation)> {
    let mut violations = Vec::new();
    let trimmed = question.trim();
    if !trimmed.ends_with('?') {
        violations.push((
            question,
            Violation {
                rule: Rule::QuestionForm,
                span: Span {
                    start: 0,
                    end: question.len(),
                },
                fix: "end the question with a question mark".into(),
            },
        ));
    }
    for choice in choices {
        let words: Vec<_> = tokens(choice)
            .into_iter()
            .filter(|t| t.raw.chars().any(|c| c.is_ascii_alphabetic()))
            .collect();
        let span = Span {
            start: 0,
            end: choice.len(),
        };
        if glossary.is_registry(choice.trim()) {
            violations.push((
                choice.as_str(),
                Violation {
                    rule: Rule::QuestionForm,
                    span,
                    fix: "a choice cannot be a registry name".into(),
                },
            ));
            continue;
        }
        if words.len() <= 1 {
            violations.push((
                choice.as_str(),
                Violation {
                    rule: Rule::QuestionForm,
                    span,
                    fix: "a choice cannot be a single word".into(),
                },
            ));
            continue;
        }
        let has_verb = words
            .iter()
            .any(|t| VERBS.contains(t.raw.to_ascii_lowercase().as_str()));
        if !has_verb {
            violations.push((
                choice.as_str(),
                Violation {
                    rule: Rule::QuestionForm,
                    span,
                    fix: "each choice must be a sentence with a verb".into(),
                },
            ));
        }
    }
    violations
}

/// Words from the glossary's recorded sentences and names, lowercased. The
/// shipped and vocabulary lists are consulted directly, so the big list is
/// never cloned (SPEC-ADE D17 R4).
fn glossary_words(glossary: &Glossary) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    for sentence in glossary.names.values().chain(glossary.terms.values()) {
        for token in tokens(sentence) {
            set.insert(token.raw.to_ascii_lowercase());
        }
    }
    for name in glossary.names.keys().chain(glossary.terms.keys()) {
        set.insert(name.to_ascii_lowercase());
    }
    set
}

#[derive(Clone, Copy)]
struct Token<'a> {
    raw: &'a str,
    start: usize,
    end: usize,
}

fn tokens(text: &str) -> Vec<Token<'_>> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if let Some(end) = take_path(text, i) {
            out.push(Token {
                raw: &text[i..end],
                start: i,
                end,
            });
            i = end;
            continue;
        }
        let start = i;
        i += 1;
        while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        let (core_start, core_end) = trim_punct(text, start, i);
        if core_start < core_end
            && (text[start..i].starts_with("--") || text[start..i].starts_with("q-…"))
        {
            // Keep flag dashes and the ellipsis in a shortened request ID.
            let end = if text[start..i].starts_with("q-…") {
                i
            } else {
                core_end
            };
            out.push(Token {
                raw: &text[start..end],
                start,
                end,
            });
            continue;
        }
        if core_start < core_end {
            out.push(Token {
                raw: &text[core_start..core_end],
                start: core_start,
                end: core_end,
            });
        }
    }
    out
}

fn take_path(text: &str, start: usize) -> Option<usize> {
    let rest = &text[start..];
    let looks_like_path = rest.starts_with('/')
        || rest.starts_with("~/")
        || rest.starts_with("./")
        || rest.starts_with("../")
        || rest[..rest
            .find(|c: char| c.is_ascii_whitespace())
            .unwrap_or(rest.len())]
            .contains('/');
    if !looks_like_path {
        return None;
    }
    let chunk = rest.split_whitespace().next()?;
    if !chunk.contains('/') {
        return None;
    }
    Some(start + chunk.len())
}

/// Strips everything that is not a letter or digit from both ends, so a
/// backtick, star or curly quote cannot hide a word (A0 review H2).
fn trim_punct(text: &str, start: usize, end: usize) -> (usize, usize) {
    let s = &text[start..end];
    let lead = s.len() - s.trim_start_matches(|c: char| !c.is_alphanumeric()).len();
    let trail = s.trim_end_matches(|c: char| !c.is_alphanumeric()).len();
    if trail <= lead {
        return (start, start);
    }
    (start + lead, start + trail)
}

fn is_code(token: &str) -> bool {
    token.starts_with("--")
        || token.starts_with("q-")
        || token
            .strip_prefix('r')
            .is_some_and(|digits| !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()))
        || is_kebab_with_digit(token)
        || is_hex_run(token)
}

pub(crate) fn is_identifier_shaped(token: &str) -> bool {
    is_code(token)
        || is_path_token(token)
        || is_snake_case(token)
        || is_camel_case(token)
        || is_all_caps(token)
}

fn is_path_token(token: &str) -> bool {
    token.contains('/') || token.starts_with("~/") || token.starts_with("./")
}

fn is_snake_case(token: &str) -> bool {
    token.contains('_')
        && token.starts_with(|c: char| c.is_ascii_lowercase())
        && token
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}

pub(crate) fn is_camel_case(token: &str) -> bool {
    let mut chars = token.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    first.is_ascii_uppercase()
        && token.chars().all(|c| c.is_ascii_alphabetic())
        && token.chars().any(|c| c.is_ascii_lowercase())
        && token.chars().filter(|c| c.is_ascii_uppercase()).count() >= 2
}

fn is_all_caps(token: &str) -> bool {
    token.len() >= 2 && token.chars().all(|c| c.is_ascii_uppercase())
}

fn is_kebab_with_digit(token: &str) -> bool {
    token.contains('-')
        && token.chars().any(|c| c.is_ascii_digit())
        && token
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn is_hex_run(token: &str) -> bool {
    token.len() >= 7
        && token.chars().all(|c| c.is_ascii_hexdigit())
        && token.chars().any(|c| c.is_ascii_digit())
        && !token.chars().all(|c| c.is_ascii_digit())
}

fn is_number(token: &str) -> bool {
    let mut parts = token.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    first.chars().all(|c| c.is_ascii_digit())
        && !first.is_empty()
        && parts.all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
}

fn is_time(token: &str) -> bool {
    if let Some((h, m)) = token.split_once(':') {
        return !h.is_empty()
            && h.chars().all(|c| c.is_ascii_digit())
            && m.chars().all(|c| c.is_ascii_digit() || c == ':');
    }
    let units = ["ms", "s", "m", "h", "d"];
    units.iter().any(|u| {
        token.ends_with(u)
            && token[..token.len() - u.len()]
                .chars()
                .all(|c| c.is_ascii_digit())
            && token.len() > u.len()
    })
}

fn sentences(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    let bytes = text.as_bytes();
    for (i, &b) in bytes.iter().enumerate() {
        if matches!(b, b'.' | b'?' | b'!' | b'\n') {
            if i > start {
                out.push((start, i));
            }
            start = i + 1;
            while start < bytes.len() && bytes[start].is_ascii_whitespace() {
                start += 1;
            }
        }
    }
    if start < text.len() {
        out.push((start, text.len()));
    }
    out
}

const VERB_LIST: &[&str] = &[
    "add", "allow", "answer", "ask", "be", "bind", "build", "choose", "close", "come", "comes",
    "continue", "count", "cut", "decide", "delete", "did", "do", "does", "end", "fail", "follow",
    "follows", "get", "give", "go", "goes", "has", "have", "help", "hold", "is", "keep", "land",
    "lands", "leave", "let", "look", "make", "mean", "means", "merge", "move", "need", "open",
    "pass", "print", "put", "read", "redesign", "refuse", "remove", "replace", "run", "running",
    "said", "say", "see", "send", "set", "show", "split", "start", "stay", "stays", "stop",
    "switch", "take", "tell", "try", "type", "use", "wait", "want", "was", "were", "work", "write",
    "wrote",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn glossary_lineage() -> Glossary {
        let mut g = Glossary::default();
        g.names.insert(
            "lineage-persist".into(),
            "Keeps the worker list after an update.".into(),
        );
        g
    }

    fn glossary_acronym() -> Glossary {
        let mut g = Glossary::default();
        g.names
            .insert("F-cap".into(), "The failing choice form.".into());
        g
    }

    /// A0 review H1, H2, M3, M4, L11: shapes that slipped past the check.
    #[test]
    fn joined_wrapped_and_capitalised_words_do_not_slip_through() {
        let g = Glossary::default();
        for text in [
            "The bisimulation-quotient is done.",
            "The lane\u{2014}bisimulation is done.",
            "The caf\u{e9} is done.",
            "The coordinator\u{2019}s bisimulation.",
            "The *bisimulation* is done.",
            "The \u{201c}bisimulation\u{201d} is done.",
        ] {
            assert_eq!(codes(&check(text, &g)), ["plain_unknown_word"], "{text}");
        }
        assert_eq!(
            codes(&check("Run `snake_case_token` now.", &g)),
            ["plain_identifier"]
        );
        let g = glossary_acronym();
        assert_eq!(codes(&check("F-CAP failed.", &g)), ["plain_bare_name"]);
        let mut empty = Glossary::default();
        empty.names.insert(String::new(), String::new());
        check("\u{e9} a b", &empty);
        assert!(check("The lane is done.", &empty).passed());
        assert!(check("The file has 1234567 lines.", &Glossary::default()).passed());
    }

    /// SPEC-ADE item 73: a possessive is checked as its word.
    #[test]
    fn a_possessive_is_checked_as_its_word() {
        let g = glossary_acronym();
        assert!(check("The coordinator's pane is open.", &g).passed());
        assert!(check("Rolf's lane is done.", &g).passed());
        assert_eq!(
            codes(&check("The zorbl's pane is open.", &g)),
            ["plain_unknown_word"]
        );
    }

    #[test]
    fn w79_plain_words_names_and_limits() {
        let mut g = Glossary::default();
        // A registry collision must not turn an English word into a bare name.
        g.names.insert(
            "main".into(),
            "Rebuilds the compartments of another task.".into(),
        );
        g.familiar_names.insert("custombrand".into());
        g.familiar_names.insert("prl-8-53".into());
        for text in [
            "the main project folder",
            "7 AM",
            "Thursday",
            "September",
            "i wa",
            "I delete the old screens",
            "I remove the location filter",
            "I switch Jev on",
            "I count the rows",
            "I move the lane",
            "I redesign the dashboard",
            "I cut the old step",
            "Claude",
            "GitHub",
            "ChatGPT",
            "Codex",
            "Gemini",
            "Mac",
            "Oracle",
            "CVS's",
            "flyonenomics",
            "venator",
            "elicio",
            "somebody",
            "adeherdr",
            "prl",
            "prl-8-53",
            "custombrand",
            "rented-GPU",
            "CPU-only",
            "long-running",
        ] {
            let result = check(text, &g);
            assert!(result.passed(), "{text}: {:?}", result.violations);
            if text.starts_with("I ") {
                let choices = [text.to_string()];
                let form = check_question_form("Which one?", &choices, &g);
                assert!(form.is_empty(), "{text}: {form:?}");
            }
        }
        let long = (0..26).map(|_| "the").collect::<Vec<_>>().join(" ");
        let refusals = [
            ("t-0369", Rule::Identifier, "t-0369"),
            ("job-0042", Rule::Identifier, "job-0042"),
            ("r138", Rule::Identifier, "r138"),
            ("f-0292", Rule::Identifier, "f-0292"),
            ("q-1234", Rule::Identifier, "q-1234"),
            ("q-…", Rule::Identifier, "q-…"),
            ("a1b2c3d4", Rule::Identifier, "a1b2c3d4"),
            ("snake_case", Rule::Identifier, "snake_case"),
            ("--flags", Rule::Identifier, "--flags"),
            (
                "~/projects/somebody",
                Rule::Identifier,
                "~/projects/somebody",
            ),
            (
                "/Users/rolfie/projects",
                Rule::Identifier,
                "/Users/rolfie/projects",
            ),
            (long.as_str(), Rule::LongSentence, long.as_str()),
        ];
        for (text, rule, phrase) in refusals {
            let result = check(text, &g);
            let violation = result
                .violations
                .iter()
                .find(|v| v.rule == rule)
                .unwrap_or_else(|| panic!("{text}: {:?}", result.violations));
            assert_eq!(&text[violation.span.start..violation.span.end], phrase);
            let refusal = crate::glossary::format_check(text, &result);
            assert!(refusal.contains(&format!("\"{phrase}\"")), "{refusal}");
            assert!(!refusal.contains("Rebuilds"), "{refusal}");
            if rule == Rule::LongSentence {
                assert!(violation.fix.contains("limit is 25 words"));
            } else {
                assert!(violation.fix.contains("not allowed"));
            }
            assert!(!violation.fix.contains("Rebuilds"));
        }
    }

    fn codes(result: &CheckResult) -> Vec<&'static str> {
        result.violations.iter().map(|v| v.rule.code()).collect()
    }

    fn list_is_clean(text: &str) {
        let mut lines: Vec<&str> = text.lines().filter(|l| !l.is_empty()).collect();
        assert!(lines.iter().all(|l| {
            let mut chars = l.chars();
            let first_ok = chars.next().is_some_and(|c| c.is_ascii_lowercase());
            let last_ok = l.chars().last().is_some_and(|c| c.is_ascii_lowercase());
            first_ok
                && last_ok
                && l.chars()
                    .all(|c| c.is_ascii_lowercase() || c == '-' || c == '\'')
        }));
        let unique = lines.len();
        lines.sort();
        lines.dedup();
        assert_eq!(lines.len(), unique);
        let mut sorted = lines.clone();
        sorted.sort();
        assert_eq!(lines, sorted);
    }

    #[test]
    fn word_lists_load_sorted_unique_lowercase_and_contain_vocabulary() {
        list_is_clean(WORDS_TXT);
        list_is_clean(VOCAB_TXT);
        // 2026-09-19: ordinary coordinator replies were rejected for these.
        for word in ["paused", "earpiece", "everyday", "inflections", "don't"] {
            assert!(shipped_words().contains(word), "{word}");
        }
        for word in plugin_vocabulary() {
            assert!(
                shipped_words().contains(word) || plugin_vocabulary().contains(word),
                "{word}"
            );
        }
        assert!(plugin_vocabulary().contains("lane"));
        assert!(plugin_vocabulary().contains("round"));
        assert!(plugin_vocabulary().contains("reviewer"));
        assert!(plugin_vocabulary().contains("checkpoint"));
    }

    #[test]
    fn r1_pass_gloss_form_and_fail_bare_name() {
        let g = glossary_lineage();
        let pass = check(
            "Keeps the worker list after an update. (lineage-persist) The rest of this note is plain.",
            &g,
        );
        assert!(pass.passed(), "{:?}", pass.violations);
        let fail = check("lineage-persist failed later.", &g);
        assert_eq!(codes(&fail), ["plain_bare_name"]);
        assert_eq!(
            fail.violations[0].fix,
            "first use of a technical name needs its recorded description"
        );
    }

    #[test]
    fn r2_pass_recorded_sentence_and_fail_invented_definition() {
        let g = glossary_lineage();
        let pass = check(
            "Keeps the worker list after an update. (lineage-persist)",
            &g,
        );
        assert!(pass.passed(), "{:?}", pass.violations);
        let fail = check("lineage-persist is a store for panes.", &g);
        assert!(codes(&fail).contains(&"plain_invented_definition"));
        assert_eq!(
            fail.violations[0].fix,
            "definitions must use the recorded description for this name"
        );
    }

    #[test]
    fn r3_pass_plain_words_and_fail_identifier() {
        let g = Glossary::default();
        let pass = check("The head follows the branch while the tree lands.", &g);
        assert!(
            !codes(&pass).contains(&"plain_identifier"),
            "{:?}",
            pass.violations
        );
        let fail = check("The snake_case_token appears here.", &g);
        assert!(codes(&fail).contains(&"plain_identifier"));
        assert_eq!(
            fail.violations
                .iter()
                .find(|v| v.rule == Rule::Identifier)
                .unwrap()
                .fix,
            "codes, flags and paths are not allowed; replace this with words"
        );
    }

    #[test]
    fn r4_pass_known_words_and_fail_unknown() {
        let g = Glossary::default();
        let pass = check("The head follows the branch while the tree lands.", &g);
        assert!(pass.passed(), "{:?}", pass.violations);
        let fail = check("The bisimulation quotient establishes confluence.", &g);
        assert!(codes(&fail).contains(&"plain_unknown_word"));
        assert!(fail.violations.iter().any(|v| v.rule == Rule::UnknownWord));
    }

    #[test]
    fn r5_pass_short_and_fail_long() {
        let g = Glossary::default();
        let pass = check("The head follows the branch while the tree lands.", &g);
        assert!(pass.passed(), "{:?}", pass.violations);
        let words = (0..26).map(|_| "the").collect::<Vec<_>>().join(" ");
        let fail = check(&format!("{words}."), &g);
        assert_eq!(codes(&fail), ["plain_long_sentence"]);
        assert_eq!(
            fail.violations[0].fix,
            "split this 26-word sentence; the limit is 25 words"
        );
    }

    #[test]
    fn r6_pass_question_and_fail_imperative() {
        let g = Glossary::default();
        let pass = check_ask(
            "keep the experiment running another hour, or stop now?",
            &["keep it running another hour".into(), "stop it now".into()],
            &g,
        );
        assert!(
            !codes(&pass).contains(&"plain_question_form"),
            "{:?}",
            pass.violations
        );
        let fail = check_ask(
            "choose the zorbulate or the normal form",
            &["choose the zorbulate or the normal form".into()],
            &g,
        );
        assert!(codes(&fail).contains(&"plain_unknown_word"));
        assert!(codes(&fail).contains(&"plain_question_form"));
    }

    #[test]
    fn r7_pass_complete_say_and_fail_empty_what() {
        let g = Glossary::default();
        let pass = check_message(
            &HumanMessage::Say {
                id: "s-1".into(),
                what: "The head follows the branch.".into(),
                means: None,
                landed_round: None,
            },
            &g,
        );
        assert!(pass.passed(), "{:?}", pass.violations);
        let fail = check_message(
            &HumanMessage::Say {
                id: "s-2".into(),
                what: "".into(),
                means: None,
                landed_round: None,
            },
            &g,
        );
        assert_eq!(codes(&fail), ["plain_envelope"]);
        assert!(fail.violations[0].fix.contains("what"));
    }

    #[test]
    fn adversarial_definition_then_bare_use_fails_r1() {
        let g = glossary_lineage();
        let result = check(
            "Keeps the worker list after an update. Then lineage-persist hides the rest.",
            &g,
        );
        assert!(
            codes(&result).contains(&"plain_bare_name"),
            "{:?}",
            result.violations
        );
    }

    #[test]
    fn adversarial_bisimulation_fails_r4() {
        let result = check(
            "The bisimulation quotient establishes confluence.",
            &Glossary::default(),
        );
        assert!(codes(&result).contains(&"plain_unknown_word"));
    }

    #[test]
    fn adversarial_registered_acronym_bare_fails_r1() {
        let g = glossary_acronym();
        let result = check("F-cap decides the row.", &g);
        assert!(
            codes(&result).contains(&"plain_bare_name"),
            "{:?}",
            result.violations
        );
    }

    #[test]
    fn adversarial_choose_unknown_word_fails_r4_and_r6() {
        let result = check_ask(
            "choose the zorbulate or the normal form",
            &[
                "choose the zorbulate or the normal form".into(),
                "you decide".into(),
            ],
            &Glossary::default(),
        );
        assert!(codes(&result).contains(&"plain_unknown_word"));
        assert!(codes(&result).contains(&"plain_question_form"));
    }

    #[test]
    fn gloss_form_passes_and_check_is_pure() {
        let g = glossary_lineage();
        let text = "Keeps the worker list after an update. (lineage-persist)";
        let first = check(text, &g);
        let second = check(text, &g);
        assert!(first.passed());
        assert_eq!(first, second);
        assert_eq!(g.names.len(), 1);
    }

    #[test]
    fn numbers_times_and_rolf_pass_r4() {
        let result = check("Rolf has 10s at 12:30.", &Glossary::default());
        assert!(result.passed(), "{:?}", result.violations);
    }

    #[test]
    fn ask_single_word_choice_and_registry_choice_fail_r6() {
        let g = glossary_lineage();
        let result = check_ask("which one?", &["yes".into(), "lineage-persist".into()], &g);
        assert!(
            codes(&result)
                .iter()
                .filter(|c| **c == "plain_question_form")
                .count()
                >= 2
        );
    }

    #[test]
    fn camel_case_hex_allcaps_path_fail_r3() {
        let g = Glossary::default();
        for text in [
            "HumanMessage shows up.",
            "a1b2c3d4 appears.",
            "DONE lands.",
            "src/plain.rs is a path.",
        ] {
            let result = check(text, &g);
            assert!(
                codes(&result).contains(&"plain_identifier"),
                "{text} {:?}",
                result.violations
            );
        }
    }
}
