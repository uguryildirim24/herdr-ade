//! Lane-facing verbs: durable completion, waiting, and role skills. On the box
//! the same verbs run against the lane card and seal locally; delivery and the
//! ticker stay on the Mac (SPEC-remote §4.3).

use std::path::Path;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::contracts::{FailureClass, LaneCard, OpKind, Recipient, Requested};
use crate::herdr::Herdr;
use crate::paths::Ctx;
use crate::project::{self, Project};
use crate::{ops, steps, thread, ticker};

#[derive(Debug)]
struct Binding {
    project: Project,
    thread: thread::Thread,
    /// Present when this pane is a box lane: the identity comes from the card,
    /// and there is no Mac-side delivery or ticker here.
    card: Option<LaneCard>,
}

impl Binding {
    fn recipient(&self) -> Result<Recipient> {
        if let Some(card) = &self.card {
            if card.recipient.pane.is_empty() {
                bail!("recipient_unavailable: lane card has no coordinator pane");
            }
            return Ok(card.recipient.clone());
        }
        recipient(&self.project)
    }
}

pub(crate) fn done(ctx: &Ctx, report: &str, sha: &str) -> Result<()> {
    // The path goes verbatim into the typed DONE line (D10).
    if report.is_empty() || report.chars().any(|c| c.is_control() || c.is_whitespace()) {
        return Err(crate::refusal::error(
            "report_path_invalid: a report path has no spaces or control characters",
        ));
    }
    let binding = current_lane(ctx)?;
    let recipient = binding.recipient()?;
    let attempt = binding.thread.attempt.max(1);
    // A box lane publishes its branch before `done`; the published ref is part
    // of the completion validation (SPEC-remote §4.3).
    if let Some(card) = &binding.card {
        ops::check_published_ref(
            ctx.runner,
            Path::new(&binding.thread.cwd),
            &card.branch,
            &card.publish_url,
            sha,
        )?;
    }
    let op = ops::reserve(
        &binding.project,
        ops::Reservation {
            thread: &binding.thread.id,
            attempt,
            kind: OpKind::Done,
            recipient,
            round: None,
            requested: Requested::Done {
                sha: sha.to_string(),
                report_path: report.to_string(),
            },
            helper_pid: std::process::id(),
        },
    )?;
    let git_folder = if crate::threads::managed_git_folder(&binding.project, &binding.thread) {
        &binding.thread.worktree_path
    } else {
        &binding.thread.cwd
    };
    ops::stage_done(&binding.project, &op.op, Path::new(git_folder), ctx.runner)?;
    let event = ops::seal(&binding.project, &op.op, |candidate| {
        validate_current(&binding, candidate.attempt, candidate)
    })?;
    let _ = crate::project::refresh_page(&binding.project);
    if binding.card.is_none() {
        steps::deliver_event(ctx, &binding.project, &event)?;
        ticker::start(ctx)?;
    }
    crate::output::insert("event", event.id.clone());
    println!("sealed {}", event.id);
    Ok(())
}

pub(crate) fn waiting_class(
    ctx: &Ctx,
    text: &str,
    class: FailureClass,
    provider_kind: Option<&str>,
) -> Result<()> {
    seal_message(ctx, text, false, class, provider_kind)
}

pub(crate) fn failed_class(
    ctx: &Ctx,
    text: &str,
    class: FailureClass,
    provider_kind: Option<&str>,
) -> Result<()> {
    seal_message(ctx, text, true, class, provider_kind)
}

