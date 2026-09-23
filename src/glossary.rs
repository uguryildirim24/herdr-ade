//! Names at birth, `ha explain` and `ha term` (SPEC-ADE D17 item 6). The
//! registry built here is passed to A0's checker as a value.

use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths::Ctx;
use crate::plain::{self, CheckResult, Glossary};
use crate::project::{self, Project, write_atomic};
use crate::thread;

/// One glossary line: `- <name>: <sentence> (<path>)`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) name: String,
    pub(crate) sentence: String,
    pub(crate) path: String,
    /// Birth time; views list names in this order.
    pub(crate) born: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub(crate) struct Term {
    pub(crate) name: String,
    pub(crate) sentence: String,
    /// A familiar name needs no explanation on Rolf's board.
    #[serde(default)]
    pub(crate) familiar: bool,
    #[serde(default)]
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) added: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TermsFile {
    #[serde(default)]
    term: Vec<Term>,
}

fn terms_path(project: &Project) -> PathBuf {
    project.record_file("terms.toml")
}

pub(crate) fn terms(project: &Project) -> Vec<Term> {
    std::fs::read_to_string(terms_path(project))
        .ok()
        .and_then(|t| toml::from_str::<TermsFile>(&t).ok())
        .map(|f| f.term)
        .unwrap_or_default()
}

/// Every born name: threads (id and branch), rounds, dialogues. Terms are
/// kept apart: a glossary term is never an exemption (R2).
pub(crate) fn names(project: &Project) -> Vec<Entry> {
    let mut out = Vec::new();
    for t in thread::list(project) {
        let sentence = crate::round::thread_plain(project, &t.id);
        let path = if t.launch.brief_hash.is_empty() {
            format!(".state/threads/{}.toml", t.id)
        } else {
            format!(".state/artifacts/{}", t.launch.brief_hash)
        };
        out.push(Entry {
            name: t.id.clone(),
            sentence: sentence.clone(),
            path: path.clone(),
            born: t.created.clone(),
        });
        if !t.branch.is_empty() {
            out.push(Entry {
                name: t.branch.clone(),
                sentence,
                path,
                born: t.created.clone(),
            });
        }
    }
    for (name, sentence, path, born) in crate::round::registry_names(project) {
        out.push(Entry {
            name,
            sentence,
            path,
            born,
        });
    }
    for d in crate::dialogue::list(project) {
        out.push(Entry {
            name: d.topic.clone(),
            sentence: d.plain.clone(),
            path: format!("tasks/{}/", d.topic),
            born: d.started.clone(),
        });
        out.push(Entry {
            name: d.branch.clone(),
            sentence: d.plain.clone(),
            path: format!("tasks/{}/", d.topic),
            born: d.started.clone(),
        });
    }
    out
}

/// The checker's input value for this project (D17 item 1).
pub(crate) fn registry(project: &Project) -> Glossary {
    let mut names_map = BTreeMap::new();
    for e in names(project) {
        names_map.entry(e.name).or_insert(e.sentence);
    }
    let stored = terms(project);
    let mut familiar_names = std::collections::BTreeSet::new();
    familiar_names.insert(project.slug.to_ascii_lowercase());
    familiar_names.extend(
        stored
            .iter()
            .filter(|t| t.familiar)
            .map(|t| t.name.to_ascii_lowercase()),
    );
    let terms_map = stored.into_iter().map(|t| (t.name, t.sentence)).collect();
    Glossary {
        names: names_map,
        terms: terms_map,
        familiar_names,
        max_sentence_words: max_sentence_words(project),
    }
}

/// `[plain] max_sentence_words` in `PROJECT.md` front matter, default 25.
fn max_sentence_words(project: &Project) -> usize {
    std::fs::read_to_string(project.project_md())
        .ok()
        .and_then(|text| {
            let front = text
                .strip_prefix("+++\n")?
                .split_once("\n+++")
                .map(|(f, _)| f.to_string())?;
            let value: toml::Value = toml::from_str(&front).ok()?;
            value
                .get("plain")?
                .get("max_sentence_words")?
                .as_integer()
                .and_then(|n| usize::try_from(n).ok())
        })
        .unwrap_or(25)
}

