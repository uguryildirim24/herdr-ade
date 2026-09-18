//! Jev: the launch-time picker's client (SPEC-jev-picker v2).
//!
//! Every call goes through `Runner` as `/usr/bin/curl --config -`; the key
//! never reaches argv or the child environment (SPEC-jev-picker v2 §5).
//! Tests use the FakeRunner, never the live service.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::contracts::Recipe;
use crate::paths::Env;
use crate::runner::{Cmd, Output, Runner};

/// The pinned model. A response whose `model` is not this is a fallback
/// (SPEC-jev-picker v2 §1 Pin).
pub const JEV_MODEL: &str = "jev-1.13.0";
pub const SYSTEMONE_URL: &str = "https://api.typesafe.ai/v1/systemone";
pub const MODELS_URL: &str = "https://api.typesafe.ai/v1/models";
pub const CURL: &str = "/usr/bin/curl";
/// The excerpt transform's version, recorded on every launch
/// (SPEC-jev-picker v2 §2 Output).
pub const EXCERPT_VERSION: u32 = 1;
/// 2,200 characters on a word boundary (SPEC-jev-picker v2 §2 Input).
pub const EXCERPT_LIMIT: usize = 2_200;
/// Total attempts: the first call and one retry (SPEC-jev-picker v2 §5).
pub const MAX_ATTEMPTS: u32 = 2;
/// curl exit codes that count as a connect error and may be retried once.
pub const CONNECT_EXIT_CODES: [i32; 3] = [6, 7, 35];
/// The key file under the home directory, after `TYPESAFE_API_KEY`.
pub const KEY_RELATIVE: &str = ".config/typesafe/api_key";

/// The fixed scrub list (SPEC-jev-picker v2 §2 Input), plus effort words and
/// permission flags (the production scrub redacts those too).
pub const FIXED_SCRUB_TERMS: [&str; 19] = [
    "grok",
    "opus",
    "fable",
    "sonnet",
    "astra",
    "sol",
    "gemini",
    "antigravity",
    "agy",
    "kimi",
    "muse",
    "claude",
    "codex",
    "cursor",
    "opencode",
    "dsh",
    "pi",
    "xhigh",
    "extra-high",
];
pub const EFFORT_TERMS: [&str; 6] = [
    "extra high",
    "high effort",
    "effort high",
    "model_reasoning_effort",
    "effort",
    "--effort",
];
pub const PERMISSION_FLAGS: [&str; 8] = [
    "--dangerously-skip-permissions",
    "--dangerously-bypass-approvals-and-sandbox",
    "--skip-permissions",
    "--full-auto",
    "--force",
    "--yolo",
    "--yes",
    "-y",
];

/// One Noul question per gate, ready for the request body.
#[derive(Debug, Clone, PartialEq)]
pub struct Question {
    /// The gate id: the gate's recipe id (SPEC-jev-picker v2 §2 Design C).
    pub id: String,
    pub instructions: String,
    pub criteria_true: String,
    pub criteria_false: String,
    pub threshold: f64,
}

/// What the state carries (SPEC-jev-picker v2 §2 Input).
#[derive(Debug, Clone, Default)]
pub struct StateInput {
    pub task: String,
    pub title: Option<String>,
    pub sentence: Option<String>,
    pub role: String,
    pub round: Option<String>,
    pub project_name: Option<String>,
    pub project_goal: Option<String>,
    pub repo: Option<String>,
    pub policy: Option<String>,
}

/// The scrub list: every recipe kind, every `--model` or `model=` value and
/// its words, every recipe `plain` phrase, plus the fixed list
/// (SPEC-jev-picker v2 §2 Input).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScrubList {
    patterns: Vec<String>,
}

