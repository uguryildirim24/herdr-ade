//! The decided-for-you records (SPEC-talk §2.7 and §6.6): a decision history
//! at `<project>/decisions.jsonl`, its validation, retry keys, authority
//! references, current-choice folding and replacements.
//!
//! Plain-check before append, enforce the existing entry-size bound, write the
//! complete newline-terminated record and flush before acknowledging success.
//! A broken tail is not a decision: further writes are refused until it is
//! repaired.

use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::contracts::{AuthorityRef, DECISION_CLASSES, Decision};
use crate::glossary;
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::talk;

pub(crate) fn decisions_path(project: &Project) -> PathBuf {
    project.dir().join("decisions.jsonl")
}

fn lock_path(project: &Project) -> PathBuf {
    project.dir().join(".decisions.lock")
}

struct DecisionsLock {
    _file: File,
}

fn decisions_lock(project: &Project) -> Result<DecisionsLock> {
    let path = lock_path(project);
    let file = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    file.lock()?;
    Ok(DecisionsLock { _file: file })
}

/// The complete lines of the log, in order, plus whether the file ends without
/// a newline (a cut write that blocks further appends).
#[derive(Debug, Default)]
pub(crate) struct Log {
    pub(crate) records: Vec<Decision>,
    pub(crate) broken_tail: bool,
}

pub(crate) fn read(project: &Project) -> Log {
    let text = std::fs::read_to_string(decisions_path(project)).unwrap_or_default();
    let mut log = Log {
        broken_tail: !text.is_empty() && !text.ends_with('\n'),
        records: Vec::new(),
    };
    let mut parts: Vec<&str> = text.split('\n').collect();
    parts.pop();
    for part in parts {
        if part.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<Decision>(part) {
            log.records.push(record);
        }
    }
    log
}

/// The ids that a later valid record replaced.
fn replaced_ids(log: &Log) -> std::collections::BTreeSet<String> {
    log.records
        .iter()
        .filter_map(|d| d.replaces.clone())
        .collect()
}

/// The latest state of each unreplaced choice, oldest change first, including
/// overturned choices. The screen reads this rather than re-folding the log.
pub(crate) fn current(project: &Project) -> Vec<Decision> {
    fold_current(&read(project))
}

fn fold_current(log: &Log) -> Vec<Decision> {
    let replaced = replaced_ids(log);
    let mut latest = std::collections::BTreeMap::new();
    for d in &log.records {
        if !replaced.contains(&d.id) {
            latest.insert(d.id.clone(), d.clone());
        }
    }
    let mut records: Vec<_> = latest.into_values().collect();
    records.sort_by_key(|d| d.seq);
    records
}

/// Keep the original record and append its overturned state under the same id.
pub(crate) fn overturn(
    ctx: &Ctx,
    slug: &str,
    id: &str,
    reason: &str,
    by: &str,
) -> Result<Decision> {
    let project = Project::load(&ctx.root, slug)?;
    if reason.trim().is_empty() || by.trim().is_empty() {
        bail!("decision_overturn: a reason and actor are required");
    }
    let _lock = decisions_lock(&project)?;
    let log = read(&project);
    if log.broken_tail {
        bail!("decision_log_broken: repair the incomplete tail before writing");
    }
    let mut record = log
        .records
        .iter()
        .rev()
        .find(|d| d.id == id)
        .cloned()
        .with_context(|| format!("decision_unknown: `{id}` is not in this log"))?;
    if record.overturned.is_some() {
        bail!("decision_overturned: `{id}` is already overturned");
    }
    if !log_current(&log, id) {
        bail!("decision_replaced: `{id}` already has a replacement");
    }
    record.seq = log.records.iter().map(|d| d.seq).max().unwrap_or(0) + 1;
    record.overturned = Some(crate::contracts::DecisionOverturn {
        by: by.to_string(),
        at: project::now(),
        reason: reason.trim().to_string(),
    });
    append(&project, &record)?;
    Ok(record)
}

pub(crate) fn status_line(record: &Decision) -> String {
    match &record.overturned {
        Some(change) => format!(
            "{}  {}  overturned by {} at {}: {}",
            record.id, record.line, change.by, change.at, change.reason
        ),
        None => format!("{}  {}  {}", record.id, record.class, record.line),
    }
}