fn seal_message(
    ctx: &Ctx,
    text: &str,
    failed: bool,
    class: FailureClass,
    provider_kind: Option<&str>,
) -> Result<()> {
    let binding = current_lane(ctx)?;
    let text = bounded_waiting(text)?;
    let provider_kind = provider_kind.map(str::trim).filter(|kind| !kind.is_empty());
    if class == FailureClass::Provider && provider_kind.is_none() {
        return Err(crate::refusal::error(
            "provider_kind_missing: a provider failure needs --provider-kind",
        ));
    }
    if class != FailureClass::Provider && provider_kind.is_some() {
        return Err(crate::refusal::error(
            "provider_kind_without_provider: --provider-kind requires --class provider",
        ));
    }
    let recipient = binding.recipient()?;
    let attempt = binding.thread.attempt.max(1);
    let op = ops::reserve(
        &binding.project,
        ops::Reservation {
            thread: &binding.thread.id,
            attempt,
            kind: if failed {
                OpKind::Failed
            } else {
                OpKind::Waiting
            },
            recipient,
            round: None,
            requested: if failed {
                Requested::Failed {
                    failure: text,
                    class,
                    provider_kind: provider_kind.map(str::to_string),
                }
            } else {
                Requested::Waiting {
                    text,
                    class,
                    provider_kind: provider_kind.map(str::to_string),
                }
            },
            helper_pid: std::process::id(),
        },
    )?;
    ops::stage_waiting(&binding.project, &op.op)?;
    let event = ops::seal(&binding.project, &op.op, |candidate| {
        validate_current(&binding, candidate.attempt, candidate)
    })?;
    let _ = crate::project::refresh_page(&binding.project);
    if binding.card.is_none() {
        // Failure is consumed by the next ticker pass, never synchronously:
        // the sealing lane gets to finish before its old tab is closed.
        if !failed {
            steps::deliver_event(ctx, &binding.project, &event)?;
        }
        ticker::start(ctx)?;
    }
    crate::output::insert("event", event.id.clone());
    crate::output::insert("failure_class", serde_json::json!(class));
    if let Some(kind) = provider_kind {
        crate::output::insert("provider_kind", serde_json::json!(kind));
    }
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
        coordinator_attempt: coordinator.attempt(),
        pane: coordinator.pane_id,
    })
}

fn validate_current(binding: &Binding, attempt: u32, op: &crate::contracts::Op) -> Result<()> {
    if binding.thread.attempt.max(1) != attempt || binding.thread.pane_id != pane_id()? {
        bail!("stale_attempt: lane binding changed before seal");
    }
    let recipient = binding.recipient()?;
    if recipient != op.recipient {
        bail!("recipient_changed: coordinator binding changed before seal");
    }
    Ok(())
}

fn current_lane(ctx: &Ctx) -> Result<Binding> {
    let pane = pane_id()?;
    let mut matches = box_lanes(ctx, &pane)?;
    if matches.is_empty() {
        matches = local_lanes(ctx, &pane)?;
    }
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => bail!("lane_binding_not_found: HERDR_PANE_ID is not a recorded lane"),
        _ => bail!("lane_binding_ambiguous: pane is recorded by more than one project"),
    }
}

/// Box lanes on this machine: the lane card under the project's hidden state.
fn box_lanes(ctx: &Ctx, pane: &str) -> Result<Vec<Binding>> {
    let mut matches = Vec::new();
    for slug in project::list_slugs(&ctx.root) {
        let project = Project::load(&ctx.root, &slug)?;
        let dir = project.record_dir("lanes");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(card) = toml::from_str::<LaneCard>(&text) else {
                continue;
            };
            if card.pane_id != pane {
                continue;
            }
            validate_card(ctx, &card)?;
            matches.push(Binding {
                project: project.clone(),
                thread: thread_from_card(&card),
                card: Some(card),
            });
        }
    }
    Ok(matches)
}

/// The local scan: a thread record whose pane is this pane.
fn local_lanes(ctx: &Ctx, pane: &str) -> Result<Vec<Binding>> {
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
            if lane.pane_id != pane || lane.is_remote() {
                continue;
            }
            let recorded = std::fs::canonicalize(&lane.cwd).ok();
            let managed = crate::threads::managed_git_folder(&project, &lane)
                .then(|| std::fs::canonicalize(&lane.worktree_path).ok())
                .flatten();
            if cwd.is_some()
                && recorded.is_some()
                && cwd != recorded
                && (managed.is_none() || cwd != managed)
            {
                continue;
            }
            matches.push(Binding {
                project: project.clone(),
                thread: lane,
                card: None,
            });
        }
    }
    Ok(matches)
}

