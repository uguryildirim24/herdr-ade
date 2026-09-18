//! Lane-facing verbs: durable completion, waiting, and role skills.

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::{OpKind, Recipient, Requested};
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::{ops, steps, thread, ticker};

#[derive(Debug)]
struct Binding {
    project: Project,
    thread: thread::Thread,
}

pub fn done(ctx: &Ctx, report: &str, sha: &str) -> Result<()> {
    let binding = current_lane(ctx)?;
    let recipient = recipient(&binding.project)?;
    let attempt = binding.thread.launch_attempts.max(1);
    let op = ops::reserve(
        &binding.project,
        &binding.thread.id,
        attempt,
        OpKind::Done,
        recipient,
        None,
        Requested::Done {
            sha: sha.to_string(),
            report_path: report.to_string(),
        },
        std::process::id(),
    )?;
    let cwd = Path::new(&binding.thread.cwd);
    ops::stage_done(&binding.project, &op.op, cwd, ctx.runner)?;
    let event = ops::seal(&binding.project, &op.op, |candidate| {
        validate_current(
            &binding.project,
            &binding.thread.id,
            candidate.attempt,
            candidate,
        )
    })?;
    steps::deliver_event(ctx, &binding.project, &event)?;
    ticker::start(ctx)?;
    println!("sealed {}", event.id);
    Ok(())
}

pub fn waiting(ctx: &Ctx, text: &str) -> Result<()> {
    let binding = current_lane(ctx)?;
    let text = bounded_waiting(text)?;
    let recipient = recipient(&binding.project)?;
    let attempt = binding.thread.launch_attempts.max(1);
    let op = ops::reserve(
        &binding.project,
        &binding.thread.id,
        attempt,
        OpKind::Waiting,
        recipient,
        None,
        Requested::Waiting { text },
        std::process::id(),
    )?;
    ops::stage_waiting(&binding.project, &op.op)?;
    let event = ops::seal(&binding.project, &op.op, |candidate| {
        validate_current(
            &binding.project,
            &binding.thread.id,
            candidate.attempt,
            candidate,
        )
    })?;
    steps::deliver_event(ctx, &binding.project, &event)?;
    ticker::start(ctx)?;
    println!("sealed {}", event.id);
    Ok(())
}

fn recipient(project: &Project) -> Result<Recipient> {
    let coordinator = project
        .coordinator()
        .context("recipient_unavailable: project has no coordinator binding")?;
    if coordinator.pane_id.is_empty() {
        bail!("recipient_unavailable: coordinator pane is empty");
    }
    Ok(Recipient {
        pane: coordinator.pane_id,
        coordinator_attempt: coordinator.launch_attempts.max(1),
    })
}

fn validate_current(
    project: &Project,
    id: &str,
    attempt: u32,
    op: &crate::contracts::Op,
) -> Result<()> {
    let current = thread::load(project, id)?;
    if current.launch_attempts.max(1) != attempt || current.pane_id != pane_id()? {
        bail!("stale_attempt: lane binding changed before seal");
    }
    let recipient = recipient(project)?;
    if recipient != op.recipient {
        bail!("recipient_changed: coordinator binding changed before seal");
    }
    Ok(())
}

fn current_lane(ctx: &Ctx) -> Result<Binding> {
    let pane = pane_id()?;
    let socket = std::env::var("HERDR_SOCKET_PATH").unwrap_or_default();
    let cwd = std::env::current_dir()
        .ok()
        .and_then(|path| std::fs::canonicalize(path).ok());
    let mut matches = Vec::new();
    for slug in project::list_slugs(&ctx.root) {
        let Ok(project) = Project::load(&ctx.root, &slug) else {
            continue;
        };
        if !socket.is_empty()
            && project
                .coordinator()
                .is_some_and(|record| !record.socket.is_empty() && record.socket != socket)
        {
            continue;
        }
        for lane in thread::list(&project) {
            if lane.pane_id != pane {
                continue;
            }
            if lane.is_remote() {
                bail!("remote_not_admissible: completion is local only");
            }
            let recorded = std::fs::canonicalize(&lane.cwd).ok();
            if cwd.is_some() && recorded.is_some() && cwd != recorded {
                continue;
            }
            matches.push(Binding {
                project: project.clone(),
                thread: lane,
            });
        }
    }
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => bail!("lane_binding_not_found: HERDR_PANE_ID is not a recorded local lane"),
        _ => bail!("lane_binding_ambiguous: pane is recorded by more than one project"),
    }
}