pub(crate) struct NewDecision<'a> {
    pub(crate) line: &'a str,
    pub(crate) class: &'a str,
    pub(crate) key: Option<&'a str>,
    pub(crate) basis: Option<&'a str>,
    pub(crate) replaces: Option<&'a str>,
    pub(crate) request: Option<&'a str>,
}

fn check_class(class: &str) -> Result<()> {
    if !DECISION_CLASSES.contains(&class) {
        bail!(
            "decision_class: `{class}` is not one of {}",
            DECISION_CLASSES.join(", ")
        );
    }
    Ok(())
}

/// A human message a `request:<id>` reference names.
fn request_exists(project: &Project, id: &str) -> bool {
    talk::read(project)
        .lines
        .iter()
        .any(|line| matches!(&line.entry, talk::Entry::Rolf { request, .. } if request == id))
}

/// Validates a `--basis` reference and its provenance (SPEC-talk §6.6): an
/// existing human message, or a current, nonzero answered ask. The presence of
/// a reference is not semantic proof of permission.
pub(crate) fn validate_basis(project: &Project, text: &str) -> Result<String> {
    let reference = AuthorityRef::parse(text).with_context(|| {
        format!("decision_basis: `{text}` is not `request:<id>` or `ask:<id>@<revision>`")
    })?;
    match &reference {
        AuthorityRef::Request(id) => {
            if !request_exists(project, id) {
                bail!("decision_basis: no message `{id}` in this project");
            }
        }
        AuthorityRef::Ask { id, revision } => {
            let latest = crate::ask::latest_revision(project, id);
            if latest != *revision {
                bail!("decision_basis: `{id}` is at revision {latest}, not {revision}");
            }
            let answer = crate::ask::answer_of(project, id, *revision)
                .with_context(|| format!("decision_basis: `{id}`@{revision} is not answered"))?;
            if answer.not_understood || answer.choice == 0 {
                bail!(
                    "decision_basis: the answer to `{id}`@{revision} is a no; it authorizes nothing"
                );
            }
        }
    }
    Ok(reference.as_str())
}

/// `ha decide` (SPEC-talk §6.6). An existing key with the same payload returns
/// its record; the same key with different content fails.
pub(crate) fn decide(ctx: &Ctx, slug: &str, new: NewDecision<'_>) -> Result<Decision> {
    let project = Project::load(&ctx.root, slug)?;
    check_class(new.class)?;
    let line = glossary::check_record_sentence(&project, "line", new.line)?;
    if new.class != "routine" && new.basis.is_none() {
        return Err(crate::refusal::error(format!(
            "decision_authority: a `{}` choice needs --basis with the permission it rests on",
            new.class
        )));
    }
    let basis = match new.basis {
        Some(basis) => Some(validate_basis(&project, basis)?),
        None => None,
    };
    let _lock = decisions_lock(&project)?;
    let log = read(&project);
    if log.broken_tail {
        bail!(
            "decision_log_broken: {} ends without a newline; repair it before writing",
            decisions_path(&project).display()
        );
    }
    if let Some(key) = new.key
        && let Some(existing) = log
            .records
            .iter()
            .rev()
            .find(|d| d.key.as_deref() == Some(key))
    {
        let same = existing.line == line
            && existing.class == new.class
            && existing.basis.as_deref() == basis.as_deref()
            && existing.replaces.as_deref() == new.replaces
            && existing.request.as_deref() == new.request;
        if same {
            return Ok(existing.clone());
        }
        bail!("decision_key: key `{key}` already names different content");
    }
    let mut request = new.request;
    if let Some(replaces) = new.replaces {
        request = Some(request.with_context(|| {
            format!("decision_replacement: --request is required with --replaces (for {replaces})")
        })?);
    }
    if let Some(request) = request
        && !request_exists(&project, request)
    {
        bail!("decision_request: no message `{request}` in this project");
    }
    if let Some(replaces) = new.replaces {
        if let Some(target) = log.records.iter().find(|d| d.id == replaces) {
            if !log_current(&log, &target.id) {
                bail!("decision_replaced: `{replaces}` already has a replacement");
            }
        } else {
            if !crate::note::target_exists(&project, replaces) {
                bail!("decision_unknown: `{replaces}` is not a note or decision");
            }
            let rows = crate::note::rows(&project);
            if crate::note::replacement_map(&rows).contains_key(replaces) {
                bail!("decision_replaced: `{replaces}` already has a replacement");
            }
        }
    }
    let seq = log.records.iter().map(|d| d.seq).max().unwrap_or(0) + 1;
    let record = Decision {
        schema: 1,
        seq,
        id: format!("d-{seq:04}"),
        at: project::now(),
        line,
        class: new.class.to_string(),
        key: new.key.map(str::to_string),
        basis,
        replaces: new.replaces.map(str::to_string),
        request: request.map(str::to_string),
        overturned: None,
    };
    append(&project, &record)?;
    Ok(record)
}

