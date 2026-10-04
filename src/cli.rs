use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};

use crate::coordinator::{self, OpenOptions};
use crate::paths::{self, Ctx, Env, SessionFlags};
use crate::project::{self, Project, Status};
use crate::runner::RealRunner;
use crate::threads::{self, ResolveArgs, StartArgs};
use crate::{actions, adopt, doctor, inbox, lifecycle, overview, ticker};

#[derive(Parser)]
#[command(name = "herdr-ade", version = crate::VERSION, about = "Projects for herdr")]
struct Cli {
    /// Projects root (default: $HERDR_ADE_ROOT, then config.toml, then ~/.herdr-ade)
    #[arg(long, global = true, value_name = "DIR")]
    root: Option<PathBuf>,

    /// Return one structured result instead of the human sentence
    #[arg(long, global = true)]
    json: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Args, Clone, Default)]
pub struct SessionArgs {
    /// herdr session name
    #[arg(long, value_name = "NAME", conflicts_with = "socket")]
    session: Option<String>,
    /// herdr socket path
    #[arg(long, value_name = "PATH")]
    socket: Option<PathBuf>,
}

impl From<SessionArgs> for SessionFlags {
    fn from(args: SessionArgs) -> Self {
        SessionFlags {
            session: args.session,
            socket: args.socket,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    #[command(hide = true)]
    InstallCheck,
    #[command(hide = true)]
    InstallRundownCheck,
    #[command(hide = true)]
    InstallJourneyStart {
        #[arg(long)]
        review: Option<String>,
    },
    /// Create a project folder with its skeleton files
    New {
        name: String,
        #[arg(long, default_value = "")]
        goal: String,
        /// A repository, as PATH or PATH@MACHINE; repeatable
        #[arg(long = "repo", value_name = "PATH[@MACHINE]")]
        repos: Vec<String>,
    },
    /// List projects
    List {
        /// Include archived projects
        #[arg(long)]
        all: bool,
    },
    /// Open a project: its workspace, coordinator tab and coordinator agent
    Open {
        #[arg(value_name = "PROJECT")]
        slug: String,
        /// Send the priming prompt again
        #[arg(long)]
        reprime: bool,
        /// Move the project to this session when its recorded socket no longer exists
        #[arg(long)]
        rebind: bool,
        /// Use this configured recipe when starting the project coordinator
        #[arg(long, value_name = "ID", requires = "basis")]
        recipe: Option<String>,
        /// Request that records Rolf's choice; may be request:<project>/<id>
        #[arg(long, value_name = "REQUEST", requires = "recipe")]
        basis: Option<String>,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Print the digest the coordinator reads at the start of every turn
    Context {
        #[arg(value_name = "PROJECT")]
        slug: String,
        /// Print without recording the inbox items as seen
        #[arg(long)]
        peek: bool,
        /// Include unchanged standing notes and historical work
        #[arg(long)]
        full: bool,
    },
    /// Print a bounded, non-consuming snapshot for a fresh coordinator
    Handoff {
        #[arg(value_name = "PROJECT")]
        slug: String,
        /// Save a complete handoff with this session note; - reads stdin
        #[arg(long, value_name = "PATH|-")]
        note_file: Option<String>,
    },
    /// Technical read of current project work, waits and review actions
    Overview {
        #[arg(value_name = "PROJECT")]
        slug: String,
        /// Include resolved thread history
        #[arg(long)]
        history: bool,
    },
    /// Inbox items
    Inbox {
        #[command(subcommand)]
        command: InboxCommand,
    },
    /// Stable tasks and their evidence-derived state
    Task {
        #[command(subcommand)]
        command: TaskCommand,
    },
    /// Provenanced project memory and standing instructions
    Note {
        #[command(subcommand)]
        command: NoteCommand,
    },
    /// Threads: the project's worker agents
    #[command(
        long_about = "Manage a project's worker agents.\n\nEveryday:\n  start, prompt, list, show\n\nRecovery:\n  retry replaces a failed or stuck attempt; only automatic retries are bounded.\n  cancel stops an attempt. rebind attaches its verified live process.\n\nAdministration:\n  attest seals verified stored output. adopt records an existing pane.\n  ack records that a report was seen. resolve performs exceptional cleanup."
    )]
    Thread {
        #[command(subcommand)]
        command: ThreadCommand,
    },
    /// Hold or release new box-lane starts on a saved machine
    Machine {
        #[command(subcommand)]
        command: MachineCommand,
    },
    /// Pause a project: the ticker skips it and `thread start` is refused
    Pause {
        #[arg(value_name = "PROJECT")]
        slug: String,
    },
    /// Make a paused project active again
    Resume {
        #[arg(value_name = "PROJECT")]
        slug: String,
    },
    /// Archive a project: paused, hidden, tokens cleared, `open` refused
    Archive {
        #[arg(value_name = "PROJECT")]
        slug: String,
    },
    /// Make an archived project active again
    Unarchive {
        #[arg(value_name = "PROJECT")]
        slug: String,
    },
    /// Delete a project everywhere; use `archive` to keep a reversible copy
    Delete {
        #[arg(value_name = "PROJECT")]
        slug: String,
        /// Also permanently delete project-owned GitHub repositories
        #[arg(long)]
        github: bool,
        /// Show the exact owned-resource scope without changing anything
        #[arg(long, conflicts_with = "cancel")]
        preview: bool,
        /// Drop a deletion plan only if no steps have completed
        #[arg(long, conflicts_with = "github")]
        cancel: bool,
    },
    /// Continue the current workspace's agent pane as a new project
    AdoptWorkspace {
        /// Project name (default: the workspace label herdr passes to the action)
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        goal: String,
        /// The agent pane to adopt
        #[arg(long)]
        pane: String,
        /// The workspace's directory (the project's repo when it is a git repository)
        #[arg(long, default_value = "")]
        workspace_cwd: String,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Run by herdr's action menu
    #[command(hide = true)]
    Action { id: String },
    /// Recover an interrupted box completion before the courier reads the box
    /// (run by the courier helper on the box)
    #[command(hide = true)]
    Recover,
    /// Run inside a plugin popup pane
    #[command(hide = true)]
    Pane { id: String },
    /// Seal and deliver this lane's completion
    Done {
        /// Report inside the recorded checkout (default: the brief's report)
        #[arg(long, value_name = "PATH")]
        report: Option<String>,
        /// Exact commit (default: the recorded checkout's HEAD)
        #[arg(long)]
        sha: Option<String>,
    },
    /// Seal and deliver the input this lane needs
    Waiting { what: String },
    /// Record a failed lane attempt and request bounded routing recovery
    Failed {
        #[arg(long, value_enum, default_value = "work_failed")]
        class: crate::contracts::FailureClass,
        #[arg(long, requires = "class")]
        provider_kind: Option<String>,
        what: String,
    },
    /// Print a role skill and the runtime-only standing rules
    Skill {
        #[arg(default_value = "coordinator")]
        role: String,
    },
    /// Retire a coordinator binding and remove its owned hook
    Close {
        #[arg(value_name = "PROJECT")]
        slug: String,
    },
    /// Native coordinator prompt hook
    #[command(hide = true)]
    Hook {
        #[arg(long)]
        kind: String,
        #[arg(long)]
        project: String,
        #[arg(long)]
        binding: String,
        #[arg(long, default_value = "complete")]
        phase: String,
    },
    /// Check the setup: versions, tools, root, ticker and each project's session
    Doctor {
        #[command(flatten)]
        session: SessionArgs,
        /// Show wall time per check and duration of outside commands
        #[arg(long)]
        timings: bool,
        /// Remove exactly one resolved retained worktree and its branch
        #[arg(long, value_name = "PROJECT/THREAD")]
        remove_kept_worktree: Option<String>,
    },
    /// Start the ready pile now; enable automatic reviews for this project; display an existing review
    #[command(
        args_conflicts_with_subcommands = true,
        subcommand_negates_reqs = true,
        subcommand_precedence_over_arg = true
    )]
    Review {
        #[arg(value_name = "PROJECT", required = true)]
        slug: Option<String>,
        #[arg(long)]
        repo: Option<String>,
        #[command(subcommand)]
        command: Option<ReviewCommand>,
    },
    /// Build and install the harness repositories after a merge
    Harness {
        #[command(subcommand)]
        command: HarnessCommand,
    },
    /// The plan card: goal, end result and steps
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
    /// The background ticker
    Ticker {
        #[command(subcommand)]
        command: TickerCommand,
    },
}

#[derive(Subcommand)]
enum ReviewCommand {
    /// Replace a stuck or dead reviewer
    Retry {
        #[arg(value_name = "PROJECT")]
        slug: String,
        #[arg(long)]
        repo: Option<String>,
    },
    /// Drop the review and return its lanes to the pile
    Cancel {
        #[arg(value_name = "PROJECT")]
        slug: String,
        #[arg(long)]
        repo: Option<String>,
    },
}