/// The check's output in the form a model corrects from: each failing span,
/// its rule and the fixed fix text.
pub(crate) fn format_check(text: &str, result: &CheckResult) -> String {
    result
        .violations
        .iter()
        .map(|v| {
            let span = text.get(v.span.start..v.span.end).unwrap_or("");
            if span.is_empty() {
                format!("{}: {}", v.rule.code(), v.fix)
            } else {
                format!("{}: \"{}\": {}", v.rule.code(), span, v.fix)
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// One checked plain sentence for a coordinator-written field (plan step,
/// result sentence). Uses the project registry so a born name may appear in
/// gloss form, and requires exactly one sentence. Returns the trimmed text.
pub(crate) fn check_sentence(project: &Project, field: &str, text: &str) -> Result<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("plain_envelope: required field {field} is missing or empty");
    }
    let result = plain::check(trimmed, &registry(project));
    if !result.passed() {
        return Err(crate::refusal::error(format!(
            "plain_refused: {field}: {}",
            format_check(trimmed, &result)
        )));
    }
    let sentences = trimmed
        .split(['.', '?', '!', '\n'])
        .filter(|s| !s.trim().is_empty())
        .count();
    if sentences != 1 {
        return Err(crate::refusal::error(format!(
            "plain_refused: {field}: \"{trimmed}\": write one sentence; the limit is {} words",
            registry(project).max_words()
        )));
    }
    Ok(trimmed.to_string())
}

/// One sentence for an internal record. Exact names, technical words and
/// long accurate details are allowed; only the record's structure is checked.
pub(crate) fn check_internal_sentence(field: &str, text: &str) -> Result<String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("plain_envelope: required field {field} is missing or empty");
    }
    if plain::sentence_count(trimmed) != 1 {
        return Err(crate::refusal::error(format!(
            "plain_refused: {field}: \"{trimmed}\": write one sentence; the limit is one sentence"
        )));
    }
    Ok(trimmed.to_string())
}

/// Checks a text for Rolf's plane; the error carries the check's output.
pub(crate) fn gate(project: &Project, text: &str) -> Result<()> {
    let result = plain::check(text, &registry(project));
    if result.passed() {
        Ok(())
    } else {
        Err(crate::refusal::error(format!(
            "plain_refused:\n{}",
            format_check(text, &result)
        )))
    }
}

/// The first born name or term `text` carries, even in gloss form. Board
/// values carry none (item 14).
pub(crate) fn name_in(project: &Project, text: &str) -> Option<String> {
    let glossary = registry(project);
    names(project)
        .into_iter()
        .map(|e| e.name)
        .chain(
            terms(project)
                .into_iter()
                .filter(|t| !t.familiar)
                .map(|t| t.name),
        )
        .find(|name| !name.is_empty() && !glossary.is_plain_name(name) && contains_name(text, name))
}

fn contains_name(sentence: &str, name: &str) -> bool {
    let lower = sentence.to_ascii_lowercase();
    let name = name.to_ascii_lowercase();
    let bytes = lower.as_bytes();
    let mut from = 0;
    while let Some(rel) = lower[from..].find(&name) {
        let start = from + rel;
        let end = start + name.len();
        let before_ok = start == 0 || !is_name_char(bytes[start - 1]);
        let after_ok = end >= bytes.len() || !is_name_char(bytes[end]);
        if before_ok && after_ok {
            return true;
        }
        from = start + name.len().max(1);
    }
    false
}

fn is_name_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// The birth check (D17 item 6): one sentence, at most the word cap, every
/// word admitted by R4, no registry name and no identifier-shaped token.
pub(crate) fn check_birth(project: &Project, sentence: &str) -> Result<()> {
    let sentence = sentence.trim();
    if sentence.is_empty() {
        bail!("plain_missing: a name needs --plain \"<one sentence>\"");
    }
    let glossary = registry(project);
    let mut problems = Vec::new();
    if plain::sentence_count(sentence) != 1 {
        problems.push("plain_birth: write exactly one sentence".to_string());
    }
    for name in glossary.names.keys().chain(glossary.terms.keys()) {
        if contains_name(sentence, name) {
            problems.push(format!(
                "plain_birth: \"{name}\": a birth sentence cannot contain a name; say what it does in words"
            ));
        }
    }
    let result = plain::check(sentence, &glossary);
    if !result.passed() {
        problems.push(format_check(sentence, &result));
    }
    if problems.is_empty() {
        Ok(())
    } else {
        bail!("plain_birth_refused:\n{}", problems.join("\n"))
    }
}

