//! Names at birth, `GLOSSARY.md`, `ha explain` and `ha term` (SPEC-ADE D17
//! item 6). The registry built here is passed to A0's checker as a value.

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
pub struct Entry {
    pub name: String,
    pub sentence: String,
    pub path: String,
    /// Birth time; `GLOSSARY.md` lists newest last.
    pub born: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Term {
    pub name: String,
    pub sentence: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub added: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct TermsFile {
    #[serde(default)]
    term: Vec<Term>,
}

fn terms_path(project: &Project) -> PathBuf {
    project.dir().join("terms.toml")
}

pub fn glossary_path(project: &Project) -> PathBuf {
    project.dir().join("GLOSSARY.md")
}

pub fn terms(project: &Project) -> Vec<Term> {
    std::fs::read_to_string(terms_path(project))
        .ok()
        .and_then(|t| toml::from_str::<TermsFile>(&t).ok())
        .map(|f| f.term)
        .unwrap_or_default()
}

/// Every born name: threads (id and branch), rounds, dialogues. Terms are
/// kept apart: a glossary term is never an exemption (R2).
pub fn names(project: &Project) -> Vec<Entry> {
    let mut out = Vec::new();
    for t in thread::list(project) {
        let sentence = crate::round::thread_plain(project, &t.id);
        let path = format!("tasks/{}.md", t.id);
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
pub fn registry(project: &Project) -> Glossary {
    let mut names_map = BTreeMap::new();
    for e in names(project) {
        names_map.entry(e.name).or_insert(e.sentence);
    }
    let terms_map = terms(project)
        .into_iter()
        .map(|t| (t.name, t.sentence))
        .collect();
    Glossary {
        names: names_map,
        terms: terms_map,
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
pub fn format_check(text: &str, result: &CheckResult) -> String {
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

/// Checks a text for Rolf's plane; the error carries the check's output.
pub fn gate(project: &Project, text: &str) -> Result<()> {
    let result = plain::check(text, &registry(project));
    if result.passed() {
        Ok(())
    } else {
        bail!("plain_refused:\n{}", format_check(text, &result))
    }
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
pub fn check_birth(project: &Project, sentence: &str) -> Result<()> {
    let sentence = sentence.trim();
    if sentence.is_empty() {
        bail!("plain_missing: a name needs --plain \"<one sentence>\"");
    }
    let glossary = registry(project);
    let mut problems = Vec::new();
    let enders = sentence
        .trim_end_matches(['.', '!', '?'])
        .chars()
        .filter(|c| matches!(c, '.' | '!' | '?' | '\n'))
        .count();
    if enders > 0 {
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

/// Rewrites `GLOSSARY.md` atomically from the records and the terms table,
/// one line per name, newest last.
pub fn rewrite(project: &Project) -> Result<()> {
    let mut entries: Vec<Entry> = names(project)
        .into_iter()
        .filter(|e| !e.sentence.is_empty())
        .collect();
    for t in terms(project) {
        entries.push(Entry {
            path: if t.path.is_empty() {
                "GLOSSARY.md".into()
            } else {
                t.path.clone()
            },
            name: t.name,
            sentence: t.sentence,
            born: t.added,
        });
    }
    entries.sort_by(|a, b| (&a.born, &a.name).cmp(&(&b.born, &b.name)));
    let mut text = String::from(
        "# Glossary\n\nWritten by herdr-ade from the records and the terms table. Do not edit by hand;\nadd a term with `term add <name> --plain \"<sentence>\"`.\n\n",
    );
    for e in &entries {
        text.push_str(&format!("- {}: {} ({})\n", e.name, e.sentence, e.path));
    }
    write_atomic(&glossary_path(project), text.as_bytes())
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
pub fn add_term(
    ctx: &Ctx,
    slug: &str,
    name: &str,
    plain: Option<&str>,
    path: Option<&str>,
) -> Result<Term> {
    let project = Project::load(&ctx.root, slug)?;
    validate_term_name(name)?;
    let Some(sentence) = plain.map(str::trim).filter(|p| !p.is_empty()) else {
        bail!(
            "plain_missing: `term add` needs --plain \"<one sentence that says what {name} is>\""
        );
    };
    let glossary = registry(&project);
    if glossary.names.contains_key(name) || glossary.terms.contains_key(name) {
        bail!("term_exists: `{name}` already has a sentence; see `explain {name}`");
    }
    if contains_name(sentence, name) {
        bail!(
            "plain_birth_refused:\nplain_birth: \"{name}\": the sentence cannot use the name it explains"
        );
    }
    check_birth(&project, sentence)?;
    let term = Term {
        name: name.to_string(),
        sentence: sentence.to_string(),
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
        write_atomic(&terms_path(&project), toml::to_string(&file)?.as_bytes())?;
    }
    rewrite(&project)?;
    Ok(term)
}

/// `ha explain <name>`: the recorded line plus its path.
pub fn explain(ctx: &Ctx, slug: &str, name: &str) -> Result<String> {
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
                        "GLOSSARY.md".into()
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
        Some(e) => bail!(
            "term_unborn: `{}` has no recorded sentence (a record from before the plain layer)",
            e.name
        ),
        None => {
            bail!("term_unknown: `{name}` is not a name in this project; add it with `term add`")
        }
    }
}