#[derive(Subcommand)]
enum HarnessCommand {
    /// Build every repository in `[harness]` and install it, then the saved box
    Install,
    /// Run the real, bounded post-install journey in a throwaway project
    Journey {
        #[arg(long, hide = true)]
        review: Option<String>,
    },
}

#[derive(Subcommand)]
enum PlanCommand {
    /// Record the outcome check's next action, acceptance evidence or explicit wait
    Check {
        slug: String,
        #[command(subcommand)]
        command: GoalCheckCommand,
    },
    /// Print the plan card; a missing card prints revision zero
    Show {
        /// Project slug
        #[arg(value_name = "PROJECT")]
        slug: String,
    },
    /// Write what this project delivers
    Set {
        /// Project slug
        #[arg(value_name = "PROJECT")]
        slug: String,
        /// Authored outcome: what this project delivers
        #[arg(long)]
        does: String,
        /// Expected plan revision; omitted uses the latest revision
        #[arg(long)]
        expect: Option<u64>,
    },
    /// Add, edit, link, unlink, remove or move one step
    Step {
        #[command(subcommand)]
        command: PlanStepCommand,
    },
    /// Derive step states from the bound work and write only on change
    Sync {
        /// Project slug
        #[arg(value_name = "PROJECT")]
        slug: String,
    },
}

#[derive(Subcommand)]
enum GoalCheckCommand {
    /// Record a party's answer and retire its open waits, keeping the evidence
    Answer {
        party: String,
        #[arg(long)]
        evidence: String,
    },
    Action {
        task: String,
        #[arg(long)]
        evidence: String,
    },
    Close {
        #[arg(long = "task", required = true)]
        tasks: Vec<String>,
        #[arg(long)]
        evidence: String,
    },
    Wait {
        party: String,
        #[arg(long)]
        condition: String,
        #[arg(long = "task")]
        tasks: Vec<String>,
        #[arg(long)]
        evidence: String,
    },
}

#[derive(Args)]
struct PlanStepTarget {
    /// Project slug
    #[arg(value_name = "PROJECT")]
    slug: String,
    /// Step id
    id: String,
    /// Expected plan revision; omitted uses the latest revision
    #[arg(long)]
    expect: Option<u64>,
}

#[derive(Subcommand)]
enum PlanStepCommand {
    /// Add a step, optionally bound to stable tasks
    Add {
        /// Project slug
        #[arg(value_name = "PROJECT")]
        slug: String,
        text: String,
        /// Stable task ids to bind to this step
        #[arg(long = "task", value_name = "ID")]
        tasks: Vec<String>,
        /// Add it as a subtask of this top-level step
        #[arg(long, value_name = "STEP")]
        under: Option<String>,
        /// Step ids that must finish first
        #[arg(long, value_name = "STEP")]
        after: Vec<String>,
        /// Expected plan revision; omitted uses the latest revision
        #[arg(long)]
        expect: Option<u64>,
    },
    /// Replace one step's sentence
    Edit {
        #[command(flatten)]
        target: PlanStepTarget,
        /// Replacement text
        text: String,
    },
    /// Add required work to a step
    Link {
        #[command(flatten)]
        target: PlanStepTarget,
        /// Stable task ids to link to this step
        #[arg(long = "task", value_name = "ID")]
        tasks: Vec<String>,
        #[arg(long, value_name = "STEP")]
        after: Vec<String>,
    },
    /// Remove required work from a step
    Unlink {
        #[command(flatten)]
        target: PlanStepTarget,
        /// Stable task ids to unlink from this step
        #[arg(long = "task", value_name = "ID")]
        tasks: Vec<String>,
        #[arg(long, value_name = "STEP")]
        after: Vec<String>,
        /// Why this step or link is removed
        #[arg(long)]
        reason: String,
    },
    /// Remove one step (identifiers are never reused)
    Remove {
        #[command(flatten)]
        target: PlanStepTarget,
    },
    /// Change display order only
    Move {
        #[command(flatten)]
        target: PlanStepTarget,
        /// Step id that should follow this one
        #[arg(long, value_name = "ID")]
        before: String,
    },
}

#[derive(serde::Serialize)]
struct ReportReference<'a> {
    thread: &'a str,
    path: String,
    sealed: bool,
}

#[derive(serde::Serialize)]
struct TaskShow<'a> {
    task: &'a crate::task::View,
    reports: Vec<ReportReference<'a>>,
    attestation: Option<crate::contracts::Attestation>,
}

enum ThreadResult<'a> {
    Start {
        thread: &'a crate::thread::Thread,
        task: &'a str,
        note: Option<String>,
    },
    Adopt(&'a crate::thread::Thread),
}

impl ThreadResult<'_> {
    fn emit(&self) -> Result<()> {
        let thread = match self {
            Self::Start { thread, .. } | Self::Adopt(thread) => thread,
        };
        let mut data =
            serde_json::json!({ "id": thread.id, "kind": thread.kind, "pane_id": thread.pane_id });
        match self {
            Self::Start { .. } => {
                data["state"] = serde_json::to_value(thread.status)?;
                data["branch"] = thread.branch.clone().into();
                data["machine"] = thread_machine(thread).into();
                data["placement_reason"] = thread.placement_reason.clone().into();
            }
            Self::Adopt(_) => data["prompt_pending"] = thread.prompt_pending.into(),
        }
        // These commands historically print JSON-shaped prose. Build it from
        // the same data, before adding the envelope-only task and note.
        let mut message = format!("{data}\n");
        if let Self::Start { task, note, .. } = self {
            if !task.is_empty() {
                data["task"] = (*task).into();
            }
            if let Some(note) = note {
                data["note"] = note.clone().into();
                message.push_str(&format!("{note}\n"));
            }
        }
        crate::output::success(None, &data, &message, "")
    }
}

fn thread_machine(thread: &crate::thread::Thread) -> &str {
    if thread.machine.is_empty() {
        "local"
    } else {
        &thread.machine
    }
}

#[derive(serde::Serialize)]
struct PlanMutation<'a> {
    operation: &'a str,
    revision: u64,
    step: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    before: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    record: Option<&'a crate::contracts::PlanStep>,
    plan: &'a crate::contracts::Plan,
}