/// A round's internal birth sentence keeps its one-sentence structure but has
/// no audience-prose or screen-width restrictions.
pub(crate) fn check_internal_birth(sentence: &str) -> Result<()> {
    let sentence = sentence.trim();
    if sentence.is_empty() {
        bail!("plain_missing: a name needs --plain \"<one sentence>\"");
    }
    let mut problems = Vec::new();
    if plain::sentence_count(sentence) != 1 {
        problems.push("plain_birth: write exactly one sentence".to_string());
    }
    if problems.is_empty() {
        Ok(())
    } else {
        bail!("plain_birth_refused:\n{}", problems.join("\n"))
    }
}

fn validate_term_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 64
        || name.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        bail!("term_name: `{name}` must be one word of at most 64 characters");
    }
    Ok(())
}

/// `ha term add <name> --plain "<sentence>"`.
pub(crate) fn add_term(
    ctx: &Ctx,
    slug: &str,
    name: &str,
    plain: Option<&str>,
    path: Option<&str>,
    familiar: bool,
) -> Result<Term> {
    let project = Project::load(&ctx.root, slug)?;
    validate_term_name(name)?;
    if familiar && plain.is_some() {
        bail!("term_name: use either --name or --plain, not both");
    }
    if familiar && plain::is_identifier_shaped(name) && !plain::is_camel_case(name) {
        bail!("term_name: `{name}` is a code or path, not a familiar name");
    }
    let sentence = if familiar {
        ""
    } else {
        plain.map(str::trim).filter(|p| !p.is_empty()).ok_or_else(|| {
            anyhow::anyhow!("plain_missing: `term add` needs --plain \"<one sentence that says what {name} is>\" or --name")
        })?
    };
    let glossary = registry(&project);
    if glossary.names.contains_key(name) || glossary.terms.contains_key(name) {
        bail!("term_exists: `{name}` already has a sentence; see `explain {name}`");
    }
    if !familiar {
        if contains_name(sentence, name) {
            bail!(
                "plain_birth_refused:\nplain_birth: \"{sentence}\": the sentence cannot use the name {name} it explains"
            );
        }
        check_birth(&project, sentence)?;
    }
    let term = Term {
        name: name.to_string(),
        sentence: sentence.to_string(),
        familiar,
        path: path.unwrap_or("").to_string(),
        added: project::now(),
    };
    {
        let _lock = project.lock()?;
        let mut file: TermsFile = std::fs::read_to_string(terms_path(&project))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default();
        if file.term.iter().any(|t| t.name == name) {
            bail!("term_exists: `{name}` already has a sentence");
        }
        file.term.push(term.clone());
        let path = project.record_file_for_write("terms.toml")?;
        write_atomic(&path, toml::to_string(&file)?.as_bytes())?;
    }
    Ok(term)
}

/// `ha explain <name>`: the recorded line plus its path.
pub(crate) fn explain(ctx: &Ctx, slug: &str, name: &str) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let found = names(&project)
        .into_iter()
        .find(|e| e.name == name)
        .or_else(|| {
            terms(&project)
                .into_iter()
                .find(|t| t.name == name)
                .map(|t| Entry {
                    name: t.name,
                    sentence: t.sentence,
                    path: if t.path.is_empty() {
                        "terms.toml".into()
                    } else {
                        t.path
                    },
                    born: t.added,
                })
        });
    match found {
        Some(e) if !e.sentence.is_empty() => {
            Ok(format!("{}: {}\n({})\n", e.name, e.sentence, e.path))
        }
        Some(e)
            if terms(&project)
                .iter()
                .any(|t| t.name == e.name && t.familiar) =>
        {
            Ok(format!("{}: familiar name\n({})\n", e.name, e.path))
        }
        Some(e) => bail!(
            "term_unborn: `{}` has no recorded sentence (a record from before the plain layer)",
            e.name
        ),
        None => {
            bail!("term_unknown: `{name}` is not a name in this project; add it with `term add`")
        }
    }
}