fn pane_id() -> Result<String> {
    std::env::var("HERDR_PANE_ID")
        .ok()
        .filter(|value| !value.is_empty())
        .context("HERDR_PANE_ID is not set")
}

fn bounded_waiting(text: &str) -> Result<String> {
    let text: String = text
        .chars()
        .filter(|character| !character.is_control())
        .collect::<String>()
        .trim()
        .to_string();
    if text.is_empty() {
        bail!("waiting_empty: explain what is missing");
    }
    if text.chars().count() > 500 {
        bail!("waiting_too_long: waiting text is limited to 500 characters");
    }
    Ok(text)
}

#[derive(Debug, Serialize, Deserialize)]
struct BootstrapReceipt {
    project: String,
    thread: String,
    attempt: u32,
    brief_hash: String,
    pane: String,
    acknowledged: String,
}

/// Prints the selected role skill and runtime-only rules. A lane call also
/// records the bootstrap receipt from `HERDR_ADE_LAUNCH` when its binding
/// matches. A1 can project the same receipt onto its extended thread record.
pub fn skill(ctx: &Ctx, role: &str) -> Result<()> {
    match role {
        "coordinator" => print!("{}", include_str!("../skill/COORDINATOR.md")),
        "lane" => {
            let binding = current_lane(ctx)?;
            acknowledge_bootstrap(&binding)?;
            print!("{}", include_str!("../skill/LANE.md"));
        }
        "reviewer" | "critic" | "drafter" | "pickup" => {
            bail!("skill_unavailable: `{role}` is supplied by ade-rounds")
        }
        _ => bail!("unknown role `{role}`"),
    }
    print_rules(&ctx.config_dir)
}

fn acknowledge_bootstrap(binding: &Binding) -> Result<()> {
    let raw = std::env::var("HERDR_ADE_LAUNCH")
        .context("bootstrap_mismatch: launch receipt is missing")?;
    let value: serde_json::Value =
        serde_json::from_str(&raw).context("bootstrap_mismatch: HERDR_ADE_LAUNCH is not JSON")?;
    let project = value["project"].as_str().unwrap_or_default();
    let thread = value["thread"].as_str().unwrap_or_default();
    let attempt = value["attempt"].as_u64().unwrap_or_default() as u32;
    let brief_hash = value["brief_hash"].as_str().unwrap_or_default();
    let pane = pane_id()?;
    if project != binding.project.slug
        || thread != binding.thread.id
        || attempt != binding.thread.launch_attempts.max(1)
        || brief_hash.is_empty()
        || pane != binding.thread.pane_id
    {
        bail!("bootstrap_mismatch: launch receipt does not match this lane");
    }
    let dir = binding.project.state_dir().join("bootstrap");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", binding.thread.id));
    if let Some(receipt) = project::read_json::<BootstrapReceipt>(&path) {
        if receipt.project == project
            && receipt.thread == thread
            && receipt.attempt == attempt
            && receipt.brief_hash == brief_hash
            && receipt.pane == pane
        {
            eprintln!("bootstrap already accepted");
            return Ok(());
        }
        bail!("bootstrap_mismatch: a different receipt is already recorded");
    }
    project::write_json(
        &path,
        &BootstrapReceipt {
            project: project.to_string(),
            thread: thread.to_string(),
            attempt,
            brief_hash: brief_hash.to_string(),
            pane,
            acknowledged: project::now(),
        },
    )
}

pub fn print_rules(config_dir: &Path) -> Result<()> {
    let path = config_dir.join("RULES.md");
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(());
    };
    const RULES_MAX: usize = 64 * 1024;
    if bytes.len() > RULES_MAX {
        bail!("rules_too_large: {} exceeds 64 KiB", path.display());
    }
    let text = String::from_utf8(bytes).context("RULES.md is not UTF-8")?;
    if !text.trim().is_empty() {
        println!("\n# Standing rules\n\n{}", text.trim_end());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waiting_text_is_bounded_and_drops_controls() {
        assert_eq!(bounded_waiting("  need\nhelp\0 ").unwrap(), "needhelp");
        assert!(bounded_waiting("\n\0").is_err());
        assert!(bounded_waiting(&"x".repeat(501)).is_err());
    }

    #[test]
    fn private_rules_are_bounded() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("RULES.md"), vec![b'x'; 65 * 1024]).unwrap();
        assert!(
            print_rules(dir.path())
                .unwrap_err()
                .to_string()
                .contains("rules_too_large")
        );
    }
}