fn run_plan_command(ctx: &Ctx, command: PlanCommand) -> Result<()> {
    use crate::plan;
    match command {
        PlanCommand::Check { slug, command } => {
            use crate::steps::goal_check::{self, Disposition};
            let project = Project::load(&ctx.root, &slug)?;
            goal_check::reconcile(&project, None, jiff::Timestamp::now().as_second() as u64)?;
            let (disposition, evidence) = match command {
                GoalCheckCommand::Answer { party, evidence } => {
                    goal_check::answer(&project, &party, &evidence)?;
                    crate::output::insert(
                        "goal_check",
                        serde_json::to_value(goal_check::load(&project))?,
                    );
                    println!("goal check answer recorded");
                    return Ok(());
                }
                GoalCheckCommand::Action { task, evidence } => {
                    (Disposition::Action { task }, evidence)
                }
                GoalCheckCommand::Close { tasks, evidence } => {
                    let outcome = plan::load(&project)?.map_or(String::new(), |p| p.does);
                    (Disposition::Closed { tasks, outcome }, evidence)
                }
                GoalCheckCommand::Wait {
                    party,
                    condition,
                    tasks,
                    evidence,
                } => (
                    Disposition::Wait {
                        tasks,
                        party,
                        condition,
                    },
                    evidence,
                ),
            };
            goal_check::record(&project, disposition, &evidence)?;
            crate::output::insert(
                "goal_check",
                serde_json::to_value(goal_check::load(&project))?,
            );
            println!("goal check disposition recorded");
            Ok(())
        }
        PlanCommand::Show { slug } => {
            crate::review::classify_old_seals(ctx, &Project::load(&ctx.root, &slug)?, true)?;
            let result = plan::show(ctx, &slug)?;
            #[derive(serde::Serialize)]
            struct ShownPlan<'a> {
                result: &'a plan::Show,
            }
            crate::output::success(None, &ShownPlan { result: &result }, &result.message(), "")
        }
        PlanCommand::Set { slug, does, expect } => {
            let p = plan::set(ctx, &slug, &does, expect)?;
            crate::output::insert("revision", p.revision);
            println!("plan revision {} set", p.revision);
            Ok(())
        }
        PlanCommand::Step { command } => {
            let (p, id, operation, before) = match command {
                PlanStepCommand::Add {
                    slug,
                    text,
                    tasks,
                    under,
                    after,
                    expect,
                } => {
                    let (p, id) = match under {
                        Some(under) => {
                            plan::subtask_add(ctx, &slug, &under, &text, tasks, after, expect)?
                        }
                        None => {
                            let p = plan::step_add(ctx, &slug, &text, tasks, after, expect)?;
                            let id = p.steps.last().map(|s| s.id.clone()).unwrap_or_default();
                            (p, id)
                        }
                    };
                    (p, id, "added", None)
                }
                PlanStepCommand::Edit {
                    target: PlanStepTarget { slug, id, expect },
                    text,
                } => {
                    let p = plan::step_edit(ctx, &slug, &id, &text, expect)?;
                    (p, id, "edited", None)
                }
                PlanStepCommand::Link {
                    target: PlanStepTarget { slug, id, expect },
                    tasks,
                    after,
                } => {
                    let p = plan::step_link(ctx, &slug, &id, tasks, after, expect)?;
                    (p, id, "linked", None)
                }
                PlanStepCommand::Unlink {
                    target: PlanStepTarget { slug, id, expect },
                    tasks,
                    after,
                    reason,
                } => {
                    let p = plan::step_unlink(ctx, &slug, &id, tasks, after, &reason, expect)?;
                    (p, id, "unlinked", None)
                }
                PlanStepCommand::Remove {
                    target: PlanStepTarget { slug, id, expect },
                } => {
                    let p = plan::step_remove(ctx, &slug, &id, expect)?;
                    (p, id, "removed", None)
                }
                PlanStepCommand::Move {
                    target: PlanStepTarget { slug, id, expect },
                    before,
                } => {
                    let p = plan::step_move(ctx, &slug, &id, &before, expect)?;
                    (p, id, "moved", Some(before))
                }
            };
            let result = PlanMutation {
                operation,
                revision: p.revision,
                step: &id,
                before: before.as_deref(),
                record: plan::all_steps(&p).find(|step| step.id == id),
                plan: &p,
            };
            crate::output::success(
                None,
                &result,
                &format!(
                    "plan revision {}: {} {}\n",
                    result.revision, result.operation, result.step
                ),
                "",
            )
        }
        PlanCommand::Sync { slug } => {
            match plan::sync(ctx, &slug)? {
                plan::SyncOutcome::Missing => {
                    crate::output::set_outcome("plan_missing");
                    println!("no plan is written down yet")
                }
                plan::SyncOutcome::Unchanged { revision, holds } => {
                    crate::output::set_outcome("unchanged");
                    crate::output::insert("revision", revision);
                    if !holds.is_empty() {
                        crate::output::insert("failed_check_holds", serde_json::to_value(&holds)?);
                    }
                    println!("plan revision {revision}: no step state changed");
                    for (id, hold) in holds {
                        println!("{id}: {}", hold.message());
                    }
                }
                plan::SyncOutcome::Changed { revision } => {
                    crate::output::insert("revision", revision);
                    println!("plan revision {revision}: step states refreshed")
                }
            }
            Ok(())
        }
    }
}

#[derive(Subcommand)]
enum InboxCommand {
    /// List unhandled inbox items, oldest first
    List {
        #[arg(value_name = "PROJECT")]
        slug: String,
    },
    /// Move handled items to inbox/done/
    Done {
        #[arg(value_name = "PROJECT")]
        slug: String,
        #[arg(value_name = "ITEM_ID", required_unless_present_any = ["all", "kind"])]
        ids: Vec<String>,
        #[arg(long, conflicts_with_all = ["ids", "kind"])]
        all: bool,
        /// Acknowledge every unhandled item of this kind
        #[arg(long, conflicts_with = "ids")]
        kind: Option<String>,
    },
}

#[derive(Subcommand)]
enum TaskCommand {
    /// Link confirmed incidents and measured outcomes to an ordinary repair task
    Repair(crate::task::RepairArgs),
    /// Add a task tied to Rolf's request and exact acceptance conditions
    Add {
        #[arg(value_name = "PROJECT")]
        slug: String,
        #[arg(long)]
        title: String,
        /// Request id, optionally qualified as <project>/<id>
        #[arg(long = "request", required = true)]
        requests: Vec<String>,
        #[arg(long = "acceptance", required = true)]
        acceptance: Vec<String>,
        #[arg(long, value_name = "PATH")]
        repo: Option<String>,
        /// Older note, instruction, decision or task this task replaces
        #[arg(long)]
        replaces: Option<String>,
    },
    /// Show one task and its derived state
    Show {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
    },
    /// List tasks and their derived state
    List {
        #[arg(value_name = "PROJECT")]
        slug: String,
    },
    /// Drop a task, or withdraw acceptance conditions replaced by a newer choice
    Drop {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        #[arg(long = "acceptance")]
        acceptance: Vec<usize>,
        #[arg(long)]
        reason: String,
    },
}

