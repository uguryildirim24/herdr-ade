//! Coordinator correction-hook lifecycle, budget, and typed envelope parsing.

use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::contracts::HumanMessage;
use crate::paths::Ctx;
use crate::plain;
use crate::project::{self, Project};
use crate::remote::quote;

const INPUT_LIMIT: usize = 64 * 1024;
/// The hook input read to find the session; the reply inside it is bounded
/// by `INPUT_LIMIT`.
const READ_LIMIT: usize = 4 * 1024 * 1024;
/// One block per turn: a second failed check publishes the fixed notice
/// instead of asking for another rewrite, so a reply is never lost to a loop.
const MAX_CORRECTIONS: u32 = 1;
const DEADLINE_SECS: i64 = 10 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Binding {
    kind: String,
    project: String,
    pane: String,
    #[serde(default)]
    session_id: String,
    /// The declaration used at install time, so removal and capture checks do
    /// not depend on mutable global configuration.
    #[serde(default)]
    adapter: crate::adapters::Adapter,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Budget {
    turn: String,
    corrections: u32,
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

fn settings_path(
    project: &Project,
    adapter: &crate::adapters::Adapter,
) -> Option<(PathBuf, ConfigShape)> {
    let shape = match adapter.hook.shape.as_str() {
        "claude" => ConfigShape::ClaudeLike,
        "cursor" => ConfigShape::Cursor,
        _ => return None,
    };
    crate::adapters::settings_path(&project.dir(), adapter).map(|path| (path, shape))
}

pub(crate) fn install(ctx: &Ctx, project: &Project, kind: &str, pane: &str) -> Result<bool> {
    if kind.is_empty() {
        return Ok(false);
    }
    let adapter = crate::adapters::declaration(&ctx.config_dir, kind)?;
    if !adapter.coordinator || !adapter.talk {
        return Ok(false);
    }
    let Some((path, shape)) = settings_path(project, &adapter) else {
        return Ok(false);
    };
    let binary = std::env::current_exe().context("could not locate herdr-ade")?;
    let command = format!(
        "{} --root {} plain hook {} --kind {} --binding {}",
        quote(&binary.to_string_lossy()),
        quote(&ctx.root.to_string_lossy()),
        quote(&project.slug),
        quote(kind),
        quote(pane)
    );
    let _lock = project.lock()?;
    let mut value = read_json_object(&path)?;
    install_entry(&mut value, shape, &adapter, &command)?;
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
            adapter: adapter.clone(),
        },
    )?;
    verify_owned_entry(&path, pane, shape, &adapter)?;
    Ok(true)
}

pub(crate) fn remove(project: &Project) -> Result<()> {
    let _lock = project.lock()?;
    let binding: Option<Binding> = project::read_json(&binding_path(project));
    if let Some(binding) = binding
        && let Some((path, shape)) = settings_path(project, &binding.adapter)
        && path.exists()
    {
        let mut value = read_json_object(&path)?;
        remove_entries(&mut value, shape, &binding.adapter);
        write_json_atomic(&path, &value)?;
    }
    let _ = std::fs::remove_file(binding_path(project));
    Ok(())
}

/// True when a prompt-submit hook is bound to this pane, so the talk layer
/// knows to write prompt markers for it.
pub(crate) fn captures(project: &Project, pane: &str) -> bool {
    let Some(binding) = project::read_json::<Binding>(&binding_path(project)) else {
        return false;
    };
    if binding.pane != pane {
        return false;
    }
    settings_path(project, &binding.adapter).is_some_and(|_| {
        !binding.adapter.hook.prompt_event.is_empty()
            && binding
                .adapter
                .hook
                .events
                .contains(&binding.adapter.hook.prompt_event)
    })
}

fn event_phase<'a>(adapter: &'a crate::adapters::Adapter, event: &str) -> Option<&'a str> {
    (event == adapter.hook.prompt_event).then_some("prompt")
}

fn owned_hook(value: &serde_json::Value) -> bool {
    value["command"]
        .as_str()
        .is_some_and(|command| command.contains(" plain hook "))
        || value["hooks"].as_array().into_iter().flatten().any(|hook| {
            hook["command"]
                .as_str()
                .is_some_and(|command| command.contains(" plain hook "))
        })
}