fn thread_from_card(card: &LaneCard) -> thread::Thread {
    thread::Thread {
        id: card.thread.clone(),
        attempt: card.attempt,
        role: card.role.clone(),
        pane_id: card.pane_id.clone(),
        cwd: card.box_worktree.clone(),
        worktree_path: card.box_worktree.clone(),
        repo: card.box_repo.clone(),
        branch: card.branch.clone(),
        machine: card.machine_label.clone(),
        machine_id: card.machine_id.clone(),
        launch: crate::contracts::Launch {
            kind: card.kind.clone(),
            brief_hash: card.brief_hash.clone(),
            attempt: card.attempt,
            ..crate::contracts::Launch::default()
        },
        ..thread::Thread::default()
    }
}

/// The card is the box's authority: `HERDR_ADE_LAUNCH`, the pane and cwd must
/// match, and the pane's live process must look like the card's kind
/// (SPEC-remote §4.3).
fn validate_card(ctx: &Ctx, card: &LaneCard) -> Result<()> {
    let launch = project::LaunchEnv::from_process()
        .context("bootstrap_mismatch: HERDR_ADE_LAUNCH is missing or malformed")?;
    if launch.project != card.project
        || launch.thread != card.thread
        || launch.attempt != card.attempt
        || launch.brief_hash != card.brief_hash
    {
        bail!("bootstrap_mismatch: launch receipt does not match the lane card");
    }
    let cwd = std::env::current_dir()
        .and_then(std::fs::canonicalize)
        .context("bootstrap_mismatch: cwd cannot be resolved")?;
    if cwd.to_string_lossy() != card.box_worktree {
        bail!("bootstrap_mismatch: cwd is not the card's box worktree");
    }
    let socket = std::env::var("HERDR_SOCKET_PATH")
        .context("bootstrap_mismatch: HERDR_SOCKET_PATH is missing")?;
    if socket.is_empty() {
        bail!("bootstrap_mismatch: HERDR_SOCKET_PATH is empty");
    }
    let herdr = Herdr::new(ctx.env.herdr_bin(), &socket, ctx.runner);
    let info = herdr.pane_process_info(&card.pane_id).map_err(|error| {
        anyhow::anyhow!("bootstrap_mismatch: pane process is unavailable: {error}")
    })?;
    let kind = card.kind.as_str();
    // The agent, a shell that started it, or the plugin running `done` all
    // count; anything else is a replacement, never this attempt.
    let known = info.foreground_processes.iter().any(|p| {
        let name = p.name.as_str();
        let argv0 = p.argv0.as_deref().unwrap_or("");
        name == kind
            || name.contains(kind)
            || argv0.contains(kind)
            || matches!(name, "sh" | "bash" | "zsh" | "-zsh" | "herdr-ade" | "ha")
    });
    if !known {
        bail!("bootstrap_mismatch: pane process does not match kind `{kind}`");
    }
    Ok(())
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

/// The skill text a role is primed with. One source for the printed skill, the
/// start-time receipt and the staleness check, so they can never disagree.
pub(crate) fn skill_text(role: &str) -> &'static str {
    match role {
        "coordinator" => include_str!("../skill/COORDINATOR.md"),
        "reviewer" => include_str!("../skill/REVIEWER.md"),
        "critic" => include_str!("../skill/CRITIC.md"),
        "drafter" => include_str!("../skill/DRAFTER.md"),
        "pickup" => include_str!("../skill/PICKUP.md"),
        _ => include_str!("../skill/LANE.md"),
    }
}

/// The repository file that carries a role's skill text.
pub(crate) fn skill_file(role: &str) -> &'static str {
    match role {
        "coordinator" => "COORDINATOR.md",
        "reviewer" => "REVIEWER.md",
        "critic" => "CRITIC.md",
        "drafter" => "DRAFTER.md",
        "pickup" => "PICKUP.md",
        _ => "LANE.md",
    }
}