#[derive(Subcommand)]
enum NoteCommand {
    /// Retire a current memory or instruction, preserving its history
    Retire {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        /// Request id behind this retirement
        #[arg(long)]
        request: String,
        /// Why this fact or instruction is no longer in force
        #[arg(long)]
        reason: String,
    },
    /// Add a memory note or standing instruction with its provenance
    Add {
        #[arg(value_name = "PROJECT")]
        slug: String,
        text: String,
        #[arg(long, value_enum)]
        kind: crate::note::Kind,
        /// Request id behind this note, optionally qualified as <project>/<id>
        #[arg(long)]
        request: String,
        /// Note or instruction this explicitly replaces
        #[arg(long)]
        replaces: Option<String>,
        /// Limit this note to briefs for these stable tasks
        #[arg(long = "task")]
        tasks: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ThreadCommand {
    /// Start a thread from a stable task; explicit details override that task
    Start {
        #[arg(value_name = "PROJECT")]
        slug: String,
        #[arg(long)]
        title: Option<String>,
        #[arg(long, value_name = "PATH")]
        repo: Option<String>,
        #[arg(long, value_name = "LABEL|ID|local")]
        machine: Option<String>,
        /// The integration branch to pin the code base to (default: the checked-out branch)
        #[arg(long, value_name = "BRANCH")]
        base: Option<String>,
        /// The task; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        task_file: String,
        /// Named files to carry beside the lane's frozen brief (repeatable)
        #[arg(long, value_name = "PATH")]
        attach: Vec<String>,
        /// Writable repository-relative globs (repeatable; *, ?, **)
        #[arg(long, value_name = "GLOB")]
        paths: Vec<String>,
        /// Instruction set for this lane; the routing table may match it
        #[arg(long, value_name = "FLOW")]
        workflow: Option<String>,
        /// Exact configured recipe for this one lane (the coordinator's choice)
        #[arg(long, value_name = "ID")]
        recipe: Option<String>,
        /// Existing stable task id. Ordinary lanes must name this or create one.
        #[arg(long, value_name = "TASK", conflicts_with = "requests")]
        job: Option<String>,
        /// Authority for a task created with this lane
        #[arg(long = "request", requires = "acceptance")]
        requests: Vec<String>,
        /// Acceptance conditions for a task created with this lane
        #[arg(long = "acceptance", requires = "requests")]
        acceptance: Vec<String>,
    },
    /// Replace a failed, blocked, or stuck attempt on the same recipe, even after automatic retries
    Retry {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Stop a thread, close its pane and remove its clean worktree
    Cancel {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Point this thread at the verified live process already doing its work
    Rebind {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        #[arg(long, value_name = "PANE")]
        pane: String,
    },
    /// Seal completion from a resolved lane's verified stored report
    Attest {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Send a follow-up to a thread's agent
    Prompt {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        /// The text; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        text_file: String,
    },
    /// List threads with live state and group
    List {
        #[arg(value_name = "PROJECT")]
        slug: String,
    },
    /// Show one thread's record
    Show {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
    },
    /// Record an existing local agent pane as a thread of this project
    Adopt {
        #[arg(value_name = "PROJECT")]
        slug: String,
        #[arg(long, value_name = "ID")]
        pane: String,
        #[arg(long)]
        title: String,
        /// Optional task; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        task_file: Option<String>,
        #[arg(long, value_name = "FLOW")]
        workflow: Option<String>,
        /// Do not send a primer
        #[arg(long)]
        passive: bool,
    },
    /// Record that the user has seen the current report
    Ack {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
    },
    /// Resolve a thread (final copy first), or reopen a resolved one
    Resolve {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        #[arg(long, conflicts_with_all = ["skip_copy", "discard_uncopied", "keep_pane"])]
        reopen: bool,
        /// Resolve even though the final copy cannot be made
        #[arg(long)]
        skip_copy: bool,
        /// Accept removing a finished worktree when some files could not be copied
        #[arg(long)]
        discard_uncopied: bool,
        /// Leave the lane's pane and tab open instead of closing them
        #[arg(long)]
        keep_pane: bool,
    },
}

/// A brief is delivered unchanged. This warning only points out Mac paths a
/// remote lane cannot read; it never affects placement or retry.
fn mac_only_brief_note(
    config_dir: &std::path::Path,
    project: &Project,
    machine: &str,
    brief: &str,
) -> Option<String> {
    if machine.is_empty() || machine == "local" {
        return None;
    }
    let mut mapped = crate::harness::repos(config_dir).unwrap_or_default();
    if let Ok(declaration) = crate::remote::machine_declaration(config_dir, machine) {
        mapped.extend(declaration.repos);
    }
    if let Ok((settings, _)) = project.read_project_md() {
        mapped.extend(
            settings
                .repos
                .into_iter()
                .filter(|row| row.box_path.is_some()),
        );
    }
    let paths = mac_only_paths(brief, &mapped);
    if paths.is_empty() {
        return None;
    }
    Some(format!(
        "note: brief names Mac-only paths the lane on {machine} can't read: {}. Paste their text into the brief, or start with --machine local.",
        paths.join(", ")
    ))
}

fn mac_only_paths(brief: &str, mapped: &[crate::project::Repo]) -> Vec<String> {
    let mut paths = Vec::new();
    for (index, _) in brief.match_indices('/') {
        if index > 0
            && !brief[..index].ends_with(|c: char| {
                c.is_whitespace()
                    || matches!(c, '`' | '"' | '\'' | '<' | '(' | '[' | '{' | '=' | ':')
            })
        {
            continue;
        }
        let tail = &brief[index..];
        let candidate = tail.starts_with("/private/tmp/")
            || tail.starts_with("/tmp/")
            || tail.strip_prefix("/Users/").is_some_and(|rest| {
                rest.split_once('/')
                    .is_some_and(|(name, _)| !name.is_empty())
            });
        if !candidate {
            continue;
        }
        let end = tail
            .find(|c: char| {
                c.is_whitespace()
                    || matches!(
                        c,
                        '`' | '"'
                            | '\''
                            | '<'
                            | '>'
                            | '('
                            | ')'
                            | '['
                            | ']'
                            | '{'
                            | '}'
                            | ','
                            | ';'
                    )
            })
            .unwrap_or(tail.len());
        let path = tail[..end].trim_end_matches(['.', ':', '!', '?']);
        if mapped.iter().any(|row| {
            row.box_path.is_some()
                && (path == row.path
                    || path
                        .strip_prefix(&row.path)
                        .is_some_and(|suffix| suffix.starts_with('/')))
        }) || paths.iter().any(|previous| previous == path)
        {
            continue;
        }
        paths.push(path.to_string());
        if paths.len() == 3 {
            break;
        }
    }
    paths
}

fn start_details(
    existing: Option<&crate::task::Task>,
    title: Option<String>,
    repo: Option<String>,
) -> Result<(String, Option<String>)> {
    let title = title
        .or_else(|| existing.map(|task| task.title.clone()))
        .ok_or_else(|| {
            crate::refusal::error(
                "task_title: --title is required when the lane does not name an existing --job", "ha thread start <project> --title \"<title>\" --request <request-id> --acceptance \"<condition>\" --task-file <file>")
        })?;
    let repo = repo.or_else(|| existing.and_then(|task| task.repo.clone()));
    Ok((title, repo))
}

/// `-` is standard input; a relative path is relative to the caller's directory.
fn read_text(file: &str) -> Result<String> {
    use std::io::Read;
    if file == "-" {
        let mut text = String::new();
        std::io::stdin().read_to_string(&mut text)?;
        Ok(text)
    } else {
        std::fs::read_to_string(file).map_err(|e| anyhow::anyhow!("could not read {file}: {e}"))
    }
}

#[derive(Subcommand)]
enum MachineCommand {
    /// Refuse new box-lane starts on this machine
    Hold { machine: String },
    /// Allow new box-lane starts again
    Release { machine: String },
}

#[derive(Subcommand)]
enum TickerCommand {
    /// Start the ticker if it is not running (does nothing when there are no projects)
    Start,
    /// Start only if no ticker holds the lock; never replace a running build
    Ensure,
    /// Run the ticker loop in the foreground
    Run,
    /// Ask the running ticker to exit and wait for it
    Stop,
    /// Show the running ticker's version, root and tool resolution
    Status,
}

fn machine_outcome(command: &str) -> String {
    let outcome = match command {
        "open" => "opened",
        "context" | "plan show" => "shown",
        "thread start" => "started",
        "thread retry" => "retried",
        "thread cancel" => "cancelled",
        "thread rebind" => "rebound",
        "thread attest" => "attested",
        "thread adopt" => "adopted",
        "thread prompt" => "prompted",
        "thread resolve" => "resolved",
        "thread ack" => "acknowledged",
        "thread list" => "listed",
        "thread show" => "shown",
        "done" | "waiting" | "failed" => "sealed",
        "doctor" => "healthy",
        "harness install" => "installed",
        "inbox done" => "moved",
        command if command.starts_with("plan ") => "plan_changed",
        _ => "succeeded",
    };
    outcome.to_string()
}

fn machine_command(matches: &clap::ArgMatches) -> (String, BTreeMap<String, serde_json::Value>) {
    let mut leaf = matches;
    let mut path = Vec::new();
    let mut data = BTreeMap::new();
    loop {
        for key in [
            "slug", "project", "review", "thread", "id", "name", "sha", "report", "branch", "pane",
        ] {
            if let Ok(Some(value)) = leaf.try_get_one::<String>(key) {
                data.insert(key.to_string(), serde_json::Value::String(value.clone()));
            }
        }
        match leaf.subcommand() {
            Some((name, child)) => {
                path.push(name.to_string());
                leaf = child;
            }
            None => break,
        }
    }
    (path.join(" "), data)
}

fn partial_machine_result() -> (String, BTreeMap<String, serde_json::Value>) {
    Cli::command()
        .ignore_errors(true)
        .try_get_matches()
        .ok()
        .map(|matches| machine_command(&matches))
        .unwrap_or_default()
}

pub fn run() -> Result<()> {
    let cli_started = std::time::Instant::now();
    let wants_json = std::env::args_os().any(|arg| arg == "--json");
    let matches = match Cli::command().try_get_matches() {
        Ok(matches) => matches,
        Err(error)
            if !wants_json
                || matches!(
                    error.kind(),
                    clap::error::ErrorKind::DisplayHelp | clap::error::ErrorKind::DisplayVersion
                ) =>
        {
            error.exit()
        }
        Err(error) => {
            let (command, data) = partial_machine_result();
            crate::output::begin(true, command, "refused".into(), data);
            return Err(error.into());
        }
    };
    let cli = Cli::from_arg_matches(&matches)?;
    let (result_command, result_data) = machine_command(&matches);
    crate::output::begin(
        cli.json,
        result_command.clone(),
        machine_outcome(&result_command),
        result_data,
    );
    let mut leaf = &matches;
    let mut explicit_slug = None;
    loop {
        if let Ok(Some(slug)) = leaf.try_get_one::<String>("slug") {
            explicit_slug = Some(slug.clone());
        }
        match leaf.subcommand() {
            Some((_, args)) => leaf = args,
            None => break,
        }
    }
    let env = Env::from_process()?;
    let config_dir = env.config_dir();
    let root = paths::resolve_root(cli.root.as_deref(), &env, &config_dir)?;
    let real_runner = RealRunner;
    let runner: &dyn crate::runner::Runner = &real_runner;
    let ctx = Ctx {
        env: &env,
        root,
        config_dir,
        runner,
        detached_ticker: true,
    };

    // The installed image's read-only probe must not wake a ticker or render
    // context. Its entire output is numbers and readability, not project text.
    match &cli.command {
        Command::InstallCheck => return install_check(&ctx),
        Command::InstallRundownCheck => {
            return installed_check_reply(crate::harness::reopened_rundown_check(&ctx));
        }
        Command::InstallJourneyStart { review } => {
            return install_journey_start(&ctx, review.as_deref());
        }
        _ => {}
    }
    // A diagnostic transport executes explicit probes only, never project
    // discovery or ticker wake-up on the target machine.
    if matches!(cli.command, Command::Doctor { .. })
        && std::env::var_os("HERDR_ADE_BOX_INPUT").is_some()
    {
        return crate::box_helper::run(&ctx);
    }
    let (_awake, _) = crate::awake::enter(&ctx.root, false)?;
    let observed_slug = explicit_slug
        .or_else(|| {
            ctx.env
                .var("HERDR_ADE_LAUNCH")
                .and_then(project::LaunchEnv::parse)
                .map(|l| l.project)
        })
        .or_else(|| {
            overview::project_for_workspace(
                &ctx,
                ctx.env.var("HERDR_WORKSPACE_ID").unwrap_or(""),
                ctx.env.var("HERDR_SOCKET_PATH").unwrap_or(""),
            )
        });
    let observed_project = observed_slug
        .as_deref()
        .and_then(|s| Project::load(&ctx.root, s).ok());
    let prefix = coordinator::current_prefix(&ctx.root)?;
    let result = dispatch_with_start(ctx, cli.command, Some(cli_started));
    record_command_outcome(observed_project.as_ref(), &result, &prefix);
    if result.is_ok() {
        crate::output::finish_success()?;
    }
    result
}

/// Keep refusal and error outcomes distinct without filing a separate failure.
fn record_command_outcome(project: Option<&Project>, result: &Result<()>, prefix: &str) {
    match result {
        Err(error) if crate::refusal::is(error) => {
            if let Some(next) = crate::refusal::next(error) {
                let next = next
                    .strip_prefix("ha ")
                    .map_or_else(|| next.to_string(), |command| format!("{prefix} {command}"));
                crate::output::set_next(&next);
            }
            crate::output::set_outcome("refused");
            crate::output::set_failure_class(None);
        }
        Err(_) => {
            if project.is_some() {
                crate::output::set_outcome("failed");
                crate::output::set_failure_class(Some("unknown"));
            } else {
                // With no project binding this is a rejected invocation (for
                // example a missing project), not evidence of failed work.
                crate::output::set_outcome("refused");
                crate::output::set_failure_class(None);
            }
        }
        Ok(()) => {}
    }
}

fn install_check(ctx: &Ctx) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(&crate::harness::check::snapshot(&ctx.root))?
    );
    Ok(())
}

fn installed_check_reply(result: Result<Option<String>>) -> Result<()> {
    println!(
        "{}",
        serde_json::to_string(&crate::harness::InstalledCheck::from_result(result))?
    );
    Ok(())
}

fn install_journey_start(ctx: &Ctx, review: Option<&str>) -> Result<()> {
    installed_check_reply((|| {
        let review = review
            .map(|review| review.split_once('/').context("expected PROJECT/REVIEW"))
            .transpose()?;
        crate::journey::after_install(ctx, review)?;
        Ok(None)
    })())
}

fn dispatch_with_start(
    ctx: Ctx<'_>,
    command: Command,
    cli_started: Option<std::time::Instant>,
) -> Result<()> {
    match command {
        Command::InstallCheck => install_check(&ctx),
        Command::InstallRundownCheck => {
            installed_check_reply(crate::harness::reopened_rundown_check(&ctx))
        }
        Command::InstallJourneyStart { review } => install_journey_start(&ctx, review.as_deref()),
        Command::New { name, goal, repos } => {
            let repos = repos
                .iter()
                .map(|arg| project::parse_repo_arg(arg))
                .collect();
            let project = project::create(&ctx.root, &name, &goal, repos)?;
            println!("created `{}` at {}", project.slug, project.dir().display());
            println!(
                "next: {} open {}",
                coordinator::current_prefix(&ctx.root)?,
                project.slug
            );
            Ok(())
        }
        Command::List { all } => {
            for slug in project::list_slugs(&ctx.root) {
                let project = Project::load(&ctx.root, &slug)?;
                let status = project.status();
                if status == Status::Archived && !all {
                    continue;
                }
                let mut counts = std::collections::BTreeMap::new();
                for row in threads::rows(&ctx, &project) {
                    *counts
                        .entry(row.group.rank())
                        .or_insert((row.group.label(), 0)) = (
                        row.group.label(),
                        counts
                            .get(&row.group.rank())
                            .map_or(0, |c: &(&str, usize)| c.1)
                            + 1,
                    );
                }
                let summary: Vec<String> = counts
                    .values()
                    .map(|(label, n)| format!("{label}: {n}"))
                    .collect();
                println!(
                    "{slug}\t{status}\t{}",
                    if summary.is_empty() {
                        "no threads".to_string()
                    } else {
                        summary.join(", ")
                    }
                );
            }
            Ok(())
        }
        Command::Open {
            slug,
            reprime,
            rebind,
            recipe,
            basis,
            session,
        } => coordinator::open(
            &ctx,
            &slug,
            &OpenOptions {
                session: session.into(),
                reprime,
                rebind,
                recipe,
                recipe_basis: basis,
            },
        ),
        Command::Context { slug, peek, full } => coordinator::context(&ctx, &slug, peek, full),
        Command::Handoff { slug, note_file } => {
            crate::handoff::print(&ctx, &slug, note_file.as_deref())
        }
        Command::Overview { slug, history } => overview::run(&ctx, &slug, history),
        Command::Inbox { command } => match command {
            InboxCommand::List { slug } => {
                let project = Project::load(&ctx.root, &slug)?;
                let items = inbox::unhandled(&project);
                let human = if items.is_empty() {
                    "no unhandled inbox items\n".to_string()
                } else {
                    items
                        .iter()
                        .map(|item| format!("{}  {}  {}\n", item.id, item.kind, item.summary))
                        .collect()
                };
                crate::output::success(None, &serde_json::json!({ "items": items }), &human, "")
            }
            InboxCommand::Done {
                slug,
                ids,
                all,
                kind,
            } => {
                let project = Project::load(&ctx.root, &slug)?;
                let record = project.coordinator();
                let pane = std::env::var("HERDR_PANE_ID").ok();
                let binding = record
                    .as_ref()
                    .and_then(|record| pane.as_deref().map(|pane| (pane, record.attempt())));
                let result = if let Some(kind) = kind {
                    inbox::done_kind_bound(&project, &kind, binding)?
                } else {
                    inbox::done_bound(&project, &ids, all, binding)?
                };
                crate::output::success(
                    None,
                    &serde_json::json!({
                        "moved": result.moved,
                        "moved_count": result.moved.len(),
                        "missing": result.missing,
                    }),
                    &result.message(),
                    &result.warnings(),
                )
            }
        },
        Command::Machine { command } => match command {
            MachineCommand::Hold { machine } => {
                let profile = crate::remote::machine_profile(
                    ctx.runner,
                    &ctx.env.herdr_bin(),
                    &ctx.config_dir,
                    &machine,
                )?;
                if profile.is_local() {
                    bail!("machine_local: the Mac cannot be held as a box");
                }
                let path = project::machine_hold(&ctx.root, &profile.id)?;
                println!(
                    "machine `{}` is held; new box starts are refused ({})",
                    profile.label,
                    path.display()
                );
                Ok(())
            }
            MachineCommand::Release { machine } => {
                let profile = crate::remote::machine_profile(
                    ctx.runner,
                    &ctx.env.herdr_bin(),
                    &ctx.config_dir,
                    &machine,
                )?;
                if profile.is_local() {
                    bail!("machine_local: the Mac is not a box hold");
                }
                let removed = project::machine_release(&ctx.root, &profile.id)?;
                if removed {
                    println!("machine `{}` is released", profile.label);
                } else {
                    println!("machine `{}` was not held", profile.label);
                }
                Ok(())
            }
        },
        Command::Note { command } => match command {
            NoteCommand::Retire {
                slug,
                id,
                request,
                reason,
            } => {
                let project = Project::load(&ctx.root, &slug)?;
                let record = crate::note::retire(&project, &id, &request, &reason)?;
                crate::output::success(
                    Some("retired"),
                    &serde_json::json!({ "retirement": record }),
                    &format!("retired {id}\n"),
                    "",
                )
            }
            NoteCommand::Add {
                slug,
                text,
                kind,
                request,
                replaces,
                tasks,
            } => {
                let project = Project::load(&ctx.root, &slug)?;
                let note =
                    crate::note::add(&project, kind, &text, &request, replaces.as_deref(), tasks)?;
                let reach = if note.tasks.is_empty() {
                    "in every lane brief".to_string()
                } else {
                    format!("in briefs for {}", note.tasks.join(", "))
                };
                crate::output::success(
                    Some("noted"),
                    &serde_json::json!({ "note": note }),
                    &format!("noted {}, {reach}\n", note.id),
                    "",
                )
            }
        },
        Command::Task { command } => match command {
            TaskCommand::Repair(args) => {
                let project = Project::load(&ctx.root, &args.slug)?;
                let task = crate::task::record_repair(&project, &args.id, args.command)?;
                let view = crate::task::view(&project, task);
                crate::output::success(
                    Some("recorded"),
                    &serde_json::json!({"task": view}),
                    &crate::task::render(&project, &view),
                    "",
                )
            }
            TaskCommand::Add {
                slug,
                title,
                requests,
                acceptance,
                repo,
                replaces,
            } => {
                let project = Project::load(&ctx.root, &slug)?;
                let authority = requests
                    .into_iter()
                    .map(|reference| {
                        if reference.starts_with("request:") || reference.starts_with("ask:") {
                            reference
                        } else {
                            format!("request:{reference}")
                        }
                    })
                    .collect();
                let record =
                    crate::task::add(&project, &title, authority, acceptance, repo, replaces)?;
                let view = crate::task::view(&project, record);
                crate::output::success(
                    Some("added"),
                    &serde_json::json!({ "task": view }),
                    &format!(
                        "{} [{}] {}\n",
                        view.record.id,
                        view.state.word(),
                        view.record.title
                    ),
                    "",
                )
            }
            TaskCommand::Show { slug, id } => {
                let project = Project::load(&ctx.root, &slug)?;
                crate::review::classify_old_seals(&ctx, &project, true)?;
                let view = crate::task::view(&project, crate::task::load(&project, &id)?);
                let reports = view
                    .record
                    .attempts
                    .iter()
                    .filter_map(|id| {
                        let thread = crate::thread::load(&project, id).ok()?;
                        let path = crate::thread::report_reference(&project, &thread)?;
                        let sealed = crate::thread::sealed_report_path(&project, &thread).is_some();
                        Some(ReportReference {
                            thread: id,
                            path,
                            sealed,
                        })
                    })
                    .collect();
                let result = TaskShow {
                    task: &view,
                    reports,
                    attestation: crate::task::attestation(&project, &view.record),
                };
                let mut message = crate::task::render(&project, result.task);
                for report in &result.reports {
                    message.push_str(&format!(
                        "{} ({}): {}\n",
                        if report.sealed {
                            "final report"
                        } else {
                            "historical report (not completion)"
                        },
                        report.thread,
                        report.path
                    ));
                }
                if let Some(attestation) = &result.attestation {
                    message.push_str(&format!(
                        "attested: {}: {}\n",
                        attestation.coordinator, attestation.reason
                    ));
                }
                crate::output::success(Some("shown"), &result, &message, "")
            }
            TaskCommand::List { slug } => {
                let project = Project::load(&ctx.root, &slug)?;
                crate::review::classify_old_seals(&ctx, &project, true)?;
                let (views, errors) = crate::task::views(&project);
                if let Some(error) = errors.first() {
                    return Err(anyhow::anyhow!("task_unreadable: {error:#}"));
                }
                let message = views
                    .iter()
                    .map(|view| crate::task::render(&project, view))
                    .collect::<Vec<_>>()
                    .join("");
                crate::output::success(
                    Some("listed"),
                    &serde_json::json!({ "tasks": views }),
                    &message,
                    "",
                )
            }
            TaskCommand::Drop {
                slug,
                id,
                acceptance,
                reason,
            } => {
                let project = Project::load(&ctx.root, &slug)?;
                if acceptance.is_empty() {
                    let outcome = crate::task::drop_task(&ctx, &project, &id, &reason)?;
                    let view = crate::task::view(&project, outcome.task);
                    let mut message = format!("{} dropped: {}\n", view.record.id, reason.trim());
                    for lane in &outcome.lanes {
                        if lane.state == "resolved" || lane.state == "cancelled" {
                            message.push_str(&lane.message(&slug));
                        } else {
                            message.push_str(&format!(
                                "{} retirement incomplete ({}): {}; retry with ha thread resolve {} {}\n",
                                lane.thread, lane.state, lane.copy_notes.join("; "), slug, lane.thread
                            ));
                        }
                    }
                    crate::output::success(
                        Some("dropped"),
                        &serde_json::json!({ "task": view, "lanes": outcome.lanes }),
                        &message,
                        "",
                    )
                } else {
                    let mut numbers = acceptance.clone();
                    numbers.sort_unstable();
                    numbers.dedup();
                    let record =
                        crate::task::withdraw_acceptance(&project, &id, acceptance, &reason)?;
                    let view = crate::task::view(&project, record);
                    let numbers = numbers
                        .iter()
                        .map(usize::to_string)
                        .collect::<Vec<_>>()
                        .join(", ");
                    crate::output::success(
                        Some("withdrawn"),
                        &serde_json::json!({ "task": view }),
                        &format!(
                            "{} acceptance {} withdrawn: {}\n",
                            view.record.id,
                            numbers,
                            reason.trim()
                        ),
                        "",
                    )
                }
            }
        },
        Command::Thread { command } => match command {
            ThreadCommand::Start {
                slug,
                title,
                repo,
                machine,
                base,
                task_file,
                attach,
                paths,
                workflow,
                recipe,
                job,
                requests,
                acceptance,
            } => {
                let task = read_text(&task_file)?;
                let project = Project::load(&ctx.root, &slug)?;
                let existing = job
                    .as_deref()
                    .map(|id| crate::task::load(&project, id))
                    .transpose()?;
                let (title, repo) = start_details(existing.as_ref(), title, repo)?;
                let task_id = match job {
                    Some(id) => id,
                    None if workflow.as_deref() == Some("reviewer") => String::new(),
                    None if requests.is_empty() => {
                        return Err(crate::refusal::error(
                            "task_missing: a lane must pass --job <task>, or create one with --request and --acceptance",
                            "ha thread start <project> --job <job> --task-file <file>",
                        ));
                    }
                    None => {
                        let authority = requests
                            .into_iter()
                            .map(|reference| {
                                if reference.starts_with("request:")
                                    || reference.starts_with("ask:")
                                {
                                    reference
                                } else {
                                    format!("request:{reference}")
                                }
                            })
                            .collect();
                        crate::task::add(
                            &project,
                            &title,
                            authority,
                            acceptance,
                            repo.clone(),
                            None,
                        )?
                        .id
                    }
                };
                let brief_text = task.clone();
                let thread = threads::start(
                    &ctx,
                    &slug,
                    StartArgs {
                        title,
                        repo,
                        machine,
                        base,
                        task,
                        attach,
                        paths,
                        workflow,
                        recipe,
                        task_id: task_id.clone(),
                        review_id: String::new(),
                    },
                )?;
                let machine = thread_machine(&thread);
                let mut note = mac_only_brief_note(&ctx.config_dir, &project, machine, &brief_text);
                if thread.prompt_pending && !thread.pane_id.is_empty() {
                    let pending = "brief pending; the ticker delivers it when the agent registers";
                    note = Some(match note {
                        Some(other) => format!("{pending}\n{other}"),
                        None => pending.to_string(),
                    });
                }
                ThreadResult::Start {
                    thread: &thread,
                    task: &task_id,
                    note,
                }
                .emit()
            }
            ThreadCommand::Retry { slug, id, reason } => {
                let result = threads::retry(&ctx, &slug, &id, &reason)?;
                let project = Project::load(&ctx.root, &slug)?;
                let current = crate::thread::load(&project, &id)?;
                let note = std::fs::read_to_string(crate::thread::task_path(&project, &id))
                    .ok()
                    .and_then(|brief| {
                        mac_only_brief_note(&ctx.config_dir, &project, &current.machine, &brief)
                    });
                if let Some(note) = &note {
                    crate::output::insert("note", note.clone());
                }
                crate::output::success(
                    Some("retried"),
                    &result,
                    &format!(
                        "{}{}\n{}",
                        result.message(),
                        result
                            .screen
                            .as_ref()
                            .map(|screen| format!(
                                "; previous startup screen still showed: {screen}"
                            ))
                            .unwrap_or_default(),
                        note.map(|note| format!("{note}\n")).unwrap_or_default()
                    ),
                    "",
                )
            }
            ThreadCommand::Cancel { slug, id, reason } => {
                let result = threads::cancel(&ctx, &slug, &id, &reason)?;
                let message = if result.state == "cleanup_pending" {
                    format!(
                        "{} cancelled; pane cleanup is pending and will be retried\n",
                        result.thread
                    )
                } else {
                    format!(
                        "{} cancelled; pane {} and worktree {}\n",
                        result.thread, result.pane, result.worktree
                    )
                };
                crate::output::success(Some(&result.state), &result, &message, "")
            }
            ThreadCommand::Rebind { slug, id, pane } => {
                let result = threads::rebind(&ctx, &slug, &id, &pane)?;
                crate::output::success(
                    Some("rebound"),
                    &result,
                    &format!("{} is rebound to pane {}\n", result.thread, result.pane_id),
                    "",
                )
            }
            ThreadCommand::Attest { slug, id, reason } => {
                let result = threads::attest(&ctx, &slug, &id, &reason)?;
                crate::output::success(
                    Some("attested"),
                    &result,
                    &format!("{} attested: {}\n", result.thread, result.reason),
                    "",
                )
            }
            ThreadCommand::Prompt {
                slug,
                id,
                text_file,
            } => {
                let text = read_text(&text_file)?;
                let outcome = threads::prompt(&ctx, &slug, &id, &text)?;
                let project = Project::load(&ctx.root, &slug)?;
                let lane = crate::thread::load(&project, &id)?;
                let events = crate::events::list(&project);
                let seal = crate::events::latest_done_event(&events, &id, lane.attempt.max(1))
                    .filter(|event| crate::threads::follow_up_pending_for_seal(&lane, Some(event)));
                match outcome {
                    threads::PromptOutcome::Queued { attempt } => {
                        crate::output::set_outcome("queued");
                        crate::output::insert("delivery", "queued");
                        crate::output::insert("attempt", attempt);
                        println!(
                            "queued for {id} attempt {attempt}; it will be delivered after the brief"
                        );
                    }
                    threads::PromptOutcome::Sent {
                        attempt,
                        agent_state,
                    } => {
                        crate::output::insert("delivery", "sent");
                        crate::output::insert("attempt", attempt);
                        crate::output::insert("agent_state", agent_state.clone());
                        println!("sent to {id} (agent was {agent_state})");
                    }
                }
                if let Some(event) = seal {
                    crate::output::insert("held_seal", event.id.clone());
                    println!(
                        "{id} sealed {}; this follow-up holds that seal until the lane is idle again. It's restored if nothing changes; a new commit or report needs a new `ha done`.",
                        event.id
                    );
                }
                Ok(())
            }
            ThreadCommand::Adopt {
                slug,
                pane,
                title,
                task_file,
                workflow,
                passive,
            } => {
                let task = task_file.map(|file| read_text(&file)).transpose()?;
                let thread = adopt::adopt(
                    &ctx,
                    &slug,
                    &pane,
                    &title,
                    task,
                    adopt::AdeAdopt { workflow, passive },
                )?;
                ThreadResult::Adopt(&thread).emit()
            }
            ThreadCommand::List { slug } => threads::print_list(&ctx, &slug),
            ThreadCommand::Show { slug, id } => {
                let project = Project::load(&ctx.root, &slug)?;
                let record = crate::thread::load(&project, &id)?;
                crate::output::insert("record", serde_json::to_value(&record)?);
                if let Some(attestation) = threads::done_attestation(&project, &record) {
                    crate::output::insert("attestation", serde_json::to_value(attestation)?);
                }
                threads::print_show(&ctx, &slug, &id)
            }
            ThreadCommand::Ack { slug, id } => threads::ack(&ctx, &slug, &id),
            ThreadCommand::Resolve {
                slug,
                id,
                reopen,
                skip_copy,
                discard_uncopied,
                keep_pane,
            } => {
                let result = threads::resolve(
                    &ctx,
                    &slug,
                    &id,
                    &ResolveArgs {
                        reopen,
                        skip_copy,
                        discard_uncopied,
                        keep_pane,
                    },
                )?;
                crate::output::success(
                    Some(if reopen { "reopened" } else { "resolved" }),
                    &serde_json::to_value(&result)?,
                    &result.message(&slug),
                    "",
                )
            }
        },
        Command::Pause { slug } => lifecycle::set_status(&ctx, &slug, Status::Paused),
        Command::Resume { slug } => {
            if Project::load(&ctx.root, &slug)?.status() == Status::Archived {
                bail!("`{slug}` is archived; use `unarchive`");
            }
            lifecycle::set_status(&ctx, &slug, Status::Active)
        }
        Command::Archive { slug } => lifecycle::set_status(&ctx, &slug, Status::Archived),
        Command::Unarchive { slug } => lifecycle::set_status(&ctx, &slug, Status::Active),
        Command::Delete {
            slug,
            github,
            preview,
            cancel,
        } => {
            if cancel {
                lifecycle::cancel_delete(&ctx, &slug)
            } else {
                lifecycle::delete(&ctx, &slug, github, preview)
            }
        }
        Command::AdoptWorkspace {
            name,
            goal,
            pane,
            workspace_cwd,
            session,
        } => adopt::adopt_workspace(
            &ctx,
            &adopt::AdoptWorkspace {
                name,
                goal,
                pane,
                workspace_cwd,
                session: session.into(),
            },
        ),
        Command::Action { id } => actions::run_action(&ctx, &id),
        Command::Recover => crate::ops::recover_box(&ctx),
        Command::Pane { id } => actions::run_pane(&ctx, &id),
        Command::Done { report, sha } => crate::lane::done(&ctx, report.as_deref(), sha.as_deref()),
        Command::Waiting { what } => crate::lane::waiting(&ctx, &what),
        Command::Failed {
            class,
            provider_kind,
            what,
        } => crate::lane::failed_class(&ctx, &what, class, provider_kind.as_deref()),
        Command::Skill { role } => crate::lane::skill(&ctx, &role),
        Command::Close { slug } => crate::coordinator::close(&ctx, &slug),
        Command::Hook {
            kind,
            project,
            binding,
            phase,
        } => crate::hook::run(&ctx, &kind, &project, &binding, &phase),
        Command::Doctor {
            session,
            timings,
            remove_kept_worktree,
        } => {
            if let Some(target) = remove_kept_worktree {
                let (slug, id) = target
                    .split_once('/')
                    .ok_or_else(|| anyhow::anyhow!("expected PROJECT/THREAD"))?;
                println!("{}", crate::threads::remove_kept_worktree(&ctx, slug, id)?);
                return Ok(());
            }
            let result = doctor::run_timed_from(&ctx, &session.into(), timings, cli_started)?;
            doctor::finish(&ctx, &result)
        }
        Command::Plan { command } => run_plan_command(&ctx, command),
        Command::Review {
            slug,
            repo,
            command,
        } => {
            let opted_in = command.is_none();
            let record = match command {
                Some(ReviewCommand::Cancel { slug, repo }) => {
                    crate::review::cancel(&ctx, &slug, repo.as_deref())?;
                    None
                }
                Some(ReviewCommand::Retry { slug, repo }) => {
                    crate::review::retry(&ctx, &slug, repo.as_deref())?
                }
                None => crate::review::start(
                    &ctx,
                    slug.as_deref().context("project required")?,
                    repo.as_deref(),
                )?,
            };
            let message = record
                .as_ref()
                .map(|r| {
                    format!(
                        "{}: {:?} ({} lanes) — {}{}\n",
                        r.id,
                        r.phase,
                        r.members.len(),
                        r.landing_summary(),
                        r.gates_summary()
                    )
                })
                .unwrap_or_else(|| {
                    if opted_in {
                        "no ready pile; automatic reviews enabled for this project\n".into()
                    } else {
                        "no pile review running\n".into()
                    }
                });
            crate::output::success(
                Some("review"),
                &serde_json::json!({"review": record}),
                &message,
                "",
            )
        }
        Command::Harness { command } => match command {
            HarnessCommand::Journey { review } => {
                let review = review
                    .as_deref()
                    .map(|value| value.split_once('/').context("expected PROJECT/REVIEW"))
                    .transpose()?;
                crate::journey::run(&ctx, review)
            }
            HarnessCommand::Install => {
                let result = crate::harness::install(&ctx)?;
                let failed = result.box_failed();
                crate::output::success(
                    Some(if failed {
                        "install_failed"
                    } else {
                        "installed"
                    }),
                    &serde_json::json!({ "install": result }),
                    &result.message(),
                    &result.warnings(),
                )?;
                if failed {
                    bail!(
                        "harness_box_pending: one or more boxes did not install; see per-box results"
                    );
                }
                Ok(())
            }
        },
        Command::Ticker { command } => match command {
            TickerCommand::Start => ticker::start(&ctx),
            TickerCommand::Ensure => ticker::ensure(&ctx),
            TickerCommand::Run => ticker::run(&ctx),
            TickerCommand::Stop => ticker::stop(&ctx.root),
            TickerCommand::Status => ticker::status(&ctx.root),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn result(fx: &crate::testkit::Fx, args: &[&str]) -> serde_json::Value {
        let matches = Cli::command()
            .try_get_matches_from(std::iter::once("ha").chain(args.iter().copied()))
            .unwrap();
        let (command, data) = machine_command(&matches);
        crate::output::begin(true, command.clone(), machine_outcome(&command), data);
        dispatch_with_start(
            fx.world.ctx(),
            Cli::from_arg_matches(&matches).unwrap().command,
            None,
        )
        .unwrap();
        crate::output::captured_success()
    }

    #[test]
    fn plan_mutations_share_the_same_receipt_and_sentence() {
        let fx = crate::testkit::fixture();
        result(&fx, &["plan", "set", "demo", "--does", "Deliver it"]);
        let cases: &[(&[&str], &str, &str)] = &[
            (&["add", "demo", "First"], "added", "s-1"),
            (&["add", "demo", "Second"], "added", "s-2"),
            (&["add", "demo", "Child", "--under", "s-1"], "added", "s-3"),
            (
                &["edit", "demo", "s-3", "Changed", "--expect", "4"],
                "edited",
                "s-3",
            ),
            (&["link", "demo", "s-3", "--after", "s-2"], "linked", "s-3"),
            (
                &[
                    "unlink",
                    "demo",
                    "s-3",
                    "--after",
                    "s-2",
                    "--reason",
                    "Not needed",
                ],
                "unlinked",
                "s-3",
            ),
            (&["move", "demo", "s-2", "--before", "s-1"], "moved", "s-2"),
            (&["remove", "demo", "s-3"], "removed", "s-3"),
        ];
        for (index, (args, operation, id)) in cases.iter().enumerate() {
            let args: Vec<_> = ["plan", "step"]
                .into_iter()
                .chain(args.iter().copied())
                .collect();
            let receipt = result(&fx, &args);
            let data = &receipt["data"];
            assert_eq!(receipt["outcome"], "plan_changed");
            assert!(receipt.get("reason").is_none());
            assert_eq!(data["operation"], *operation);
            assert_eq!(data["step"], *id);
            assert_eq!(data["revision"], index + 2);
            assert_eq!(data["plan"]["revision"], data["revision"]);
            assert_eq!(
                receipt["message"],
                format!("plan revision {}: {operation} {id}\n", index + 2)
            );
            if *operation == "removed" {
                assert!(data.get("record").is_none());
            } else {
                assert_eq!(data["record"]["id"], *id);
            }
            if *operation == "moved" {
                assert_eq!(data["before"], "s-1");
            } else {
                assert!(data.get("before").is_none());
            }
        }
        let shown = result(&fx, &["plan", "show", "demo"]);
        assert_eq!(shown["data"]["result"]["revision"], 9);
        assert_eq!(shown["data"]["result"]["steps"][0]["id"], "s-2");
    }

    #[test]
    fn thread_start_and_adopt_render_their_facts_without_reconstruction() {
        use crate::thread::{Kind, Status, Thread};
        let mut thread = Thread {
            id: "t-1".into(),
            status: Status::Open,
            kind: Kind::Worktree,
            branch: "lane/1".into(),
            pane_id: "w1:p1".into(),
            placement_reason: "selected".into(),
            ..Thread::default()
        };
        for (machine, task, note) in [
            ("", "", None),
            ("oci", "job-1", Some("brief pending".to_string())),
        ] {
            thread.machine = machine.into();
            crate::output::begin(
                true,
                "thread start".into(),
                "started".into(),
                BTreeMap::new(),
            );
            ThreadResult::Start {
                thread: &thread,
                task,
                note: note.clone(),
            }
            .emit()
            .unwrap();
            let receipt = crate::output::captured_success();
            let facts = serde_json::json!({"id":"t-1", "state":"open", "kind":"worktree", "branch":"lane/1", "pane_id":"w1:p1", "machine":if machine.is_empty() { "local" } else { machine }, "placement_reason":"selected"});
            let mut data = facts.clone();
            if !task.is_empty() {
                data["task"] = task.into();
            }
            if let Some(note) = &note {
                data["note"] = note.clone().into();
            }
            assert_eq!(receipt["data"], data);
            assert_eq!(
                receipt["message"],
                format!(
                    "{facts}\n{}",
                    note.map(|note| format!("{note}\n")).unwrap_or_default()
                )
            );
            assert_eq!(receipt["outcome"], "started");
        }
        thread.kind = Kind::Adopted;
        thread.prompt_pending = true;
        crate::output::begin(
            true,
            "thread adopt".into(),
            "adopted".into(),
            BTreeMap::new(),
        );
        ThreadResult::Adopt(&thread).emit().unwrap();
        let receipt = crate::output::captured_success();
        let data = serde_json::json!({"id":"t-1", "kind":"adopted", "pane_id":"w1:p1", "prompt_pending":true});
        assert_eq!(receipt["data"], data);
        assert_eq!(receipt["message"], format!("{data}\n"));
        assert_eq!(receipt["outcome"], "adopted");
    }

    #[test]
    fn task_show_keeps_sealed_and_historical_reports_distinct() {
        let fx = crate::testkit::fixture();
        let (historical, _) = fx.lane(1);
        let (sealed, sha) = fx.lane(2);
        let path = crate::thread::home_report_path(&fx.project, &historical);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "old report").unwrap();
        fx.seal_done(&sealed, 1, 1, &sha, "final report");
        let task = crate::task::Task {
            id: "job-0001".into(),
            title: "Reports".into(),
            authority: vec!["request:historical".into()],
            acceptance: vec!["Reports remain readable".into()],
            attempts: vec![historical.clone(), sealed.clone()],
            ..crate::task::Task::default()
        };
        let dir = fx.project.state_dir().join("tasks");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("job-0001.toml"), toml::to_string(&task).unwrap()).unwrap();
        let receipt = result(&fx, &["task", "show", "demo", "job-0001"]);
        assert_eq!(receipt["outcome"], "shown");
        assert!(receipt["data"]["attestation"].is_null());
        let reports = receipt["data"]["reports"].as_array().unwrap();
        assert_eq!(reports.len(), 2);
        for (report, id, sealed) in [
            (&reports[0], historical, false),
            (&reports[1], sealed, true),
        ] {
            assert_eq!(report["thread"], id);
            assert_eq!(report["sealed"], sealed);
            let label = if sealed {
                "final report"
            } else {
                "historical report (not completion)"
            };
            assert!(receipt["message"].as_str().unwrap().contains(&format!(
                "{label} ({id}): {}\n",
                report["path"].as_str().unwrap()
            )));
        }
    }

    #[test]
    fn lane_description_is_the_title_and_flags_match_other_actions() {
        let adopted = Cli::try_parse_from([
            "ha",
            "thread",
            "adopt",
            "demo",
            "--pane",
            "w1:p1",
            "--title",
            "Repair startup",
            "--workflow",
            "critic",
        ])
        .unwrap();
        assert!(
            matches!(adopted.command, Command::Thread { command: ThreadCommand::Adopt { workflow: Some(ref flow), .. } } if flow == "critic")
        );
        let unlinked = Cli::try_parse_from([
            "ha", "plan", "step", "unlink", "demo", "s-1", "--task", "job-1", "--reason",
            "Replaced",
        ])
        .unwrap();
        assert!(
            matches!(unlinked.command, Command::Plan { command: PlanCommand::Step { command: PlanStepCommand::Unlink { ref reason, .. } } } if reason == "Replaced")
        );
        let plan = Cli::try_parse_from([
            "ha",
            "plan",
            "set",
            "demo",
            "--does",
            "Compare red.md and blue.md",
        ])
        .unwrap();
        assert!(matches!(plan.command,
            Command::Plan { command: PlanCommand::Set { ref does, .. } }
            if does == "Compare red.md and blue.md"));
        for command in [
            "ha plan set demo --kind screen --does Outcome",
            "ha thread start demo --task-file - --plain Extra",
            "ha thread adopt demo --pane w1:p1 --title Repair --plain Extra",
            "ha thread adopt demo --pane w1:p1 --title Repair --role critic",
            "ha plan step unlink demo s-1 --task job-1 --why Replaced",
        ] {
            assert!(
                Cli::try_parse_from(command.split_whitespace()).is_err(),
                "{command}"
            );
        }
    }

    #[test]
    fn start_accepts_repeatable_named_attachments_and_paths() {
        let cli = Cli::try_parse_from([
            "ha",
            "thread",
            "start",
            "demo",
            "--job",
            "job-0001",
            "--task-file",
            "task.md",
            "--attach",
            "report.md",
            "--attach",
            "image.png",
            "--paths",
            "src/**",
            "--paths",
            "Cargo.?oml",
            "--json",
        ])
        .unwrap();
        let Command::Thread {
            command: ThreadCommand::Start { attach, paths, .. },
        } = cli.command
        else {
            panic!("start");
        };
        assert_eq!(attach, ["report.md", "image.png"]);
        assert_eq!(paths, ["src/**", "Cargo.?oml"]);
    }

    #[test]
    fn done_defaults_and_overrides_and_waiting_only_accepts_missing_input() {
        for (args, report, sha) in [
            (vec!["ha", "done"], None, None),
            (
                vec!["ha", "done", "--report", "report with spaces.md"],
                Some("report with spaces.md"),
                None,
            ),
            (vec!["ha", "done", "--sha", "abc"], None, Some("abc")),
            (
                vec!["ha", "done", "--report", "report.md", "--sha", "abc"],
                Some("report.md"),
                Some("abc"),
            ),
        ] {
            let cli = Cli::try_parse_from(args).unwrap();
            assert!(matches!(cli.command, Command::Done { report: r, sha: s }
                if r.as_deref() == report && s.as_deref() == sha));
        }
        let cli = Cli::try_parse_from(["ha", "waiting", "Need a design"]).unwrap();
        assert!(matches!(cli.command, Command::Waiting { what } if what == "Need a design"));
        for flag in ["--class", "--provider-kind"] {
            assert!(
                Cli::try_parse_from(["ha", "waiting", "Need a design", flag, "provider"]).is_err()
            );
        }
    }

    #[test]
    fn failed_defaults_to_the_work_failed_class() {
        let cli = Cli::try_parse_from(["herdr-ade", "failed", "the approach failed"]).unwrap();
        assert!(matches!(
            cli.command,
            Command::Failed {
                class: crate::contracts::FailureClass::WorkFailed,
                provider_kind: None,
                ..
            }
        ));
    }
}