fn append(project: &Project, record: &Decision) -> Result<()> {
    let text = serde_json::to_string(record)?;
    if text.len() > talk::MAX_ENTRY_BYTES {
        bail!(
            "decision_too_large: {} bytes, at most {}",
            text.len(),
            talk::MAX_ENTRY_BYTES
        );
    }
    let mut file = File::options()
        .create(true)
        .append(true)
        .open(decisions_path(project))?;
    file.write_all(text.as_bytes())?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

/// Current according to an already-read log, not a fresh read (the lock holds).
fn log_current(log: &Log, id: &str) -> bool {
    let replaced = replaced_ids(log);
    log.records
        .iter()
        .rev()
        .find(|d| d.id == id)
        .is_some_and(|d| d.overturned.is_none())
        && !replaced.contains(id)
}

/// `ha decide list [--json]`: current choices, newest first.
pub(crate) fn list(ctx: &Ctx, slug: &str, json: bool) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let broken_tail = read(&project).broken_tail;
    let mut records = current(&project);
    records.reverse();
    if json {
        return Ok(format!("{}\n", serde_json::to_string_pretty(&records)?));
    }
    let mut out = String::new();
    if broken_tail {
        out.push_str(
            "warning: the decision log ends without a newline; repair it before writing\n",
        );
    }
    if records.is_empty() {
        out.push_str("no choices have been recorded yet\n");
    }
    for record in records {
        out.push_str(&format!("{}\n", status_line(&record)));
    }
    Ok(out)
}