impl ScrubList {
    pub fn new(recipes: &BTreeMap<String, Recipe>) -> Self {
        let mut set: BTreeSet<String> = FIXED_SCRUB_TERMS
            .iter()
            .chain(EFFORT_TERMS.iter())
            .chain(PERMISSION_FLAGS.iter())
            .map(|term| term.to_ascii_lowercase())
            .collect();
        for recipe in recipes.values() {
            if !recipe.kind.trim().is_empty() {
                set.insert(recipe.kind.trim().to_ascii_lowercase());
            }
            for (index, arg) in recipe.args.iter().enumerate() {
                let value = if arg == "--model" || arg == "-m" {
                    recipe.args.get(index + 1).cloned()
                } else {
                    arg.strip_prefix("model=").map(|value| value.to_string())
                };
                if let Some(value) = value {
                    add_model_words(&mut set, &value);
                }
                // A `key=value` pair such as the codex `-c` flags.
                if !arg.starts_with('-')
                    && let Some((key, value)) = arg.split_once('=')
                {
                    set.insert(arg.to_ascii_lowercase());
                    if !key.is_empty() {
                        set.insert(key.to_ascii_lowercase());
                    }
                    add_model_words(&mut set, value);
                }
            }
            let plain = recipe.plain.trim();
            if !plain.is_empty() {
                set.insert(plain.to_ascii_lowercase());
            }
        }
        let mut patterns: Vec<String> = set.into_iter().filter(|p| !p.is_empty()).collect();
        // Longest first, so a whole model id wins over one of its words.
        patterns.sort_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
        ScrubList { patterns }
    }

    /// The fixed list alone, for callers that build the state before the table
    /// is validated.
    pub fn fixed() -> Self {
        Self::new(&BTreeMap::new())
    }

    pub fn patterns(&self) -> &[String] {
        &self.patterns
    }

    /// Replace every whole occurrence with `[agent]` and collapse runs.
    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_string();
        for pattern in &self.patterns {
            out = replace_whole(&out, pattern);
        }
        collapse_agents(&out)
    }

    /// A stable digest of the rules, folded into `policy_hash`.
    pub fn fingerprint(&self) -> String {
        sha256_hex(self.patterns.join("\u{1f}").as_bytes())
    }
}

/// Add a model value and each of its word pieces to the scrub set.
fn add_model_words(set: &mut BTreeSet<String>, value: &str) {
    let full = value.trim().to_ascii_lowercase();
    if full.is_empty() {
        return;
    }
    for word in full.split(|c: char| !c.is_ascii_alphanumeric()) {
        if word.len() >= 2 && word.chars().any(|c| c.is_ascii_alphabetic()) {
            set.insert(word.to_string());
        }
    }
    set.insert(full);
}

/// Replace `pattern` (lower-case) when it stands on its own: the bytes before
/// and after are not name characters. Case-insensitive.
fn replace_whole(text: &str, pattern: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let bytes = text.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &text[i..];
        if starts_with_ignore_case(rest, pattern) {
            let before_ok = i == 0 || !is_name_at(bytes, i - 1);
            let end = i + pattern.len();
            let after_ok = end >= bytes.len() || !is_name_at(bytes, end);
            if before_ok && after_ok {
                out.push_str("[agent]");
                i = end;
                continue;
            }
        }
        let Some(ch) = rest.chars().next() else { break };
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn starts_with_ignore_case(text: &str, pattern: &str) -> bool {
    let mut chars = text.chars();
    for want in pattern.chars() {
        match chars.next() {
            Some(got) if got.to_ascii_lowercase() == want => {}
            _ => return false,
        }
    }
    true
}

/// A name character joins a token: letters, digits, `_` and `-` always, and a
/// `.` only when an alphanumeric follows it (inside `3.8` or `file.rs`). A
/// trailing period ends the token, so `Cursor.` is scrubbed.
fn is_name_at(bytes: &[u8], index: usize) -> bool {
    match bytes.get(index) {
        Some(byte) if byte.is_ascii_alphanumeric() || *byte == b'_' || *byte == b'-' => true,
        Some(byte) if *byte == b'.' => bytes
            .get(index + 1)
            .is_some_and(|next| next.is_ascii_alphanumeric()),
        _ => false,
    }
}

fn collapse_agents(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(pos) = rest.find("[agent]") {
        out.push_str(&rest[..pos]);
        out.push_str("[agent]");
        let mut tail = &rest[pos + "[agent]".len()..];
        loop {
            let trimmed = tail.trim_start_matches([' ', ',', ';']);
            if let Some(after) = trimmed.strip_prefix("[agent]") {
                tail = after;
            } else {
                break;
            }
        }
        rest = tail;
    }
    out.push_str(rest);
    out
}

/// The excerpt: drop start lines and fenced code, scrub, collapse blank runs,
/// cut at 2,200 characters on a word boundary (SPEC-jev-picker v2 §2 Input).
pub fn transform_task(task: &str, scrub: &ScrubList) -> String {
    let mut kept: Vec<&str> = Vec::new();
    let mut in_fence = false;
    for line in task.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        if trimmed.starts_with("Start line") || line.contains("herdr agent start") {
            continue;
        }
        kept.push(line);
    }
    let mut collapsed: Vec<&str> = Vec::new();
    for line in kept {
        if line.trim().is_empty() && collapsed.last().is_some_and(|l| l.trim().is_empty()) {
            continue;
        }
        collapsed.push(line);
    }
    while collapsed.first().is_some_and(|l| l.trim().is_empty()) {
        collapsed.remove(0);
    }
    while collapsed.last().is_some_and(|l| l.trim().is_empty()) {
        collapsed.pop();
    }
    let scrubbed = scrub.scrub(&collapsed.join("\n"));
    cut_on_word_boundary(scrubbed.trim(), EXCERPT_LIMIT).to_string()
}

