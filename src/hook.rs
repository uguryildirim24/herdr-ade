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

fn binding_failure(project: &Project, path: &Path, error: impl std::fmt::Display) -> anyhow::Error {
    let detail = format!("hook_binding_unreadable: {}: {error}", path.display());
    crate::ledger::observe(
        project,
        "hook-binding-unreadable",
        &path.to_string_lossy(),
        &detail,
    );
    anyhow::anyhow!(detail)
}

fn read_binding(project: &Project) -> Result<Option<Binding>> {
    let path = binding_path(project);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(binding_failure(project, &path, error)),
    };
    match serde_json::from_slice(&bytes) {
        Ok(binding) => {
            crate::ledger::recovered(project, "hook-binding-unreadable", &path.to_string_lossy());
            Ok(Some(binding))
        }
        Err(error) => Err(binding_failure(project, &path, error)),
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
    if shape == ConfigShape::Pi {
        // pi does not load hooks.json. Remove our old inert entries while
        // leaving unrelated project settings alone.
        remove_old_pi_hooks(project, &adapter)?;
        let argv = vec![
            binary.to_string_lossy().to_string(),
            "--root".into(),
            ctx.root.to_string_lossy().to_string(),
            "hook".into(),
            "--kind".into(),
            kind.into(),
            "--project".into(),
            project.slug.clone(),
            "--binding".into(),
            pane.into(),
        ];
        let mut prompt = argv.clone();
        prompt.extend(["--phase".into(), "prompt".into()]);
        write_json_atomic(
            &path,
            &serde_json::json!({
                "pane": pane,
                "prompt": prompt,
            }),
        )?;
    } else {
        let mut value = read_json_object(&path)?;
        remove_entries(&mut value, shape, &adapter);
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
    verify_owned_entry(&path, pane, shape, &adapter)?;
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
                remove_old_pi_hooks(project, &adapter)?;
            } else if path.exists() {
                let mut value = read_json_object(&path)?;
                remove_entries(&mut value, shape, &adapter);
                write_json_atomic(&path, &value)?;
            }
        }
    }
    let _ = std::fs::remove_file(binding_path(project));
    Ok(())
}

fn remove_old_pi_hooks(project: &Project, adapter: &crate::adapters::Adapter) -> Result<()> {
    let old = project.dir().join(".pi/hooks.json");
    if old.exists() {
        let mut value = read_json_object(&old)?;
        remove_entries(&mut value, ConfigShape::ClaudeLike, adapter);
        if value["hooks"].as_object().is_some_and(|hooks| {
            value.as_object().is_some_and(|root| root.len() == 1)
                && hooks
                    .values()
                    .all(|entries| entries.as_array().is_some_and(Vec::is_empty))
        }) {
            std::fs::remove_file(old)?;
        } else {
            write_json_atomic(&old, &value)?;
        }
    }
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

fn event_phase<'a>(adapter: &'a crate::adapters::Adapter, event: &str) -> Option<&'a str> {
    (event == adapter.hook.prompt_event).then_some("prompt")
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
    match shape {
        ConfigShape::ClaudeLike => {
            for event in adapter
                .hook
                .events
                .iter()
                .filter(|event| *event == &adapter.hook.prompt_event)
            {
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
            for event in adapter
                .hook
                .events
                .iter()
                .filter(|event| *event == &adapter.hook.prompt_event)
            {
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
        ConfigShape::Pi => bail!("pi hooks are written as extension commands"),
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
    shape: ConfigShape,
    adapter: &crate::adapters::Adapter,
) -> Result<()> {
    let value = read_json_object(path)?;
    if shape == ConfigShape::Pi {
        if value["pane"] != pane
            || !["prompt"].iter().all(|key| {
                value[key].as_array().is_some_and(|args| {
                    args.iter().any(|arg| arg == pane) && args.iter().any(|arg| arg == "hook")
                })
            })
        {
            bail!("hook_install_failed: pi extension commands did not verify");
        }
        return Ok(());
    }
    let names = std::slice::from_ref(&adapter.hook.prompt_event);
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
    match crate::prompt::take_pending_prompt(project, pane, text) {
        Some(crate::prompt::PendingPrompt::Automated) => Ok(None),
        None => {
            let text = crate::prompt::take_automated_parts(project, pane, text);
            let Some(text) = crate::prompt::human_request_text(&text) else {
                return Ok(None);
            };
            Ok(Some(crate::prompt::record_pane_request(project, &text)?))
        }
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
        let parent = crate::task::add(
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
        let ask = crate::ask::ask(
            &fx.world.ctx(),
            "demo",
            crate::ask::NewAsk {
                question: "Use the new dashboard?".into(),
                choices: vec!["Use it".into(), "Wait".into()],
                what: None,
                means: None,
                task: Some(parent.id),
            },
        )
        .unwrap();
        crate::ask::answer(&fx.world.ctx(), "demo", &ask.id, ask.revision, 1, "Rolf").unwrap();
        crate::task::add(
            project,
            "Use dashboard",
            vec![format!("ask:{}@{}", ask.id, ask.revision)],
            vec!["Dashboard used".into()],
            None,
            None,
        )
        .unwrap();

        let task = crate::task::add(
            project,
            "Another task",
            vec![format!("request:{request}")],
            vec!["Answer only while open".into()],
            None,
            None,
        )
        .unwrap();
        let linked = crate::ask::ask(
            &fx.world.ctx(),
            "demo",
            crate::ask::NewAsk {
                question: "Use the alternate dashboard?".into(),
                choices: vec!["Yes".into(), "No".into()],
                what: None,
                means: None,
                task: Some(task.id.clone()),
            },
        )
        .unwrap();
        assert!(
            crate::ask::open_asks(project)
                .iter()
                .any(|row| row.id == linked.id)
        );
        crate::task::drop_task(project, &task.id, "No longer needed").unwrap();
        assert!(
            !crate::ask::open_asks(project)
                .iter()
                .any(|row| row.id == linked.id)
        );
        assert!(crate::ask::answer(&fx.world.ctx(), "demo", &linked.id, 1, 1, "Rolf").is_err());
    }

    #[test]
    fn pi_installs_executable_extension_commands_and_removes_inert_hooks() {
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
        assert!(!old.exists(), "pi never reads hooks.json");
        let path = project.dir().join(".pi/herdr-ade-hooks.json");
        let value = read_json_object(&path).unwrap();
        assert_eq!(value["pane"], "w1:p1");
        assert_eq!(
            value["prompt"].as_array().unwrap().last().unwrap(),
            "prompt"
        );
        assert!(value.get("stop").is_none());
        assert!(value.get("activate").is_none());
        assert!(captures(&project, "w1:p1").unwrap());
        install(&ctx, &project, "pi", "w1:p1").unwrap();
        remove(&ctx, &project).unwrap();
        assert!(!path.exists());
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
        let digest = crate::coordinator::digest(&ctx, &project, "ha").unwrap().0;
        assert!(digest.contains("hook-binding-unreadable"), "{digest}");
        assert!(digest.contains("coordinator-hook.json"), "{digest}");
    }
}