/// Prints the selected role skill and runtime-only rules. A lane call also
/// records the bootstrap receipt from `HERDR_ADE_LAUNCH` when its binding
/// matches. A box lane prints the fixed box prefix, never the Mac's path.
pub(crate) fn skill(ctx: &Ctx, role: &str) -> Result<()> {
    match role {
        "coordinator" => print!("{}", skill_text(role)),
        "lane" | "reviewer" | "critic" | "drafter" | "research" | "planner" => {
            let binding = current_lane(ctx)?;
            let recorded = match binding.thread.role.as_str() {
                "" => "lane",
                other => other,
            };
            if recorded != role {
                bail!(
                    "bootstrap_mismatch: this pane is a `{recorded}` thread; run `skill {recorded}`"
                );
            }
            acknowledge_bootstrap(&binding)?;
            let prefix = crate::coordinator::current_prefix(&ctx.root)?;
            print!("{}", crate::thread::commands_line(&prefix));
            print!("{}", skill_text(role));
        }
        "pickup" => print!("{}", skill_text(role)),
        _ => bail!("unknown role `{role}`"),
    }
    print_rules(&ctx.config_dir)
}

fn acknowledge_bootstrap(binding: &Binding) -> Result<()> {
    let launch = project::LaunchEnv::from_process()
        .context("bootstrap_mismatch: HERDR_ADE_LAUNCH is missing or malformed")?;
    let pane = pane_id()?;
    let lane = &binding.thread;
    if launch.project != binding.project.slug
        || launch.thread != lane.id
        || launch.attempt != lane.attempt.max(1)
        || launch.brief_hash != lane.launch.brief_hash
        || pane != lane.pane_id
    {
        bail!("bootstrap_mismatch: launch receipt does not match this lane");
    }
    let (project, thread, attempt, brief_hash) = (
        launch.project.as_str(),
        launch.thread.as_str(),
        launch.attempt,
        launch.brief_hash.as_str(),
    );
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
        // A restart is a new attempt with a new receipt; an older one is kept
        // on the record only as history, never as authority.
        if receipt.attempt >= attempt {
            bail!("bootstrap_mismatch: a different receipt is already recorded");
        }
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
    )?;
    if binding.card.is_none() {
        thread::update(&binding.project, &binding.thread.id, |t| {
            t.bootstrap = "acknowledged".into();
        })?;
    }
    Ok(())
}

fn print_rules(config_dir: &Path) -> Result<()> {
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
    fn a_box_card_builds_the_lane_identity() {
        let card = LaneCard {
            project: "demo".into(),
            thread: "t-0001".into(),
            attempt: 2,
            brief_hash: "abcd".into(),
            role: "lane".into(),
            kind: "pi".into(),
            pane_id: "w1:p2".into(),
            machine_label: "oci".into(),
            machine_id: "abc".into(),
            box_repo: "/home/ubuntu/projects/demo".into(),
            box_worktree: "/home/ubuntu/projects/demo/.worktrees/t-0001".into(),
            brief_commit: "b0b0".into(),
            branch: "hp/demo/t-0001".into(),
            publish_url: "https://github.com/uguryildirim24/demo.git".into(),
            recipient: Recipient {
                pane: "w1:p1".into(),
                coordinator_attempt: 3,
            },
            start_line: "Run the box skill".into(),
            created: "2026-09-19T00:00:00Z".into(),
        };
        let lane = thread_from_card(&card);
        assert_eq!(lane.id, "t-0001");
        assert_eq!(lane.attempt, 2);
        assert_eq!(lane.launch.brief_hash, "abcd");
        assert!(lane.is_remote());

        let root = tempfile::tempdir().unwrap();
        let project = project::create(root.path(), "demo", "", vec![]).unwrap();
        let binding = Binding {
            project,
            thread: lane,
            card: Some(card),
        };
        assert_eq!(binding.recipient().unwrap().coordinator_attempt, 3);
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
