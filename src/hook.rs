//! Coordinator prompt hook lifecycle: preserve the provenance of Rolf's messages.

use std::io::Read as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::remote::quote;

const READ_LIMIT: usize = 4 * 1024 * 1024;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct Binding {
    kind: String,
    project: String,
    pane: String,
    #[serde(default)]
    session_id: String,
}

fn binding_path(project: &Project) -> PathBuf {
    project.state_dir().join("coordinator-hook.json")
}

fn binding_failure(path: &Path, error: impl std::fmt::Display) -> anyhow::Error {
    let detail = format!("hook_binding_unreadable: {}: {error}", path.display());
    anyhow::anyhow!(detail)
}

fn read_binding(project: &Project) -> Result<Option<Binding>> {
    let path = binding_path(project);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(binding_failure(&path, error)),
    };
    match serde_json::from_slice(&bytes) {
        Ok(binding) => Ok(Some(binding)),
        Err(error) => Err(binding_failure(&path, error)),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConfigShape {
    ClaudeLike,
    Cursor,
    Pi,
}

fn settings_path(
    project: &Project,
    adapter: &crate::adapters::Adapter,
) -> Option<(PathBuf, ConfigShape)> {
    let shape = match adapter.hook.shape.as_str() {
        "claude" => ConfigShape::ClaudeLike,
        "cursor" => ConfigShape::Cursor,
        "pi" => ConfigShape::Pi,
        _ => return None,
    };
    crate::adapters::settings_path(&project.dir(), adapter).map(|path| (path, shape))
}

pub(crate) fn install(ctx: &Ctx, project: &Project, kind: &str, pane: &str) -> Result<bool> {
    if kind.is_empty() {
        return Ok(false);
    }
    let adapter = crate::adapters::declaration(&ctx.config_dir, kind)?;
    if !adapter.coordinator {
        return Ok(false);
    }
    let Some((path, shape)) = settings_path(project, &adapter) else {
        return Ok(false);
    };
    let binary = std::env::current_exe().context("could not locate herdr-ade")?;
    let command = format!(
        "{} --root {} hook --kind {} --project {} --binding {}",
        quote(&binary.to_string_lossy()),
        quote(&ctx.root.to_string_lossy()),
        quote(kind),
        quote(&project.slug),
        quote(pane)
    );
    let _lock = project.lock()?;
    // Pi's global extension reads the binding from ADE state, not checkout hooks.
    if shape != ConfigShape::Pi {
        let mut value = read_json_object(&path)?;
        remove_entries(&mut value, &adapter);
        install_entry(&mut value, shape, &adapter, &command)?;
        write_json_atomic(&path, &value)?;
    }
    let binding = Binding {
        kind: kind.to_string(),
        project: project.slug.clone(),
        pane: pane.to_string(),
        session_id: String::new(),
    };
    project::write_json(&binding_path(project), &binding)?;
    if read_binding(project)?.as_ref() != Some(&binding) {
        bail!("hook_install_failed: hook binding did not verify");
    }
    if shape != ConfigShape::Pi {
        verify_owned_entry(&path, pane, &adapter)?;
    }
    Ok(true)
}

/// Rewrites every open project's hook settings and small binding through the
/// newly installed binary's current adapter declarations.
pub(crate) fn reinstall_open(ctx: &Ctx) -> Result<Vec<String>> {
    let mut rebound = Vec::new();
    for slug in crate::project::list_slugs(&ctx.root) {
        let project = Project::load(&ctx.root, &slug)?;
        let Some(coordinator) = project.coordinator() else {
            continue;
        };
        if coordinator.pane_id.is_empty() || coordinator.launch.kind.is_empty() {
            continue;
        }
        if !install(
            ctx,
            &project,
            &coordinator.launch.kind,
            &coordinator.pane_id,
        )? {
            bail!(
                "coordinator_hook_unsupported: open project `{slug}` uses `{}` without a prompt-submit hook",
                coordinator.launch.kind
            );
        }
        rebound.push(format!(
            "{slug} ({}, pane {})",
            coordinator.launch.kind, coordinator.pane_id
        ));
    }
    Ok(rebound)
}

pub(crate) fn remove(ctx: &Ctx, project: &Project) -> Result<()> {
    let _lock = project.lock()?;
    if let Some(binding) = read_binding(project)? {
        let adapter = crate::adapters::declaration(&ctx.config_dir, &binding.kind)?;
        if let Some((path, shape)) = settings_path(project, &adapter) {
            if shape == ConfigShape::Pi {
                if path.exists() {
                    std::fs::remove_file(&path)?;
                }
            } else if path.exists() {
                let mut value = read_json_object(&path)?;
                remove_entries(&mut value, &adapter);
                write_json_atomic(&path, &value)?;
            }
        }
    }
    let _ = std::fs::remove_file(binding_path(project));
    Ok(())
}

/// True when a prompt-submit hook is bound to this pane, so the journal
/// knows to write prompt markers for it.
pub(crate) fn captures(project: &Project, pane: &str) -> Result<bool> {
    let Some(binding) = read_binding(project)? else {
        return Ok(false);
    };
    if binding.pane != pane {
        return Ok(false);
    }
    Ok(true)
}

fn owned_hook(value: &serde_json::Value) -> bool {
    value["command"].as_str().is_some_and(|command| {
        command.contains(" hook --kind ") || command.contains(" plain hook --kind ")
    }) || value["hooks"].as_array().into_iter().flatten().any(|hook| {
        hook["command"].as_str().is_some_and(|command| {
            command.contains(" hook --kind ") || command.contains(" plain hook --kind ")
        })
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
    let event = &adapter.hook.prompt_event;
    let entries = hooks
        .entry(event)
        .or_insert_with(|| serde_json::json!([]))
        .as_array_mut()
        .with_context(|| format!("hook_config_invalid: `hooks.{event}` is not an array"))?;
    let command = format!("{command} --phase prompt");
    entries.push(match shape {
        ConfigShape::ClaudeLike => serde_json::json!({
            "matcher": "",
            "hooks": [{ "type": "command", "command": command }]
        }),
        ConfigShape::Cursor => serde_json::json!({
            "command": command,
            "loop_limit": 3
        }),
        ConfigShape::Pi => bail!("pi hooks use the ADE state binding"),
    });
    Ok(())
}

fn remove_entries(value: &mut serde_json::Value, adapter: &crate::adapters::Adapter) {
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

fn verify_owned_entry(path: &Path, pane: &str, adapter: &crate::adapters::Adapter) -> Result<()> {
    let value = read_json_object(path)?;
    let found = value["hooks"][&adapter.hook.prompt_event]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|entry| owned_hook(entry))
        .count();
    if found != 1 || !value.to_string().contains(pane) {
        bail!("hook_install_failed: owned prompt entry did not verify");
    }
    Ok(())
}

/// Runs from a native CLI hook. Non-matching pane/session invocations are out
/// of scope and exit successfully without checking the turn.
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
        return Ok(());
    }
    let input: serde_json::Value =
        serde_json::from_slice(&bytes).context("hook input is not JSON")?;
    let session = input["session_id"].as_str().unwrap_or_default();
    if !scope_binding(&project, kind, pane, session, phase)? {
        return Ok(());
    }
    if phase == "prompt" {
        let text = prompt_text(&input).unwrap_or_default();
        let request = if text.trim().is_empty() {
            None
        } else {
            handle_prompt(&project, pane, text)?
        };
        if let Some(request) = request {
            println!("request {request}");
        }
        return Ok(());
    }
    Ok(())
}

fn scope_binding(
    project: &Project,
    kind: &str,
    pane: &str,
    session: &str,
    phase: &str,
) -> Result<bool> {
    let _lock = project.lock()?;
    let Some(mut binding) = read_binding(project)? else {
        return Ok(false);
    };
    if binding.kind != kind || binding.pane != pane || binding.project != project.slug {
        return Ok(false);
    }
    if binding.session_id != session {
        // A relaunch or native session reset changes the harness session while
        // keeping ADE's pane binding. Only its next submitted prompt may claim
        // the new session; a late Stop from the old process cannot take it back.
        if phase != "prompt" || session.is_empty() {
            return Ok(false);
        }
        binding.session_id = session.to_string();
        project::write_json(&binding_path(project), &binding)?;
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
    if crate::prompt::is_task_notification_prompt(text)
        || crate::prompt::is_cross_session_prompt(text)
        || crate::prompt::is_idle_notice_prompt(text)
    {
        return Ok(None);
    }
    if crate::prompt::take_pending_prompt(project, pane, text) {
        return Ok(None);
    }
    let text = crate::prompt::take_automated_parts(project, pane, text);
    let Some(text) = crate::prompt::human_request_text(&text) else {
        return Ok(None);
    };
    Ok(Some(crate::prompt::record_pane_request(project, &text)?))
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
        assert_eq!(value["hooks"]["Stop"].as_array().unwrap().len(), 1);
        let prompt = value["hooks"]["UserPromptSubmit"].as_array().unwrap();
        assert_eq!(prompt.len(), 1, "the prompt-submit hook is installed once");
        assert!(
            prompt[0]["hooks"][0]["command"]
                .as_str()
                .unwrap()
                .ends_with("--phase prompt")
        );
        remove(&ctx, &project).unwrap();
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
    fn a_nudge_joined_to_half_typed_words_records_only_rolfs_words() {
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
        let nudge = "[herdr-ade ticker: automated, not the user, approves nothing] Continue open work: job-0001: verify 3 acceptance condition(s); job-0005: ...";
        let mixed = format!("e{nudge}");

        crate::prompt::mark_automated_prompt(&project, "w1:p1", nudge).unwrap();
        let request = handle_prompt(&project, "w1:p1", &mixed)
            .unwrap()
            .expect("the half-typed word remains Rolf's request");
        assert_eq!(
            crate::prompt::request_text(&project, &request).as_deref(),
            Some("e")
        );

        let history = project
            .record_dir_for_write("talk")
            .unwrap()
            .join("journal.jsonl");
        std::fs::write(history, format!("{}\n", serde_json::json!({"seq":1,"rolf":{"request":"q-historical-mixed-nudge","text":mixed}}))).unwrap();
        assert_eq!(
            crate::prompt::request_text(&project, "q-historical-mixed-nudge").as_deref(),
            Some("e")
        );

        let other = "[herdr-ade ticker: automated, not the user, approves nothing] New inbox items. Run context.";
        crate::prompt::mark_automated_prompt(&project, "w1:p1", nudge).unwrap();
        crate::prompt::mark_automated_prompt(&project, "w1:p1", other).unwrap();
        let request = handle_prompt(&project, "w1:p1", &format!("hal{nudge}{other}f"))
            .unwrap()
            .expect("words around two marked prompts remain");
        assert_eq!(
            crate::prompt::request_text(&project, &request).as_deref(),
            Some("half")
        );
    }

    #[test]
    fn rolf_chat_request_and_answered_ask_authorize_tasks() {
        let fx = crate::testkit::fixture();
        let project = &fx.project;
        let request = handle_prompt(project, "w1:p1", "Build the dashboard")
            .unwrap()
            .unwrap();
        crate::task::add(
            project,
            "Build dashboard",
            vec![format!("request:{request}")],
            vec!["Dashboard opens".into()],
            None,
            None,
        )
        .unwrap();
        assert_eq!(
            crate::prompt::request_text(project, &request).as_deref(),
            Some("Build the dashboard")
        );
        // Historical records are read, never created through a live ask command.
        let dir = project.record_dir_for_write("asks").unwrap().join("a-1");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("r1.toml"),
            r#"id = "a-1"
revision = 1
project = "demo"
question = "Use the new dashboard?"
choices = ["Use it", "Wait"]
asked = "2026-09-18T00:00:00Z"
coordinator_binding = "w1:p1"
"#,
        )
        .unwrap();
        std::fs::write(
            dir.join("r1.answer.toml"),
            r#"id = "a-1"
revision = 1
choice = 1
text = "Use it"
not_understood = false
answered = "2026-09-18T00:01:00Z"
by = "Rolf"
"#,
        )
        .unwrap();
        let ask = crate::ask::load_revision(project, "a-1", 1)
            .unwrap()
            .unwrap();
        assert_eq!(ask.choices, ["Use it", "Wait"]);
        assert_eq!(ask.task, None);
        assert_eq!(crate::ask::latest_revision(project, "a-1"), 1);
        crate::task::add(
            project,
            "Use dashboard",
            vec![format!("ask:{}@{}", ask.id, ask.revision)],
            vec!["Dashboard used".into()],
            None,
            None,
        )
        .unwrap();

        assert_eq!(
            crate::note::validate_basis(project, "ask:a-1@1").unwrap(),
            "ask:a-1@1"
        );
        assert!(crate::note::validate_basis(project, "ask:a-1@2").is_err());
        std::fs::write(
            dir.join("r2.toml"),
            std::fs::read_to_string(dir.join("r1.toml"))
                .unwrap()
                .replace("revision = 1", "revision = 2"),
        )
        .unwrap();
        assert!(crate::note::validate_basis(project, "ask:a-1@1").is_err());
        assert!(crate::note::validate_basis(project, "ask:a-1@2").is_err());
        std::fs::write(
            dir.join("r2.withdrawn.toml"),
            r#"id = "a-1"
revision = 2
reason = "No longer needed"
by = "Rolf"
at = "2026-09-18T00:02:00Z"
"#,
        )
        .unwrap();
        let withdrawal = crate::ask::withdrawal_of(project, "a-1", 2).unwrap();
        assert_eq!(withdrawal.reason, "No longer needed");
        assert_eq!(crate::ask::latest_revision(project, "a-1"), 2);
        assert!(crate::ask::answer_of(project, "a-1", 2).is_none());
    }

    #[test]
    fn pi_binds_in_ade_state_without_writing_checkout_commands() {
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
        let old = project.dir().join(".pi/hooks.json");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, r#"{"hooks":{"Stop":[{"hooks":[{"command":"ha plain hook --kind pi"}]}],"UserPromptSubmit":[{"hooks":[{"command":"ha plain hook --kind pi --phase prompt"}]}]}}"#).unwrap();
        install(&ctx, &project, "pi", "w1:p1").unwrap();
        assert!(old.exists(), "pi never reads or scrubs hooks.json");
        std::fs::write(&old, "not json").unwrap();
        let path = project.dir().join(".pi/herdr-ade-hooks.json");
        assert!(!path.exists(), "no pi commands are written to the checkout");
        let binding = read_binding(&project).unwrap().unwrap();
        assert_eq!(binding.kind, "pi");
        assert_eq!(binding.project, "demo");
        assert_eq!(binding.pane, "w1:p1");
        assert!(captures(&project, "w1:p1").unwrap());
        // Historical command files are ignored, not parsed or refreshed.
        let historical = r#"{"pane":"w1:p1","prompt":["hostile","command"]}"#;
        std::fs::write(&path, historical).unwrap();
        install(&ctx, &project, "pi", "w1:p1").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), historical);
        std::fs::write(&path, b"not json").unwrap();
        install(&ctx, &project, "pi", "w1:p1").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "not json");
        // A historical binding without session_id remains readable.
        std::fs::write(
            binding_path(&project),
            r#"{"kind":"pi","project":"demo","pane":"w1:p1"}"#,
        )
        .unwrap();
        assert!(scope_binding(&project, "pi", "w1:p1", "pi-session", "prompt").unwrap());
        remove(&ctx, &project).unwrap();
        assert!(!path.exists());
        assert_eq!(std::fs::read_to_string(&old).unwrap(), "not json");
        assert!(!captures(&project, "w1:p1").unwrap());
    }

    #[test]
    fn install_rebinds_open_hooks_and_binding_failures_stay_visible() {
        let temp = tempfile::tempdir().unwrap();
        let env = Env::for_test(temp.path(), &[]);
        let runner = FakeRunner::new();
        let root = temp.path().join("root");
        std::fs::create_dir(&root).unwrap();
        let project = project::create(&root, "demo", "", vec![]).unwrap();
        project
            .update_coordinator(|record| {
                record.pane_id = "w1:p1".into();
                record.launch.kind = "claude".into();
            })
            .unwrap();
        let ctx = Ctx {
            env: &env,
            root,
            config_dir: temp.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        std::fs::create_dir_all(binding_path(&project).parent().unwrap()).unwrap();
        std::fs::write(
            binding_path(&project),
            r#"{"kind":"claude","project":"demo","pane":"w1:p1","session_id":"old","adapter":{"future_field":true}}"#,
        )
        .unwrap();

        assert_eq!(reinstall_open(&ctx).unwrap(), ["demo (claude, pane w1:p1)"]);
        let rewritten = std::fs::read_to_string(binding_path(&project)).unwrap();
        assert!(!rewritten.contains("adapter"), "{rewritten}");
        assert!(scope_binding(&project, "claude", "w1:p1", "one", "prompt").unwrap());
        assert!(!scope_binding(&project, "claude", "w1:p1", "two", "stop").unwrap());
        assert!(scope_binding(&project, "claude", "w1:p1", "two", "prompt").unwrap());

        std::fs::write(binding_path(&project), b"not json").unwrap();
        let error = captures(&project, "w1:p1").unwrap_err().to_string();
        assert!(error.contains("hook_binding_unreadable"), "{error}");
    }
}
