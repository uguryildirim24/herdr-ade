//! Coordinator correction-hook lifecycle, budget, and typed envelope parsing.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contracts::HumanMessage;
use crate::paths::Ctx;
use crate::plain;
use crate::project::{self, Project};
use crate::remote::quote;
use crate::runner::Cmd;

const INPUT_LIMIT: usize = 64 * 1024;
const MAX_CORRECTIONS: u32 = 3;
const DEADLINE_SECS: i64 = 10 * 60;
const SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Binding {
    kind: String,
    project: String,
    pane: String,
    #[serde(default)]
    session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Budget {
    turn: String,
    corrections: u32,
    translator_runs: u32,
    started: String,
    started_second: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CursorPending {
    session: String,
    turn: String,
    text: String,
    reason: String,
}

fn binding_path(project: &Project) -> PathBuf {
    project.state_dir().join("plain").join("hook-binding.json")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigShape {
    ClaudeLike,
    Cursor,
}

fn settings_path(project: &Project, kind: &str) -> Option<(PathBuf, ConfigShape)> {
    match kind {
        "claude" => Some((
            project.dir().join(".claude").join("settings.local.json"),
            ConfigShape::ClaudeLike,
        )),
        "codex" => Some((
            project.dir().join(".codex").join("hooks.json"),
            ConfigShape::ClaudeLike,
        )),
        "cursor" => Some((
            project.dir().join(".cursor").join("hooks.json"),
            ConfigShape::Cursor,
        )),
        _ => None,
    }
}

pub fn install(ctx: &Ctx, project: &Project, kind: &str, pane: &str) -> Result<bool> {
    let Some((path, shape)) = settings_path(project, kind) else {
        return Ok(false);
    };
    let binary = std::env::current_exe().context("could not locate herdr-ade")?;
    let command = format!(
        "{} --root {} plain hook --kind {} --project {} --binding {}",
        quote(&binary.to_string_lossy()),
        quote(&ctx.root.to_string_lossy()),
        quote(kind),
        quote(&project.slug),
        quote(pane)
    );
    let _lock = project.lock()?;
    let mut value = read_json_object(&path)?;
    install_entry(&mut value, shape, &command)?;
    write_json_atomic(&path, &value)?;
    let dir = project.state_dir().join("plain");
    std::fs::create_dir_all(&dir)?;
    project::write_json(
        &binding_path(project),
        &Binding {
            kind: kind.to_string(),
            project: project.slug.clone(),
            pane: pane.to_string(),
            session_id: String::new(),
        },
    )?;
    verify_owned_entry(&path, pane, shape)?;
    Ok(true)
}

pub fn remove(project: &Project) -> Result<()> {
    let _lock = project.lock()?;
    let binding: Option<Binding> = project::read_json(&binding_path(project));
    if let Some(binding) = binding
        && let Some((path, shape)) = settings_path(project, &binding.kind)
        && path.exists()
    {
        let mut value = read_json_object(&path)?;
        remove_entries(&mut value, shape);
        write_json_atomic(&path, &value)?;
    }
    let _ = std::fs::remove_file(binding_path(project));
    Ok(())
}

fn owned_hook(value: &serde_json::Value) -> bool {
    value["command"]
        .as_str()
        .is_some_and(|command| command.contains(" plain hook --kind "))
        || value["hooks"].as_array().into_iter().flatten().any(|hook| {
            hook["command"]
                .as_str()
                .is_some_and(|command| command.contains(" plain hook --kind "))
        })
}

fn install_entry(value: &mut serde_json::Value, shape: ConfigShape, command: &str) -> Result<()> {
    let object = value
        .as_object_mut()
        .expect("read_json_object returns an object");
    if shape == ConfigShape::Cursor {
        object.insert("version".into(), serde_json::json!(1));
    }
    let hooks = object
        .entry("hooks")
        .or_insert_with(|| serde_json::json!({}))
        .as_object_mut()
        .context("hook_config_invalid: `hooks` is not an object")?;
    match shape {
        ConfigShape::ClaudeLike => {
            let stop = hooks
                .entry("Stop")
                .or_insert_with(|| serde_json::json!([]))
                .as_array_mut()
                .context("hook_config_invalid: `hooks.Stop` is not an array")?;
            stop.retain(|entry| !owned_hook(entry));
            stop.push(serde_json::json!({
                "matcher": "",
                "hooks": [{ "type": "command", "command": command }]
            }));
        }
        ConfigShape::Cursor => {
            for (event, phase) in [("afterAgentResponse", "observe"), ("stop", "stop")] {
                let entries = hooks
                    .entry(event)
                    .or_insert_with(|| serde_json::json!([]))
                    .as_array_mut()
                    .with_context(|| {
                        format!("hook_config_invalid: `hooks.{event}` is not an array")
                    })?;
                entries.retain(|entry| !owned_hook(entry));
                entries.push(serde_json::json!({
                    "command": format!("{command} --phase {phase}"),
                    "loop_limit": 3
                }));
            }
        }
    }
    Ok(())
}

fn remove_entries(value: &mut serde_json::Value, shape: ConfigShape) {
    let names: &[&str] = match shape {
        ConfigShape::ClaudeLike => &["Stop"],
        ConfigShape::Cursor => &["afterAgentResponse", "stop"],
    };
    for name in names {
        if let Some(entries) = value
            .get_mut("hooks")
            .and_then(|hooks| hooks.get_mut(*name))
            .and_then(serde_json::Value::as_array_mut)
        {
            entries.retain(|entry| !owned_hook(entry));
        }
    }
}

fn verify_owned_entry(path: &Path, pane: &str, shape: ConfigShape) -> Result<()> {
    let value = read_json_object(path)?;
    let names: &[&str] = match shape {
        ConfigShape::ClaudeLike => &["Stop"],
        ConfigShape::Cursor => &["afterAgentResponse", "stop"],
    };
    let found: usize = names
        .iter()
        .map(|event| {
            value["hooks"][event]
                .as_array()
                .into_iter()
                .flatten()
                .filter(|entry| owned_hook(entry))
                .count()
        })
        .sum();
    if found != names.len() || !value.to_string().contains(pane) {
        bail!("hook_install_failed: owned Stop entry did not verify");
    }
    Ok(())
}

/// Runs from a native CLI hook. Non-matching pane/session invocations are out
/// of scope and exit successfully without checking or publishing.
pub fn run(ctx: &Ctx, kind: &str, slug: &str, pane: &str, phase: &str) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let inherited = std::env::var("HERDR_PANE_ID").unwrap_or_default();
    if inherited != pane {
        return Ok(());
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((INPUT_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > INPUT_LIMIT {
        return correction(kind, "plain_input_too_large: reply exceeds 64 KiB");
    }
    let input: serde_json::Value =
        serde_json::from_slice(&bytes).context("hook input is not JSON")?;
    let session = input["session_id"].as_str().unwrap_or_default();
    if !scope_binding(&project, kind, pane, session)? {
        return Ok(());
    }
    // The coordinator's turn ended: talk requests it was handed are taken.
    if phase != "observe" {
        crate::talk::mark_accepted(&project)?;
    }
    if kind == "cursor" && phase == "stop" {
        return cursor_stop(ctx, &project, kind);
    }
    let text = reply_text(kind, &input).unwrap_or_default();
    let turn = if kind == "cursor" {
        project::read_json::<CursorPending>(&cursor_pending_path(&project))
            .map(|pending| pending.turn)
            .unwrap_or_else(|| turn_key(&input, text))
    } else {
        turn_key(&input, text)
    };
    let messages = match parse_envelopes(text) {
        Ok(messages) => messages,
        Err(error) => {
            if kind == "cursor" && phase == "observe" {
                return save_cursor_pending(&project, session, &turn, text, &error.to_string());
            }
            return failed_check(
                ctx,
                &project,
                kind,
                session,
                &turn,
                text,
                &error.to_string(),
            );
        }
    };
    if let Some(reason) = messages.iter().find_map(|message| {
        validate_message(&project, message)
            .err()
            .map(|error| error.to_string())
    }) {
        if kind == "cursor" && phase == "observe" {
            return save_cursor_pending(&project, session, &turn, text, &reason);
        }
        return failed_check(ctx, &project, kind, session, &turn, text, &reason);
    }
    publish(ctx, &project, session, &turn, &messages)?;
    if kind == "cursor" {
        let _ = std::fs::remove_file(cursor_pending_path(&project));
    }
    Ok(())
}

fn cursor_pending_path(project: &Project) -> PathBuf {
    project
        .state_dir()
        .join("plain")
        .join("cursor-pending.json")
}

fn save_cursor_pending(
    project: &Project,
    session: &str,
    turn: &str,
    text: &str,
    reason: &str,
) -> Result<()> {
    let path = cursor_pending_path(project);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    project::write_json(
        &path,
        &CursorPending {
            session: session.to_string(),
            turn: turn.to_string(),
            text: text.to_string(),
            reason: reason.to_string(),
        },
    )
}

fn cursor_stop(ctx: &Ctx, project: &Project, kind: &str) -> Result<()> {
    let Some(pending) = project::read_json::<CursorPending>(&cursor_pending_path(project)) else {
        return Ok(());
    };
    failed_check(
        ctx,
        project,
        kind,
        &pending.session,
        &pending.turn,
        &pending.text,
        &pending.reason,
    )
}

fn scope_binding(project: &Project, kind: &str, pane: &str, session: &str) -> Result<bool> {
    let _lock = project.lock()?;
    let Some(mut binding) = project::read_json::<Binding>(&binding_path(project)) else {
        return Ok(false);
    };
    if binding.kind != kind || binding.pane != pane || binding.project != project.slug {
        return Ok(false);
    }
    if binding.session_id.is_empty() && !session.is_empty() {
        binding.session_id = session.to_string();
        project::write_json(&binding_path(project), &binding)?;
    } else if binding.session_id != session {
        return Ok(false);
    }
    Ok(true)
}

fn reply_text<'a>(kind: &str, input: &'a serde_json::Value) -> Option<&'a str> {
    match kind {
        "claude" => input["last_assistant_message"].as_str(),
        "cursor" => input["text"]
            .as_str()
            .or_else(|| input["response"].as_str()),
        "codex" => input["last_assistant_message"]
            .as_str()
            .or_else(|| input["text"].as_str()),
        _ => None,
    }
}

fn turn_key(input: &serde_json::Value, text: &str) -> String {
    for field in ["turn_id", "last_user_message_id", "prompt_id"] {
        if let Some(value) = input[field].as_str().filter(|value| !value.is_empty()) {
            return value.to_string();
        }
    }
    // Continuations keep the same transcript path. This fallback does not
    // reset on `stop_hook_active`; a fresh native human turn should supply one
    // of the identifiers above on qualified adapters.
    input["transcript_path"]
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| format!("text-{:x}", Sha256::digest(text.as_bytes())))
}

fn failed_check(
    ctx: &Ctx,
    project: &Project,
    kind: &str,
    session: &str,
    turn: &str,
    text: &str,
    reason: &str,
) -> Result<()> {
    let mut budget = load_budget(project, session, turn)?;
    let now = jiff::Timestamp::now().as_second();
    let expired = now - budget.started_second >= DEADLINE_SECS;
    if !expired && budget.corrections < MAX_CORRECTIONS {
        budget.corrections += 1;
        save_budget(project, session, &budget)?;
        return correction(kind, reason);
    }
    if !expired
        && budget.translator_runs == 0
        && let Some(command) = translator_command(&ctx.config_dir)?
    {
        budget.translator_runs = 1;
        save_budget(project, session, &budget)?;
        if let Some(rewrite) = run_translator(ctx, &command, text, reason)?
            && let Ok(messages) = parse_envelopes(&rewrite)
            && messages
                .iter()
                .all(|message| validate_message(project, message).is_ok())
        {
            return publish(ctx, project, session, turn, &messages);
        }
    }
    publish(
        ctx,
        project,
        session,
        turn,
        &[HumanMessage::Notice {
            id: "plain_exhausted".into(),
        }],
    )
}

fn correction(kind: &str, reason: &str) -> Result<()> {
    let reason = format!("Rewrite the reply for the plain-language check: {reason}");
    let value = match kind {
        "cursor" => serde_json::json!({ "followup_message": reason }),
        "claude" | "codex" => serde_json::json!({ "decision": "block", "reason": reason }),
        _ => return Ok(()),
    };
    println!("{}", serde_json::to_string(&value)?);
    Ok(())
}

fn parse_envelopes(text: &str) -> Result<Vec<HumanMessage>> {
    let mut messages = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find("```ade-") {
        rest = &rest[start + 3..];
        let Some((header, body_and_tail)) = rest.split_once('\n') else {
            bail!("plain_envelope: envelope fence has no body");
        };
        let Some((body, tail)) = body_and_tail.split_once("```") else {
            bail!("plain_envelope: envelope fence is not closed");
        };
        let fields: BTreeMap<String, String> = body
            .lines()
            .filter_map(|line| line.split_once(':'))
            .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
            .collect();
        match header.trim() {
            "ade-say" => messages.push(HumanMessage::Say {
                what: fields.get("what").cloned().unwrap_or_default(),
                means: fields
                    .get("means")
                    .cloned()
                    .filter(|value| !value.is_empty()),
            }),
            "ade-ask" => {
                let ask = fields.get("ask").map(String::as_str).unwrap_or_default();
                let (id, revision) = ask
                    .rsplit_once('@')
                    .context("plain_envelope: ask must be <id>@<revision>")?;
                messages.push(HumanMessage::Ask {
                    id: id.to_string(),
                    revision: revision
                        .parse()
                        .context("plain_envelope: ask revision is not a number")?,
                });
            }
            _ => {}
        }
        rest = tail;
    }
    if messages.is_empty() {
        bail!("plain_envelope: end your reply with an `ade-say` block");
    }
    Ok(messages)
}

pub(crate) fn validate_message(project: &Project, message: &HumanMessage) -> Result<()> {
    let glossary = crate::glossary::registry(project);
    let result = plain::check_message(message, &glossary);
    if let Some(violation) = result.violations.first() {
        bail!("{}: {}", violation.rule.code(), violation.fix);
    }
    if let HumanMessage::Ask { id, revision } = message {
        crate::ask::open_revision(project, id, *revision, &glossary)?;
    }
    Ok(())
}

/// Hands each checked message to the one publisher, keyed by session, turn
/// and position so a repeated hook run of the same reply appends once.
fn publish(
    ctx: &Ctx,
    project: &Project,
    session: &str,
    turn: &str,
    messages: &[HumanMessage],
) -> Result<()> {
    for (n, message) in messages.iter().enumerate() {
        let key = format!("hook:{session}:{turn}:{n}");
        crate::ask::publish_keyed(ctx, project, message, Some(&key))?;
    }
    Ok(())
}

fn budget_path(project: &Project, session: &str) -> PathBuf {
    let hash = format!("{:x}", Sha256::digest(session.as_bytes()));
    project
        .state_dir()
        .join("plain")
        .join("budget")
        .join(format!("{hash}.json"))
}

fn load_budget(project: &Project, session: &str, turn: &str) -> Result<Budget> {
    let path = budget_path(project, session);
    if let Some(budget) = project::read_json::<Budget>(&path)
        && budget.turn == turn
    {
        return Ok(budget);
    }
    Ok(Budget {
        turn: turn.to_string(),
        corrections: 0,
        translator_runs: 0,
        started: project::now(),
        started_second: jiff::Timestamp::now().as_second(),
    })
}

fn save_budget(project: &Project, session: &str, budget: &Budget) -> Result<()> {
    let path = budget_path(project, session);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    project::write_json(&path, budget)
}

#[derive(Deserialize, Default)]
struct PlainConfig {
    model: Option<String>,
}

#[derive(Deserialize, Default)]
struct Config {
    #[serde(default)]
    plain: PlainConfig,
}

fn translator_command(config_dir: &Path) -> Result<Option<String>> {
    let path = config_dir.join("config.toml");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Ok(None);
    };
    let config: Config = toml::from_str(&text)?;
    Ok(config
        .plain
        .model
        .filter(|command| !command.trim().is_empty()))
}

fn run_translator(ctx: &Ctx, command: &str, text: &str, reason: &str) -> Result<Option<String>> {
    let input = format!("{reason}\n\n{text}");
    let output = ctx.runner.run(
        &Cmd::new("/bin/sh", SUBPROCESS_TIMEOUT)
            .args(["-lc", command])
            .stdin(input)
            .own_group(),
    )?;
    if output.success() {
        Ok(Some(output.stdout))
    } else {
        Ok(None)
    }
}

fn read_json_object(path: &Path) -> Result<serde_json::Value> {
    let value = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .with_context(|| format!("{} does not parse", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => serde_json::json!({}),
        Err(error) => return Err(error.into()),
    };
    if !value.is_object() {
        bail!("hook_config_invalid: {} is not an object", path.display());
    }
    Ok(value)
}

fn write_json_atomic(path: &Path, value: &serde_json::Value) -> Result<()> {
    let parent = path.parent().context("hook path has no parent")?;
    std::fs::create_dir_all(parent)?;
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    project::write_atomic(path, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::Env;
    use crate::runner::fake::FakeRunner;

    #[test]
    fn raw_question_does_not_bypass_the_typed_envelope() {
        assert!(
            parse_envelopes("Keep it running, or stop it now?")
                .unwrap_err()
                .to_string()
                .contains("ade-say")
        );
    }

    #[test]
    fn prose_question_is_private_when_say_envelope_exists() {
        let messages = parse_envelopes(
            "Keep it running, or stop it now?\n```ade-say\nwhat: The work is ready.\n```",
        )
        .unwrap();
        assert_eq!(
            messages,
            vec![HumanMessage::Say {
                what: "The work is ready.".into(),
                means: None
            }]
        );
    }

    #[test]
    fn claude_install_is_idempotent_and_preserves_unrelated_hooks() {
        let temp = tempfile::tempdir().unwrap();
        let env = Env::for_test(temp.path(), &[]);
        let runner = FakeRunner::new();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: temp.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        let path = project.dir().join(".claude/settings.local.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(
            &path,
            r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"other"}]}]}}"#,
        )
        .unwrap();
        install(&ctx, &project, "claude", "w1:p1").unwrap();
        install(&ctx, &project, "claude", "w1:p1").unwrap();
        let value = read_json_object(&path).unwrap();
        assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 2);
        remove(&project).unwrap();
        let value = read_json_object(&path).unwrap();
        assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn correction_budget_persists_across_continuations_and_exhausts_once() {
        let temp = tempfile::tempdir().unwrap();
        let env = Env::for_test(temp.path(), &[]);
        let runner = FakeRunner::new();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: temp.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        for _ in 0..5 {
            failed_check(
                &ctx,
                &project,
                "claude",
                "session-one",
                "same-turn",
                "bad",
                "plain_unknown_word: replace it",
            )
            .unwrap();
        }
        let budget = load_budget(&project, "session-one", "same-turn").unwrap();
        assert_eq!(budget.corrections, 3);
        assert_eq!(budget.translator_runs, 0);
        let lines = crate::talk::read(&project).lines;
        assert_eq!(lines.len(), 1);
        assert_eq!(
            lines[0].entry,
            crate::talk::Entry::Notice {
                id: "plain_exhausted".into()
            }
        );
    }

    #[test]
    fn cursor_and_codex_use_their_project_hook_shapes() {
        let temp = tempfile::tempdir().unwrap();
        let env = Env::for_test(temp.path(), &[]);
        let runner = FakeRunner::new();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: temp.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };

        install(&ctx, &project, "cursor", "w1:p1").unwrap();
        install(&ctx, &project, "cursor", "w1:p1").unwrap();
        let cursor = read_json_object(&project.dir().join(".cursor/hooks.json")).unwrap();
        assert_eq!(cursor["version"], 1);
        assert_eq!(
            cursor["hooks"]["afterAgentResponse"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert!(
            cursor["hooks"]["afterAgentResponse"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with("--phase observe")
        );
        assert!(
            cursor["hooks"]["stop"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with("--phase stop")
        );
        remove(&project).unwrap();

        install(&ctx, &project, "codex", "w1:p1").unwrap();
        let codex = read_json_object(&project.dir().join(".codex/hooks.json")).unwrap();
        assert_eq!(codex["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(
            codex["hooks"]["Stop"][0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .contains("--kind codex")
        );
        remove(&project).unwrap();
        let codex = read_json_object(&project.dir().join(".codex/hooks.json")).unwrap();
        assert!(codex["hooks"]["Stop"].as_array().unwrap().is_empty());
    }
}