fn cut_on_word_boundary(text: &str, limit: usize) -> &str {
    if text.len() <= limit {
        return text;
    }
    let mut end = limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let prefix = &text[..end];
    match prefix.rfind(char::is_whitespace) {
        Some(pos) if pos > 0 => prefix[..pos].trim_end(),
        _ => prefix.trim_end(),
    }
}

/// File extensions found in the task, at most 8, sorted
/// (SPEC-jev-picker v2 §2 Input).
pub fn languages(task: &str) -> Vec<String> {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for chunk in task.split(char::is_whitespace) {
        let token = chunk.trim_matches(|c: char| {
            !c.is_ascii_alphanumeric() && !matches!(c, '.' | '_' | '-' | '/' | '~')
        });
        let Some((stem, extension)) = token.rsplit_once('.') else {
            continue;
        };
        if stem.is_empty() || extension.len() < 2 || extension.len() > 8 {
            continue;
        }
        if !extension.chars().all(|c| c.is_ascii_alphanumeric())
            || !extension.chars().any(|c| c.is_ascii_alphabetic())
        {
            continue;
        }
        if !stem
            .chars()
            .last()
            .is_some_and(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == '*')
        {
            continue;
        }
        set.insert(extension.to_ascii_lowercase());
        if set.len() == 8 {
            break;
        }
    }
    set.into_iter().collect()
}

/// The `state` object sent to Jev (SPEC-jev-picker v2 §2 Input).
pub fn build_state(input: &StateInput) -> serde_json::Value {
    let mut state = serde_json::Map::new();
    state.insert("task".into(), serde_json::json!(input.task));
    for (key, value) in [
        ("title", input.title.as_deref()),
        ("sentence", input.sentence.as_deref()),
        ("round", input.round.as_deref()),
        ("repo", input.repo.as_deref()),
        ("policy", input.policy.as_deref()),
    ] {
        if let Some(value) = value.filter(|v| !v.trim().is_empty()) {
            state.insert(key.into(), serde_json::json!(value));
        }
    }
    state.insert("role".into(), serde_json::json!(input.role));
    state.insert(
        "project".into(),
        serde_json::json!({
            "name": input.project_name.clone().unwrap_or_default(),
            "goal": input.project_goal.clone().unwrap_or_default(),
        }),
    );
    state.insert(
        "languages".into(),
        serde_json::json!(languages(&input.task)),
    );
    serde_json::Value::Object(state)
}

/// One Noul per gate, keyed by the gate id (SPEC-jev-picker v2 §2 Design C).
pub fn questions(questions: &[Question]) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for question in questions {
        map.insert(
            question.id.clone(),
            serde_json::json!({
                "type": "noul",
                "instructions": question.instructions,
                "criteria": { "true": question.criteria_true, "false": question.criteria_false },
            }),
        );
    }
    serde_json::Value::Object(map)
}

/// The request body: one request, one Noul per gate, pinned model
/// (SPEC-jev-picker v2 §5).
pub fn request_body(state: &serde_json::Value, questions: &[Question], model: &str) -> String {
    serde_json::json!({
        "state": state,
        "model": model,
        "questions": self::questions(questions),
    })
    .to_string()
}