/// `ha decide show <id> [--json]`.
pub(crate) fn show(ctx: &Ctx, slug: &str, id: &str, json: bool) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let log = read(&project);
    let record = log
        .records
        .iter()
        .rev()
        .find(|d| d.id == id)
        .with_context(|| format!("decision_unknown: `{id}` is not in this log"))?;
    if json {
        let mut value = serde_json::to_value(record)?;
        value["current"] = serde_json::json!(log_current(&log, id));
        return Ok(format!("{}\n", serde_json::to_string_pretty(&value)?));
    }
    Ok(format!(
        "{}{}\n",
        status_line(record),
        if log_current(&log, id) || record.overturned.is_some() {
            ""
        } else {
            "  (replaced)"
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::testkit::{Fx, fixture};

    fn decide_routine(fx: &Fx, line: &str) -> Decision {
        decide(
            &fx.world.ctx(),
            "demo",
            NewDecision {
                line,
                class: "routine",
                key: None,
                basis: None,
                replaces: None,
                request: None,
            },
        )
        .unwrap()
    }

    fn current(fx: &Fx) -> Vec<Decision> {
        fold_current(&read(&fx.project))
    }

    #[test]
    fn overturn_preserves_history_and_is_shown_in_views_and_context() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let original = decide_routine(&fx, "I kept the words short.");
        let changed = overturn(&ctx, "demo", &original.id, "I want more detail.", "rolf").unwrap();
        let log = read(&fx.project);
        assert_eq!(log.records, vec![original.clone(), changed.clone()]);
        let change = changed.overturned.as_ref().unwrap();
        assert_eq!(change.by, "rolf");
        assert_eq!(change.reason, "I want more detail.");
        assert!(!change.at.is_empty());
        assert_eq!(current(&fx), vec![changed]);
        assert!(
            list(&ctx, "demo", false)
                .unwrap()
                .contains("overturned by rolf")
        );
        let shown: serde_json::Value =
            serde_json::from_str(&show(&ctx, "demo", &original.id, true).unwrap()).unwrap();
        assert_eq!(shown["current"], false);
        assert_eq!(shown["overturned"]["by"], "rolf");
        assert!(
            show(&ctx, "demo", &original.id, false)
                .unwrap()
                .contains("I want more detail.")
        );
        let (digest, _) = crate::coordinator::digest(&ctx, &fx.project, "ha").unwrap();
        assert!(digest.contains("overturned by rolf"));
        assert!(digest.contains("do not repeat them"));
        assert!(
            overturn(&ctx, "demo", &original.id, "Again.", "rolf")
                .unwrap_err()
                .to_string()
                .starts_with("decision_overturned")
        );
        assert_eq!(read(&fx.project).records.len(), 2);
    }

    #[test]
    fn overturn_unknown_is_refused_without_writing() {
        let fx = fixture();
        assert!(
            overturn(&fx.world.ctx(), "demo", "d-9999", "No thanks.", "rolf")
                .unwrap_err()
                .to_string()
                .starts_with("decision_unknown")
        );
        assert!(read(&fx.project).records.is_empty());
    }

    #[test]
    fn a_decision_line_may_name_a_file_and_still_keeps_the_length_limit() {
        let fx = fixture();
        let record = decide_routine(&fx, "I changed config.toml today.");
        assert_eq!(record.line, "I changed config.toml today.");

        let two_sentences = decide(
            &fx.world.ctx(),
            "demo",
            NewDecision {
                line: "I changed config.toml. Then I checked it.",
                class: "routine",
                key: None,
                basis: None,
                replaces: None,
                request: None,
            },
        )
        .unwrap_err()
        .to_string();
        assert!(
            two_sentences.contains("write one sentence"),
            "{two_sentences}"
        );

        let long = format!("I changed {}.", vec!["config.toml"; 26].join(" "));
        let e = format!(
            "{:#}",
            decide(
                &fx.world.ctx(),
                "demo",
                NewDecision {
                    line: &long,
                    class: "routine",
                    key: None,
                    basis: None,
                    replaces: None,
                    request: None,
                }
            )
            .unwrap_err()
        );
        assert!(e.contains("plain_long_sentence"), "{e}");
    }

    fn keyed<'a>(line: &'a str) -> NewDecision<'a> {
        NewDecision {
            line,
            class: "routine",
            key: Some("k-1"),
            basis: None,
            replaces: None,
            request: None,
        }
    }

    /// A project ask answered with a nonzero choice, as `--basis ask:` needs.
    fn answered_ask(fx: &Fx) -> (String, u32) {
        let ctx = fx.world.ctx();
        let a = crate::ask::ask(
            &ctx,
            "demo",
            crate::ask::NewAsk {
                question: "keep the experiment running another hour, or stop now?".into(),
                choices: vec!["keep it running another hour".into(), "stop it now".into()],
                what: None,
                means: None,
                round: None,
                reask: None,
            },
        )
        .unwrap();
        crate::ask::answer(&ctx, "demo", &a.id, a.revision, 1, "test").unwrap();
        (a.id, a.revision)
    }

    #[test]
    fn a_routine_choice_is_recorded_once_and_folded_as_current() {
        let fx = fixture();
        let first = decide_routine(&fx, "I kept the words short.");
        assert_eq!((first.id.as_str(), first.seq), ("d-0001", 1));
        assert_eq!(first.class, "routine");
        assert!(first.key.is_none() && first.basis.is_none() && first.replaces.is_none());
        decide_routine(&fx, "I used the same words in both views.");
        assert_eq!(current(&fx).len(), 2);
        assert!(
            list(&fx.world.ctx(), "demo", false)
                .unwrap()
                .contains("d-0001")
        );
    }

    #[test]
    fn a_keyed_retry_returns_the_record_and_a_changed_payload_fails() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let a = decide(&ctx, "demo", keyed("I kept the words short.")).unwrap();
        let b = decide(&ctx, "demo", keyed("I kept the words short.")).unwrap();
        assert_eq!(a.id, b.id);
        assert_eq!(read(&fx.project).records.len(), 1);
        let e = format!(
            "{:#}",
            decide(&ctx, "demo", keyed("I changed my mind.")).unwrap_err()
        );
        assert!(e.starts_with("decision_key"), "{e}");
    }

    #[test]
    fn a_consequential_choice_needs_a_basis_and_a_no_answer_authorizes_nothing() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let error = decide(
            &ctx,
            "demo",
            NewDecision {
                line: "I will spend five dollars on the check.",
                class: "money",
                key: None,
                basis: None,
                replaces: None,
                request: None,
            },
        )
        .unwrap_err();
        assert!(crate::refusal::is(&error));
        let e = format!("{error:#}");
        assert!(e.starts_with("decision_authority"), "{e}");
        let (id, revision) = answered_ask(&fx);
        let e = format!(
            "{:#}",
            decide(
                &ctx,
                "demo",
                NewDecision {
                    line: "I will spend five dollars on the check.",
                    class: "money",
                    key: None,
                    basis: Some("ask:a-9@1"),
                    replaces: None,
                    request: None,
                }
            )
            .unwrap_err()
        );
        assert!(e.starts_with("decision_basis"), "{e}");
        let record = decide(
            &ctx,
            "demo",
            NewDecision {
                line: "I will spend five dollars on the check.",
                class: "money",
                key: None,
                basis: Some(&format!("ask:{id}@{revision}")),
                replaces: None,
                request: None,
            },
        )
        .unwrap();
        assert_eq!(
            record.basis.as_deref(),
            Some(format!("ask:{id}@{revision}").as_str())
        );
    }

    #[test]
    fn a_replacement_keeps_the_history_and_folds_to_the_new_choice() {
        let fx = fixture();
        let ctx = fx.world.ctx();
        let original = decide_routine(&fx, "I kept the words short.");
        // A replacement needs an incoming request that led to it.
        let request = "q-1234";
        crate::talk::append(
            &fx.project,
            None,
            crate::talk::Entry::Rolf {
                request: request.into(),
                text: "Put more detail beside those choices.".into(),
                answer: None,
            },
        )
        .unwrap();
        let e = format!(
            "{:#}",
            decide(
                &ctx,
                "demo",
                NewDecision {
                    line: "I will show more detail beside each choice.",
                    class: "routine",
                    key: None,
                    basis: None,
                    replaces: Some(&original.id),
                    request: None,
                }
            )
            .unwrap_err()
        );
        assert!(e.starts_with("decision_replacement"), "{e}");
        let replacement = decide(
            &ctx,
            "demo",
            NewDecision {
                line: "I will show more detail beside each choice.",
                class: "routine",
                key: None,
                basis: None,
                replaces: Some(&original.id),
                request: Some(request),
            },
        )
        .unwrap();
        assert_eq!(replacement.replaces.as_deref(), Some(original.id.as_str()));
        assert_eq!(read(&fx.project).records.len(), 2, "history is preserved");
        let folded = current(&fx);
        assert_eq!(folded.len(), 1);
        assert_eq!(folded[0].id, replacement.id);
        // The original cannot be replaced twice.
        let e = format!(
            "{:#}",
            decide(
                &ctx,
                "demo",
                NewDecision {
                    line: "I will do something else.",
                    class: "routine",
                    key: None,
                    basis: None,
                    replaces: Some(original.id.as_str()),
                    request: Some(request),
                }
            )
            .unwrap_err()
        );
        assert!(e.starts_with("decision_replaced"), "{e}");
    }

    #[test]
    fn a_broken_tail_blocks_further_writes() {
        let fx = fixture();
        decide_routine(&fx, "I kept the words short.");
        let mut file = File::options()
            .append(true)
            .open(decisions_path(&fx.project))
            .unwrap();
        file.write_all(br#"{"schema":1,"seq":2"#).unwrap();
        drop(file);
        let e = format!(
            "{:#}",
            decide(
                &fx.world.ctx(),
                "demo",
                NewDecision {
                    line: "I used the same words in both views.",
                    class: "routine",
                    key: None,
                    basis: None,
                    replaces: None,
                    request: None,
                }
            )
            .unwrap_err()
        );
        assert!(e.starts_with("decision_log_broken"), "{e}");
        assert_eq!(read(&fx.project).records.len(), 1);
    }
}