fn install_entry(
    value: &mut serde_json::Value,
    shape: ConfigShape,
    adapter: &crate::adapters::Adapter,
    command: &str,
) -> Result<()> {
    let object = value
        .as_object_mut()
        .context("hook_install_failed: the settings file is not a JSON object")?;
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
            for event in &adapter.hook.events {
                let entries = hooks
                    .entry(event.clone())
                    .or_insert_with(|| serde_json::json!([]))
                    .as_array_mut()
                    .with_context(|| {
                        format!("hook_config_invalid: `hooks.{event}` is not an array")
                    })?;
                entries.retain(|entry| !owned_hook(entry));
                let command = match event_phase(adapter, event) {
                    Some(phase) => format!("{command} --phase {phase}"),
                    None => command.to_string(),
                };
                entries.push(serde_json::json!({
                    "matcher": "",
                    "hooks": [{ "type": "command", "command": command }]
                }));
            }
        }
        ConfigShape::Cursor => {
            for event in &adapter.hook.events {
                let phase = if event == &adapter.hook.prompt_event {
                    "prompt"
                } else if event == "stop" {
                    "stop"
                } else {
                    "observe"
                };
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

fn remove_entries(
    value: &mut serde_json::Value,
    _shape: ConfigShape,
    adapter: &crate::adapters::Adapter,
) {
    for name in &adapter.hook.events {
        if let Some(entries) = value
            .get_mut("hooks")
            .and_then(|hooks| hooks.get_mut(name))
            .and_then(serde_json::Value::as_array_mut)
        {
            entries.retain(|entry| !owned_hook(entry));
        }
    }
}

fn verify_owned_entry(
    path: &Path,
    pane: &str,
    _shape: ConfigShape,
    adapter: &crate::adapters::Adapter,
) -> Result<()> {
    let value = read_json_object(path)?;
    let names = &adapter.hook.events;
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
pub(crate) fn run(ctx: &Ctx, kind: &str, slug: &str, pane: &str, phase: &str) -> Result<()> {
    let project = Project::load(&ctx.root, slug)?;
    let inherited = std::env::var("HERDR_PANE_ID").unwrap_or_default();
    if inherited != pane {
        return Ok(());
    }
    let mut bytes = Vec::new();
    std::io::stdin()
        .take((READ_LIMIT + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > READ_LIMIT {
        // Too large to read the session from: not blocked, and not checked.
        return Ok(());
    }
    let input: serde_json::Value =
        serde_json::from_slice(&bytes).context("hook input is not JSON")?;
    let session = input["session_id"].as_str().unwrap_or_default();
    if !scope_binding(&project, kind, pane, session)? {
        return Ok(());
    }
    // A prompt Rolf typed straight into the pane: record it under a request id
    // the coordinator can cite. A talk delivery or a harness line is not his
    // words and is not recorded again.
    if phase == "prompt" {
        let text = prompt_text(&input).unwrap_or_default();
        if !text.trim().is_empty()
            && let Some(request) = handle_prompt(&project, pane, text)?
        {
            println!("request {request}");
        }
        return Ok(());
    }
    // The coordinator's turn ended: talk requests it was handed are taken.
    if phase != "observe" {
        crate::talk::mark_accepted(&project)?;
    }
    let adapter = crate::adapters::declaration(&ctx.config_dir, kind)?;
    let cursor_shape = adapter.hook.shape == "cursor";
    if cursor_shape && phase == "stop" {
        return cursor_stop(ctx, &project, kind);
    }
    let text = crate::adapters::reply_text(&adapter, &input).unwrap_or_default();
    // An oversize reply is a failed check like any other: it spends the
    // turn's budget and ends in the fixed notice, never a block loop.
    let (text, oversize) = if text.len() > INPUT_LIMIT {
        ("", true)
    } else {
        (text, false)
    };
    let turn = if cursor_shape {
        project::read_json::<CursorPending>(&cursor_pending_path(&project))
            .map(|pending| pending.turn)
            .map_or_else(|| turn_key(&project, session, &input, text), Ok)?
    } else {
        turn_key(&project, session, &input, text)?
    };
    if oversize {
        return failed_check(
            ctx,
            &project,
            kind,
            session,
            &turn,
            "plain_input_too_large: reply exceeds 64 KiB",
        );
    }
    let messages = match parse_envelopes(text) {
        Ok(messages) => messages,
        Err(error) => {
            if cursor_shape && phase == "observe" {
                return save_cursor_pending(&project, session, &turn, text, &error.to_string());
            }
            return failed_check(ctx, &project, kind, session, &turn, &error.to_string());
        }
    };
    if let Some(reason) = messages.iter().find_map(|message| {
        validate_message(&project, message)
            .err()
            .map(|error| error.to_string())
    }) {
        if cursor_shape && phase == "observe" {
            return save_cursor_pending(&project, session, &turn, text, &reason);
        }
        return failed_check(ctx, &project, kind, session, &turn, &reason);
    }
    publish(ctx, &project, session, &turn, &messages)?;
    if cursor_shape {
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

/// The submitted user text from a prompt-submit hook.
fn prompt_text(input: &serde_json::Value) -> Option<&str> {
    input["prompt"].as_str()
}

/// Records a prompt typed into the coordinator pane and returns the request id
/// to print, or `None` when a harness line must not be recorded as Rolf's.
fn handle_prompt(project: &Project, pane: &str, text: &str) -> Result<Option<String>> {
    if crate::talk::is_task_notification_prompt(text) {
        return Ok(None);
    }
    // Herdr, rather than the plugin, sends parent BLOCKED/GONE lines. Mark
    // those full-agent-name prompts through the same exact-text path used by
    // DONE/WAITING before the hook classifies this submission.
    crate::talk::mark_parent_status_prompt(project, pane, text)?;
    match crate::talk::take_pending_prompt(project, pane, text) {
        Some(crate::talk::PendingPrompt::Delivery(request)) => Ok(Some(request)),
        Some(crate::talk::PendingPrompt::Automated) => Ok(None),
        None => Ok(Some(crate::talk::record_pane_request(project, text)?)),
    }
}

/// The human turn this stop belongs to. A native id wins. Claude sends none:
/// its first stop of a turn has `stop_hook_active = false` and starts a new
/// turn (a fresh budget and fresh publication keys); a continuation has it
/// `true` and keeps the turn it continues.
fn turn_key(
    project: &Project,
    session: &str,
    input: &serde_json::Value,
    text: &str,
) -> Result<String> {
    for field in ["turn_id", "last_user_message_id", "prompt_id"] {
        if let Some(value) = input[field].as_str().filter(|value| !value.is_empty()) {
            return Ok(value.to_string());
        }
    }
    // Without the flag (Cursor) the transcript is the turn, as before.
    let Some(continuing) = input["stop_hook_active"].as_bool() else {
        return Ok(input["transcript_path"]
            .as_str()
            .filter(|value| !value.is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| format!("text-{:x}", Sha256::digest(text.as_bytes()))));
    };
    let path = project
        .state_dir()
        .join("plain")
        .join("turn")
        .join(format!("{:x}", Sha256::digest(session.as_bytes())));
    if continuing && let Ok(turn) = std::fs::read_to_string(&path) {
        return Ok(turn);
    }
    let turn = format!(
        "turn-{:x}",
        Sha256::digest(
            format!(
                "{}\n{}\n{text}",
                input["transcript_path"].as_str().unwrap_or_default(),
                jiff::Timestamp::now().as_nanosecond()
            )
            .as_bytes()
        )
    );
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    project::write_atomic(&path, turn.as_bytes())?;
    Ok(turn)
}

fn failed_check(
    ctx: &Ctx,
    project: &Project,
    kind: &str,
    session: &str,
    turn: &str,
    reason: &str,
) -> Result<()> {
    let mut budget = load_budget(project, session, turn)?;
    let now = jiff::Timestamp::now().as_second();
    let expired = now - budget.started_second >= DEADLINE_SECS;
    if !expired && budget.corrections < MAX_CORRECTIONS {
        budget.corrections += 1;
        save_budget(project, session, &budget)?;
        return correction(ctx, kind, reason);
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

fn correction(ctx: &Ctx, kind: &str, reason: &str) -> Result<()> {
    let reason = format!("Rewrite the reply for the plain-language check: {reason}");
    let adapter = crate::adapters::declaration(&ctx.config_dir, kind)?;
    if let Some(value) = crate::adapters::correction(&adapter, &reason) {
        println!("{}", serde_json::to_string(&value)?);
    }
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
                landed_round: None,
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
                means: None,
                landed_round: None
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
        let prompt = value["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(prompt.len(), 1, "the prompt-submit hook is installed once");
        assert!(
            prompt[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with("--phase prompt")
        );
        remove(&project).unwrap();
        let value = read_json_object(&path).unwrap();
        assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 1);
        assert!(
            value["hooks"]["UserPromptSubmit"]
                .as_array()
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_pane_prompt_gets_a_request_id_but_a_harness_line_does_not() {
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
        install(&ctx, &project, "claude", "w1:p1").unwrap();

        crate::talk::mark_automated_prompt(&project, "w1:p1", "DONE t-0001 report.md sha").unwrap();
        let wrapped_done = "\n\n<pasted_content id=\"2459\">\nDONE t-0001 report.md sha\n</pasted_content id=\"2459\">\n";
        assert_eq!(
            handle_prompt(&project, "w1:p1", wrapped_done).unwrap(),
            None
        );
        assert!(crate::talk::recent_requests(&project, 5).is_empty());

        let task_notice = "<task-notification>\n<task-id>abc</task-id>\n<tool-use-id>tool</tool-use-id>\n<output-file>/tmp/task</output-file>\n<status>completed</status>\n<summary>done</summary>\n</task-notification>";
        assert_eq!(handle_prompt(&project, "w1:p1", task_notice).unwrap(), None);
        assert!(crate::talk::recent_requests(&project, 5).is_empty());

        assert_eq!(
            handle_prompt(&project, "w1:p1", "GONE hp-demo-t-0162").unwrap(),
            None
        );
        assert_eq!(
            handle_prompt(&project, "w1:p1", "BLOCKED hp-demo-t-0162").unwrap(),
            None
        );
        assert!(crate::talk::recent_requests(&project, 5).is_empty());

        let id = handle_prompt(&project, "w1:p1", "Spend five dollars on the check.")
            .unwrap()
            .unwrap();
        assert!(id.starts_with("q-"), "{id}");
        assert_eq!(
            crate::talk::recent_requests(&project, 5),
            vec![(id.clone(), "Spend five dollars on the check.".to_string())]
        );
        let record = crate::decide::decide(
            &ctx,
            "demo",
            crate::decide::NewDecision {
                line: "I will spend five dollars on the check.",
                class: "money",
                key: None,
                basis: Some(&format!("request:{id}")),
                replaces: None,
                request: None,
            },
        )
        .unwrap();
        assert_eq!(
            record.basis.as_deref(),
            Some(format!("request:{id}").as_str())
        );

        crate::talk::mark_automated_prompt(&project, "w1:p1", "GONE t-0002").unwrap();
        let mixed = "Keep working.\n<pasted_content id=\"2460\">\nGONE t-0002\n</pasted_content id=\"2460\">";
        let mixed_id = handle_prompt(&project, "w1:p1", mixed)
            .unwrap()
            .expect("native text mixed with a pasted harness line is Rolf's request");
        assert!(crate::talk::recent_requests(&project, 5).contains(&(mixed_id, mixed.to_string())));
    }

    #[test]
    fn a_talk_delivery_reuses_the_request_the_talk_tab_recorded() {
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
        install(&ctx, &project, "claude", "w1:p1").unwrap();
        crate::talk::mark_talk_delivery(&project, "w1:p1", "q-42-7", "please look at the tests")
            .unwrap();
        assert_eq!(
            handle_prompt(&project, "w1:p1", "please look at the tests").unwrap(),
            Some("q-42-7".into())
        );
        assert!(crate::talk::recent_requests(&project, 5).is_empty());
    }

    /// Review defect: Claude's key was the transcript path, so the budget of
    /// the first failing turn never reset for the rest of the session.
    #[test]
    fn a_new_human_turn_gets_a_new_key_and_a_continuation_keeps_it() {
        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let stop =
            |active| serde_json::json!({"transcript_path": "/t.jsonl", "stop_hook_active": active});
        let first = turn_key(&project, "s", &stop(false), "a").unwrap();
        assert_eq!(turn_key(&project, "s", &stop(true), "b").unwrap(), first);
        assert_ne!(turn_key(&project, "s", &stop(false), "a").unwrap(), first);
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
                "plain_unknown_word: replace it",
            )
            .unwrap();
        }
        let budget = load_budget(&project, "session-one", "same-turn").unwrap();
        assert_eq!(budget.corrections, 1);
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