/// `instructions + criteria + excerpt transform version`
/// (SPEC-jev-picker v2 §2 Output).
pub fn prompt_hash(questions: &[Question], excerpt_version: u32) -> String {
    let canonical = serde_json::json!({
        "excerpt_version": excerpt_version,
        "questions": questions
            .iter()
            .map(|q| serde_json::json!({
                "id": q.id,
                "instructions": q.instructions,
                "criteria": { "true": q.criteria_true, "false": q.criteria_false },
            }))
            .collect::<Vec<_>>(),
    })
    .to_string();
    sha256_hex(canonical.as_bytes())
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// The Nouls that fired: the highest at or over its threshold; a tie keeps
/// the default (SPEC-jev-picker v2 §2 Design C).
pub fn fired_gate<'a>(
    questions: &'a [Question],
    nouls: &BTreeMap<String, f64>,
) -> Option<&'a Question> {
    let mut best: Option<(&Question, f64)> = None;
    let mut tied = false;
    for question in questions {
        let Some(p) = nouls.get(&question.id).copied() else {
            continue;
        };
        if p < question.threshold {
            continue;
        }
        match best {
            None => best = Some((question, p)),
            Some((_, best_p)) if p > best_p => {
                best = Some((question, p));
                tied = false;
            }
            Some((_, best_p)) if p == best_p => tied = true,
            _ => {}
        }
    }
    if tied { None } else { best.map(|(q, _)| q) }
}

/// Parsed answers of an accepted HTTP 200 body.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Answers {
    pub model: String,
    pub nouls: BTreeMap<String, f64>,
    pub input_tokens: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Parsed {
    Answers(Answers),
    ModelMismatch { model: String },
    Malformed,
}

/// Parse an HTTP 200 body: the model must be the pin, and every answer must be
/// a Noul number (SPEC-jev-picker v2 §3 step 6).
pub fn parse_body(body: &str, pinned_model: &str) -> Parsed {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(body) else {
        return Parsed::Malformed;
    };
    let Some(object) = value.as_object() else {
        return Parsed::Malformed;
    };
    let model = object
        .get("model")
        .and_then(|m| m.as_str())
        .unwrap_or_default()
        .to_string();
    if model != pinned_model {
        return Parsed::ModelMismatch { model };
    }
    let Some(answers) = object.get("answers").and_then(|a| a.as_object()) else {
        return Parsed::Malformed;
    };
    let mut nouls = BTreeMap::new();
    for (id, answer) in answers {
        let Some(item) = answer.as_object() else {
            return Parsed::Malformed;
        };
        if item.get("type").and_then(|t| t.as_str()) != Some("noul") {
            return Parsed::Malformed;
        }
        let Some(p) = item.get("noul").and_then(|n| n.as_f64()) else {
            return Parsed::Malformed;
        };
        nouls.insert(id.clone(), p);
    }
    let input_tokens = object
        .get("usage")
        .and_then(|u| u.get("input_tokens"))
        .and_then(|t| t.as_u64());
    Parsed::Answers(Answers {
        model,
        nouls,
        input_tokens,
    })
}

/// Transport-level result of one call (one or two attempts).
#[derive(Debug, Clone, PartialEq)]
pub enum Transport {
    Response { status: u16, body: String },
    Timeout,
    Failed { code: Option<i32>, detail: String },
    NoCurl,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Call {
    pub transport: Transport,
    pub attempts: u32,
}

/// POST the request through `/usr/bin/curl`, one retry on 429, 529 or a
/// connect error while the total budget allows (SPEC-jev-picker v2 §3 step 5,
/// §5).
pub fn call(runner: &dyn Runner, key: &str, body: &str, budget: Duration) -> Call {
    let start = Instant::now();
    let mut attempts = 0;
    loop {
        attempts += 1;
        let remaining = budget.saturating_sub(start.elapsed());
        if remaining.is_zero() {
            return Call {
                transport: Transport::Timeout,
                attempts,
            };
        }
        let config = systemone_config(key, body);
        let cmd = Cmd::new(CURL, remaining + Duration::from_millis(300))
            .args(curl_args(remaining))
            .stdin(config);
        let output = match runner.run(&cmd) {
            Ok(output) => output,
            Err(_) => {
                return Call {
                    transport: Transport::NoCurl,
                    attempts,
                };
            }
        };
        let transport = classify(&output);
        let retry = match &transport {
            Transport::Response { status, .. } => matches!(*status, 429 | 529),
            Transport::Failed { code, .. } => code
                .map(|code| CONNECT_EXIT_CODES.contains(&code))
                .unwrap_or(false),
            _ => false,
        };
        if !retry || attempts >= MAX_ATTEMPTS {
            return Call {
                transport,
                attempts,
            };
        }
    }
}

fn curl_args(timeout: Duration) -> Vec<String> {
    vec![
        "--silent".into(),
        "--show-error".into(),
        "--max-time".into(),
        format!("{:.1}", timeout.as_secs_f64()),
        "--config".into(),
        "-".into(),
    ]
}

fn classify(output: &Output) -> Transport {
    if output.timed_out {
        return Transport::Timeout;
    }
    let stdout = output.stdout.trim_end_matches(['\n', '\r']);
    if let Some((body, status)) = stdout.rsplit_once('\n') {
        if let Ok(status) = status.trim().parse::<u16>() {
            return Transport::Response {
                status,
                body: body.to_string(),
            };
        }
    } else if let Ok(status) = stdout.trim().parse::<u16>() {
        return Transport::Response {
            status,
            body: String::new(),
        };
    }
    Transport::Failed {
        code: output.code,
        detail: output.error_text(),
    }
}

/// The curl config written to the child's stdin. The key is never in argv or
/// the child environment (SPEC-jev-picker v2 §5).
pub fn systemone_config(key: &str, body: &str) -> String {
    let mut config = String::new();
    config.push_str(&format!("url = \"{SYSTEMONE_URL}\"\n"));
    config.push_str(&format!("header = \"Authorization: Bearer {key}\"\n"));
    config.push_str("header = \"Content-Type: application/json\"\n");
    config.push_str(&format!("data-binary = \"{}\"\n", escape_curl(body)));
    config.push_str("write-out = \"\\n%{http_code}\"\n");
    config
}

fn models_config(key: &str) -> String {
    let mut config = String::new();
    config.push_str(&format!("url = \"{MODELS_URL}\"\n"));
    config.push_str(&format!("header = \"Authorization: Bearer {key}\"\n"));
    config.push_str("write-out = \"\\n%{http_code}\"\n");
    config
}

fn escape_curl(text: &str) -> String {
    text.replace('\\', "\\\\").replace('"', "\\\"")
}

/// `GET /v1/models`: the doctor's live key check only
/// (SPEC-jev-picker v2 §5). Returns the HTTP status and a short body head.
pub fn models_probe(runner: &dyn Runner, key: &str, budget: Duration) -> (u16, String) {
    let cmd = Cmd::new(CURL, budget + Duration::from_millis(300))
        .args(curl_args(budget))
        .stdin(models_config(key));
    match runner.run(&cmd) {
        Ok(output) => match classify(&output) {
            Transport::Response { status, body } => (status, body.trim().to_string()),
            Transport::Timeout => (0, "timed out".into()),
            Transport::Failed { detail, .. } => (0, detail),
            Transport::NoCurl => (0, format!("{CURL} could not be run")),
        },
        Err(error) => (0, format!("{error:#}")),
    }
}

/// Which kends `herdr agent start` accepts, parsed from its `--help`
/// (SPEC-jev-picker v2 §2 Validation).
pub fn parse_kinds(help: &str) -> Option<BTreeSet<String>> {
    let marker = "[possible values:";
    let start = help.find(marker)? + marker.len();
    let rest = &help[start..];
    let end = rest.find(']')?;
    let kinds: BTreeSet<String> = rest[..end]
        .split(',')
        .map(|kind| kind.trim().to_string())
        .filter(|kind| !kind.is_empty())
        .collect();
    if kinds.is_empty() { None } else { Some(kinds) }
}

/// Where the key comes from; the value is never carried here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeySource {
    Env,
    File,
    Missing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyReport {
    pub source: KeySource,
    pub path: PathBuf,
    /// Unix mode bits of the file when it exists.
    pub mode: Option<u32>,
    /// True when the file is readable by group or others.
    pub readable_by_others: bool,
}

pub fn key_path(env: &Env) -> PathBuf {
    env.home.join(KEY_RELATIVE)
}

/// `TYPESAFE_API_KEY`, else `~/.config/typesafe/api_key`
/// (SPEC-jev-picker v2 §5).
pub fn load_key(env: &Env) -> Option<String> {
    if let Some(value) = env.var("TYPESAFE_API_KEY") {
        let value = value.trim();
        if !value.is_empty() {
            return Some(value.to_string());
        }
    }
    let text = std::fs::read_to_string(key_path(env)).ok()?;
    let key = text.trim();
    if key.is_empty() {
        None
    } else {
        Some(key.to_string())
    }
}

pub fn key_report(env: &Env) -> KeyReport {
    let path = key_path(env);
    let (source, mode, readable_by_others) = if let Some(value) = env.var("TYPESAFE_API_KEY") {
        if value.trim().is_empty() {
            (KeySource::Missing, None, false)
        } else {
            (KeySource::Env, None, false)
        }
    } else if let Ok(metadata) = std::fs::metadata(&path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = metadata.permissions().mode() & 0o7777;
            (KeySource::File, Some(mode), mode & 0o077 != 0)
        }
        #[cfg(not(unix))]
        {
            (KeySource::File, None, false)
        }
    } else {
        (KeySource::Missing, None, false)
    };
    KeyReport {
        source,
        path,
        mode,
        readable_by_others,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::{CostClass, Gate, GateCriteria};
    use crate::runner::fake::{FakeRunner, fail, ok, timeout};

    fn recipes() -> BTreeMap<String, Recipe> {
        let mut map = BTreeMap::new();
        map.insert(
            "cursor_grok_xhigh".into(),
            Recipe {
                kind: "cursor".into(),
                args: vec![
                    "--model".into(),
                    "cursor-grok-4.6-xhigh".into(),
                    "--force".into(),
                ],
                cost: CostClass::Default,
                plain: "the usual coding helper".into(),
                ..Recipe::default()
            },
        );
        map.insert(
            "agy_gemini_flash".into(),
            Recipe {
                kind: "agy".into(),
                args: vec![
                    "--model".into(),
                    "gemini-3.8-flash-high".into(),
                    "--dangerously-skip-permissions".into(),
                ],
                cost: CostClass::Sideways,
                plain: "the web research helper".into(),
                ..Recipe::default()
            },
        );
        map.insert(
            "codex_sol_high".into(),
            Recipe {
                kind: "codex".into(),
                args: vec![
                    "-c".into(),
                    "model=gpt-5.6-sol".into(),
                    "-c".into(),
                    "model_reasoning_effort=high".into(),
                ],
                cost: CostClass::Upgrade,
                plain: "the careful number helper".into(),
                ..Recipe::default()
            },
        );
        map
    }

    fn question() -> Question {
        Question {
            id: "agy_gemini_flash".into(),
            instructions: "Is the main job of `task` web research?".into(),
            criteria_true: "Web research with citations.".into(),
            criteria_false: "Implementation.".into(),
            threshold: 0.75,
        }
    }

    #[test]
    fn a_successful_body_parses_with_its_nouls() {
        let body = r#"{"model":"jev-1.13.0","answers":{"web":{"type":"noul","noul":0.91}},"usage":{"input_tokens":356}}"#;
        match parse_body(body, JEV_MODEL) {
            Parsed::Answers(answers) => {
                assert_eq!(answers.nouls["web"], 0.91);
                assert_eq!(answers.input_tokens, Some(356));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn another_model_is_a_mismatch_not_a_pick() {
        let body = r#"{"model":"jev-1.12.0","answers":{"web":{"type":"noul","noul":0.91}}}"#;
        assert_eq!(
            parse_body(body, JEV_MODEL),
            Parsed::ModelMismatch {
                model: "jev-1.12.0".into()
            }
        );
    }

    #[test]
    fn a_malformed_body_is_malformed() {
        for body in [
            "not json",
            "[]",
            r#"{"model":"jev-1.13.0"}"#,
            r#"{"model":"jev-1.13.0","answers":{"web":{"noul":0.9}}}"#,
            r#"{"model":"jev-1.13.0","answers":{"web":{"type":"noul"}}}"#,
            r#"{"model":"jev-1.13.0","answers":{"web":"yes"}}"#,
        ] {
            assert_eq!(parse_body(body, JEV_MODEL), Parsed::Malformed, "{body}");
        }
    }

    #[test]
    fn the_fired_gate_is_the_highest_and_a_tie_keeps_the_default() {
        let low = Question {
            id: "low".into(),
            threshold: 0.5,
            ..question()
        };
        let high = Question {
            id: "high".into(),
            threshold: 0.75,
            ..question()
        };
        let mut nouls = BTreeMap::new();
        nouls.insert("low".into(), 0.9);
        nouls.insert("high".into(), 0.91);
        assert_eq!(
            fired_gate(&[low.clone(), high.clone()], &nouls).unwrap().id,
            "high"
        );
        nouls.insert("high".into(), 0.9);
        assert!(fired_gate(&[low.clone(), high.clone()], &nouls).is_none());
        nouls.insert("high".into(), 0.4);
        assert_eq!(fired_gate(&[low, high], &nouls).unwrap().id, "low");
    }

    #[test]
    fn the_transform_drops_start_lines_and_code_then_scrubs() {
        let scrub = ScrubList::new(&recipes());
        let task = "Start line: herdr agent start l1 --kind cursor -- --model cursor-grok-4.6-xhigh\n\
                    Read the datasheet with the Agy helper.\n\
                    ```\nherdr agent start hidden --kind codex\n```\n\
                    Then write code with Cursor.\n";
        let excerpt = transform_task(task, &scrub);
        assert!(!excerpt.contains("herdr agent start"), "{excerpt}");
        assert!(!excerpt.contains("datasheet with the Agy"), "{excerpt}");
        assert!(!excerpt.contains("cursor-grok"), "{excerpt}");
        assert!(
            excerpt.contains("Read the datasheet with the [agent] helper."),
            "{excerpt}"
        );
        assert!(
            excerpt.contains("Then write code with [agent]."),
            "{excerpt}"
        );
        assert!(!excerpt.contains("```"), "{excerpt}");
    }

    #[test]
    fn the_excerpt_cuts_at_a_word_boundary_at_2200() {
        let scrub = ScrubList::fixed();
        let word = "word ";
        let task = word.repeat(1000);
        let excerpt = transform_task(&task, &scrub);
        assert!(excerpt.len() <= EXCERPT_LIMIT, "{}", excerpt.len());
        assert!(!excerpt.ends_with(' '));
        assert!(excerpt.starts_with("word word"));
        let single = "x".repeat(EXCERPT_LIMIT + 50);
        assert_eq!(transform_task(&single, &scrub).len(), EXCERPT_LIMIT);
    }

    #[test]
    fn the_scrub_list_covers_model_words_and_plain_phrases() {
        let scrub = ScrubList::new(&recipes());
        let text = "gemini flash and the web research helper and gpt-5.6-sol";
        let out = scrub.scrub(text);
        assert!(!out.contains("flash"), "{out}");
        assert!(!out.contains("web research helper"), "{out}");
        assert!(!out.contains("gpt"), "{out}");
        assert!(scrub.fingerprint().len() == 64);
    }

    #[test]
    fn languages_are_found_and_capped_at_eight() {
        assert_eq!(
            languages("edit src/plain.rs and tests/picker_plain.rs and Cargo.toml"),
            ["rs", "toml"]
        );
        assert!(languages("no files here").is_empty());
        assert!(languages("read e.g. the notes").is_empty());
        let many = "a.aa b.bb c.cc d.dd e.ee f.ff g.gg h.hh i.ii j.jj";
        assert_eq!(languages(many).len(), 8);
    }

    #[test]
    fn the_state_carries_only_named_fields() {
        let state = build_state(&StateInput {
            task: "write the parser".into(),
            title: Some("Parser".into()),
            sentence: Some("The lane writes the parser.".into()),
            role: "lane".into(),
            round: Some("r1".into()),
            project_name: Some("demo".into()),
            project_goal: Some("ship".into()),
            repo: Some("demo".into()),
            policy: None,
        });
        assert_eq!(state["task"], "write the parser");
        assert_eq!(state["role"], "lane");
        assert_eq!(state["project"]["name"], "demo");
        assert_eq!(state["languages"].as_array().unwrap().len(), 0);
        assert!(state.get("policy").is_none());
    }

    #[test]
    fn the_request_pins_the_model_and_sends_one_noul_per_gate() {
        let state = serde_json::json!({"task": "t"});
        let body = request_body(&state, &[question()], JEV_MODEL);
        let value: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(value["model"], JEV_MODEL);
        assert_eq!(value["questions"]["agy_gemini_flash"]["type"], "noul");
        assert_eq!(prompt_hash(&[question()], EXCERPT_VERSION).len(), 64);
    }

    #[test]
    fn the_curl_command_keeps_the_key_out_of_argv() {
        let config = systemone_config("sec-ret", r#"{"a":"b\c"}"#);
        let cmd = Cmd::new(CURL, Duration::from_secs(3))
            .args(curl_args(Duration::from_secs(3)))
            .stdin(config.clone());
        assert!(!cmd.display().contains("sec-ret"));
        assert!(config.contains("data-binary = \"{\\\"a\\\":\\\"b\\\\c\\\"}\""));
        assert!(config.contains("write-out = \"\\n%{http_code}\""));
    }

    #[test]
    fn a_200_call_is_one_attempt_and_the_body_is_the_rest_of_stdout() {
        let runner = FakeRunner::new();
        runner.on(
            "--config",
            ok("{\"model\":\"jev-1.13.0\",\"answers\":{}}\n200"),
        );
        let result = call(&runner, "k", "{}", Duration::from_secs(3));
        assert_eq!(result.attempts, 1);
        match result.transport {
            Transport::Response { status, body } => {
                assert_eq!(status, 200);
                assert!(body.starts_with("{\"model\""));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_429_is_retried_once_and_a_401_is_not() {
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.display().contains("--config"),
            |_| Ok(ok("{}\n429")),
        );
        let result = call(&runner, "k", "{}", Duration::from_secs(3));
        assert_eq!(result.attempts, 2);

        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.display().contains("--config"),
            |_| Ok(fail(0, "")),
        );
        let result = call(&runner, "k", "{}", Duration::from_secs(3));
        assert_eq!(result.attempts, 1);
        match result.transport {
            Transport::Failed { code, .. } => assert_eq!(code, Some(0)),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_timeout_is_not_retried_and_a_connect_error_is() {
        let runner = FakeRunner::new();
        runner.on_fn(|cmd| cmd.display().contains("--config"), |_| Ok(timeout()));
        let result = call(&runner, "k", "{}", Duration::from_secs(3));
        assert_eq!(result.attempts, 1);
        assert_eq!(result.transport, Transport::Timeout);

        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.display().contains("--config"),
            move |cmd| {
                if cmd.display().contains("--max-time") {
                    Ok(fail(7, "could not connect"))
                } else {
                    Ok(ok("{}"))
                }
            },
        );
        let result = call(&runner, "k", "{}", Duration::from_secs(3));
        assert_eq!(result.attempts, 2);
    }

    #[test]
    fn a_missing_curl_is_no_curl() {
        let runner = FakeRunner::new();
        let result = call(&runner, "k", "{}", Duration::from_secs(3));
        assert_eq!(result.transport, Transport::NoCurl);
    }

    #[test]
    fn kinds_parse_from_the_help_possible_values() {
        let kinds = parse_kinds(
            "Options:\n      --kind <KIND>\n          [possible values: pi, claude, codex, cursor, agy]\n",
        )
        .unwrap();
        assert!(kinds.contains("agy"));
        assert!(!kinds.contains("chatgpt"));
        assert!(parse_kinds("no list here").is_none());
    }

    #[test]
    fn the_key_report_names_the_source_and_the_mode() {
        let home = tempfile::tempdir().unwrap();
        let env = Env::for_test(home.path(), &[]);
        let report = key_report(&env);
        assert_eq!(report.source, KeySource::Missing);
        assert!(report.mode.is_none());

        let path = key_path(&env);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "abc\n").unwrap();
        assert_eq!(load_key(&env).as_deref(), Some("abc"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            let report = key_report(&env);
            assert_eq!(report.source, KeySource::File);
            assert_eq!(report.mode, Some(0o644));
            assert!(report.readable_by_others);
        }

        let env = Env::for_test(home.path(), &[("TYPESAFE_API_KEY", "env-key")]);
        assert_eq!(load_key(&env).as_deref(), Some("env-key"));
        assert_eq!(key_report(&env).source, KeySource::Env);
    }

    #[test]
    fn the_models_probe_reads_the_last_stdout_line() {
        let runner = FakeRunner::new();
        runner.on("--config", ok("{\"models\":[]}\n200"));
        assert_eq!(models_probe(&runner, "k", Duration::from_secs(3)).0, 200);
        let runner = FakeRunner::new();
        runner.on("--config", ok("{}\n401"));
        assert_eq!(models_probe(&runner, "k", Duration::from_secs(3)).0, 401);
    }

    #[test]
    fn a_gate_and_its_criteria_are_sent_verbatim() {
        let gate = Gate {
            recipe: "agy_gemini_flash".into(),
            cost: CostClass::Sideways,
            threshold: Some(0.75),
            instructions: "Is the main job of `task` web research?".into(),
            criteria: GateCriteria {
                is_true: "Web research with citations.".into(),
                is_false: "Implementation.".into(),
            },
        };
        let question = Question {
            id: gate.recipe.clone(),
            instructions: gate.instructions.clone(),
            criteria_true: gate.criteria.is_true.clone(),
            criteria_false: gate.criteria.is_false.clone(),
            threshold: gate.threshold.unwrap(),
        };
        let value = questions(&[question]);
        assert_eq!(value["agy_gemini_flash"]["instructions"], gate.instructions);
        assert_eq!(
            value["agy_gemini_flash"]["criteria"]["false"],
            "Implementation."
        );
    }
}
