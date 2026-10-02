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
    /// Print threads grouped by what needs you
    Overview {
        #[arg(value_name = "PROJECT")]
        slug: String,
        /// Include resolved thread history
        #[arg(long)]
        history: bool,
        /// Wait for Enter before exiting (only when on a terminal; used by the popup)
        #[arg(long)]
        wait: bool,
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
        #[arg(long)]
        preview: bool,
    },
    /// Continue the current workspace's agent pane as a new project
    AdoptWorkspace {
        /// Project name (default: the workspace label herdr passes to the action)
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "")]
        goal: String,
        /// The adopted thread's birth sentence
        #[arg(long)]
        plain: Option<String>,
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
    #[command(hide = true)]
    Event { id: String },
    /// Run inside a plugin popup pane
    #[command(hide = true)]
    Pane { id: String },
    /// Seal and deliver this lane's completion
    Done {
        #[arg(long, value_name = "PATH")]
        report: String,
        #[arg(long)]
        sha: String,
    },
    /// Seal and deliver why this lane must wait
    Waiting {
        #[arg(long, value_enum, default_value = "unknown")]
        class: crate::contracts::FailureClass,
        #[arg(long, requires = "class")]
        provider_kind: Option<String>,
        what: String,
    },
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
    /// Start or show the repository's pile review
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
    /// Ask Rolf a question with two to four choices he can picture
    #[command(
        args_conflicts_with_subcommands = true,
        allow_missing_positional = true
    )]
    Ask {
        /// Project slug (required when creating a question)
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        #[command(subcommand)]
        command: Option<AskCommand>,
        question: Option<String>,
        #[arg(long = "choice", value_name = "SENTENCE")]
        choices: Vec<String>,
        /// One sentence: what happened
        #[arg(long)]
        what: Option<String>,
        /// One sentence: what it means for Rolf
        #[arg(long)]
        means: Option<String>,
        /// Drop this ask when the task closes
        #[arg(long, value_name = "TASK_ID")]
        task: Option<String>,
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
}

#[derive(Subcommand)]
enum PlanCommand {
    /// Print the plan card; a missing card prints revision zero
    Show {
        /// Project slug
        #[arg(value_name = "PROJECT")]
        slug: String,
    },
    /// Set the end-result kind and its one sentence
    Set {
        /// Project slug
        #[arg(value_name = "PROJECT")]
        slug: String,
        /// End result: screen, command, background, document, picture, number, or finding
        #[arg(long, value_parser = ["screen", "command", "background", "document", "picture", "number", "finding"])]
        kind: String,
        /// One sentence about what this project delivers
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
        /// Project slug
        #[arg(value_name = "PROJECT")]
        slug: String,
        /// Step id
        id: String,
        /// Replacement text
        text: String,
        /// Expected plan revision; omitted uses the latest revision
        #[arg(long)]
        expect: Option<u64>,
    },
    /// Add required work to a step
    Link {
        /// Project slug
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        /// Stable task ids to link to this step
        #[arg(long = "task", value_name = "ID")]
        tasks: Vec<String>,
        #[arg(long, value_name = "STEP")]
        after: Vec<String>,
        /// Expected plan revision; omitted uses the latest revision
        #[arg(long)]
        expect: Option<u64>,
    },
    /// Remove required work from a step
    Unlink {
        /// Project slug
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        /// Stable task ids to unlink from this step
        #[arg(long = "task", value_name = "ID")]
        tasks: Vec<String>,
        #[arg(long, value_name = "STEP")]
        after: Vec<String>,
        /// Why this step or link is removed
        #[arg(long)]
        why: String,
        /// Expected plan revision; omitted uses the latest revision
        #[arg(long)]
        expect: Option<u64>,
    },
    /// Remove one step (identifiers are never reused)
    Remove {
        /// Project slug
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        /// Expected plan revision; omitted uses the latest revision
        #[arg(long)]
        expect: Option<u64>,
    },
    /// Change display order only
    Move {
        /// Project slug
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        /// Step id that should follow this one
        #[arg(long, value_name = "ID")]
        before: String,
        /// Expected plan revision; omitted uses the latest revision
        #[arg(long)]
        expect: Option<u64>,
    },
}

#[derive(Subcommand)]
enum AskCommand {
    /// Answer or withdraw an open ask
    Close {
        #[arg(value_name = "PROJECT")]
        slug: String,
        id: String,
        /// Choice number (0 means not understood), or exact choice sentence
        #[arg(long, conflicts_with = "withdraw")]
        choice: Option<String>,
        /// Reason for withdrawing instead of answering
        #[arg(long, conflicts_with = "choice")]
        withdraw: Option<String>,
    },
}

fn run_project_commands(ctx: &Ctx, command: Command) -> Result<()> {
    use crate::{ask, plan};
    match command {
        Command::Ask {
            slug,
            command,
            question,
            choices,
            what,
            means,
            task,
        } => match command {
            Some(AskCommand::Close {
                slug,
                id,
                choice: None,
                withdraw: Some(reason),
            }) => {
                let by = ctx
                    .env
                    .var("USER")
                    .context("USER is required to record who withdrew the ask")?;
                ask::withdraw(ctx, &slug, &id, &reason, by)?;
                crate::output::insert("ask", id.clone());
                println!("{id} withdrawn: {reason}");
                Ok(())
            }
            Some(AskCommand::Close {
                slug,
                id,
                choice: Some(choice),
                withdraw: None,
            }) => {
                let project = crate::project::Project::load(&ctx.root, &slug)?;
                let revision = ask::latest_revision(&project, &id);
                let answer = ask::answer_text(ctx, &slug, &id, revision, &choice, "command")?;
                crate::output::success(
                    Some("answered"),
                    &serde_json::json!({ "ask": id, "answer": answer }),
                    &format!(
                        "{} revision {}: {} ({})\n",
                        answer.id, answer.revision, answer.choice, answer.text
                    ),
                    "",
                )
            }
            None => {
                let slug = slug.context("a project slug is required")?;
                let question = question.context("a question is required")?;
                let a = ask::ask(
                    ctx,
                    &slug,
                    ask::NewAsk {
                        question,
                        choices,
                        what,
                        means,
                        task,
                    },
                )?;
                crate::output::insert("ask", a.id.clone());
                println!(
                    "{} revision {}: {}",
                    a.id,
                    a.revision,
                    ask::numbered(&a)
                        .split_whitespace()
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                Ok(())
            }
            Some(AskCommand::Close { .. }) => anyhow::bail!("pass --choice or --withdraw"),
        },
        Command::Plan { command } => match command {
            PlanCommand::Show { slug } => {
                crate::review::classify_old_seals(ctx, &Project::load(&ctx.root, &slug)?, true)?;
                let text = plan::show(ctx, &slug, false)?;
                if crate::output::structured() {
                    let value: serde_json::Value =
                        serde_json::from_str(&plan::show(ctx, &slug, true)?)?;
                    crate::output::insert("result", value);
                }
                print!("{text}");
                Ok(())
            }
            PlanCommand::Set {
                slug,
                kind,
                does,
                expect,
            } => {
                let p = plan::set(ctx, &slug, &kind, &does, expect)?;
                crate::output::insert("revision", p.revision);
                println!("plan revision {} set to `{}`", p.revision, p.kind);
                Ok(())
            }
            PlanCommand::Step { command } => match command {
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
                    crate::output::success(
                        None,
                        &serde_json::json!({
                            "operation": "added",
                            "revision": p.revision,
                            "step": id,
                            "record": plan::all_steps(&p).find(|step| step.id == id),
                            "plan": p,
                        }),
                        &format!("plan revision {}: added {id}\n", p.revision),
                        "",
                    )
                }
                PlanStepCommand::Edit {
                    slug,
                    id,
                    text,
                    expect,
                } => {
                    let p = plan::step_edit(ctx, &slug, &id, &text, expect)?;
                    crate::output::success(
                        None,
                        &serde_json::json!({
                            "operation": "edited",
                            "revision": p.revision,
                            "step": id,
                            "record": plan::all_steps(&p).find(|step| step.id == id),
                            "plan": p,
                        }),
                        &format!("plan revision {}: edited {id}\n", p.revision),
                        "",
                    )
                }
                PlanStepCommand::Link {
                    slug,
                    id,
                    tasks,
                    after,
                    expect,
                } => {
                    let p = plan::step_link(ctx, &slug, &id, tasks, after, expect)?;
                    crate::output::success(
                        None,
                        &serde_json::json!({
                            "operation": "linked",
                            "revision": p.revision,
                            "step": id,
                            "record": plan::all_steps(&p).find(|step| step.id == id),
                            "plan": p,
                        }),
                        &format!("plan revision {}: linked {id}\n", p.revision),
                        "",
                    )
                }
                PlanStepCommand::Unlink {
                    slug,
                    id,
                    tasks,
                    after,
                    why,
                    expect,
                } => {
                    let p = plan::step_unlink(ctx, &slug, &id, tasks, after, &why, expect)?;
                    crate::output::success(
                        None,
                        &serde_json::json!({
                            "operation": "unlinked",
                            "revision": p.revision,
                            "step": id,
                            "record": plan::all_steps(&p).find(|step| step.id == id),
                            "plan": p,
                        }),
                        &format!("plan revision {}: unlinked {id}\n", p.revision),
                        "",
                    )
                }
                PlanStepCommand::Remove { slug, id, expect } => {
                    let p = plan::step_remove(ctx, &slug, &id, expect)?;
                    crate::output::success(
                        None,
                        &serde_json::json!({
                            "operation": "removed",
                            "revision": p.revision,
                            "step": id,
                            "plan": p,
                        }),
                        &format!("plan revision {}: removed {id}\n", p.revision),
                        "",
                    )
                }
                PlanStepCommand::Move {
                    slug,
                    id,
                    before,
                    expect,
                } => {
                    let p = plan::step_move(ctx, &slug, &id, &before, expect)?;
                    crate::output::success(
                        None,
                        &serde_json::json!({
                            "operation": "moved",
                            "revision": p.revision,
                            "step": id,
                            "before": before,
                            "record": plan::all_steps(&p).find(|step| step.id == id),
                            "plan": p,
                        }),
                        &format!("plan revision {}: moved {id}\n", p.revision),
                        "",
                    )
                }
            },
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
                            crate::output::insert(
                                "failed_check_holds",
                                serde_json::to_value(&holds)?,
                            );
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
        },
        _ => unreachable!("run_project_commands only receives project commands"),
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
        /// The integration branch the brief is committed on (default: the checked-out branch)
        #[arg(long, value_name = "BRANCH")]
        base: Option<String>,
        /// The task; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        task_file: String,
        /// Instruction set for this lane; the routing table may match it
        #[arg(long, value_name = "FLOW")]
        workflow: Option<String>,
        /// Exact configured recipe for this one lane (the coordinator's choice)
        #[arg(long, value_name = "ID")]
        recipe: Option<String>,
        /// Birth sentence; defaults to the stable task's title
        #[arg(long)]
        plain: Option<String>,
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
    /// Replace a failed, blocked, or stuck attempt through bounded routing
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
        /// Birth sentence
        #[arg(long)]
        plain: Option<String>,
        #[arg(long, value_name = "ROLE")]
        role: Option<String>,
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
    plain: Option<String>,
) -> Result<(String, Option<String>, String)> {
    let title = title
        .or_else(|| existing.map(|task| task.title.clone()))
        .ok_or_else(|| {
            crate::refusal::error(
                "task_title: --title is required when the lane does not name an existing --job", "ha thread start <project> --title \"<title>\" --request <request-id> --acceptance \"<condition>\" --task-file <file>")
        })?;
    let repo = repo.or_else(|| existing.and_then(|task| task.repo.clone()));
    let plain = plain.unwrap_or_else(|| {
        existing
            .map(|task| task.title.clone())
            .unwrap_or_else(|| title.clone())
    });
    Ok((title, repo, plain))
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
        "ask" => "asked",
        "ask close" => "closed",
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
        Err(error) if !wants_json => error.exit(),
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
    // A running herdr server may still fire the old manifest until reload.
    if matches!(&cli.command, Command::Event { id } if id == "review-advance") {
        return Ok(crate::output::finish_success()?);
    }
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
    let result = dispatch_with_start(ctx, cli.command, Some(cli_started));
    record_command_outcome(observed_project.as_ref(), &result);
    if result.is_ok() {
        crate::output::finish_success()?;
    }
    result
}

/// Keep refusal and error outcomes distinct without filing a separate failure.
fn record_command_outcome(project: Option<&Project>, result: &Result<()>) {
    match result {
        Err(error) if crate::refusal::is(error) => {
            if let Some(next) = crate::refusal::next(error) {
                crate::output::set_next(next);
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

fn dispatch_with_start(
    ctx: Ctx<'_>,
    command: Command,
    cli_started: Option<std::time::Instant>,
) -> Result<()> {
    match command {
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
        Command::Overview {
            slug,
            history,
            wait,
        } => overview::run(&ctx, Some(&slug), history, wait),
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
                let attestation = crate::task::attestation(&project, &view.record);
                let reports: Vec<_> = view
                    .record
                    .attempts
                    .iter()
                    .filter_map(|id| {
                        let thread = crate::thread::load(&project, id).ok()?;
                        let path = crate::thread::report_reference(&project, &thread)?;
                        let sealed = crate::thread::sealed_report_path(&project, &thread).is_some();
                        Some(serde_json::json!({ "thread": id, "path": path, "sealed": sealed }))
                    })
                    .collect();
                let mut message = crate::task::render(&project, &view);
                for report in &reports {
                    message.push_str(&format!(
                        "{} ({}): {}\n",
                        if report["sealed"].as_bool().unwrap_or(false) {
                            "final report"
                        } else {
                            "historical report (not completion)"
                        },
                        report["thread"].as_str().unwrap_or_default(),
                        report["path"].as_str().unwrap_or_default()
                    ));
                }
                if let Some(attestation) = &attestation {
                    message.push_str(&format!(
                        "attested: {}: {}\n",
                        attestation.coordinator, attestation.reason
                    ));
                }
                crate::output::success(
                    Some("shown"),
                    &serde_json::json!({ "task": view, "reports": reports, "attestation": attestation }),
                    &message,
                    "",
                )
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
                    .map(|view| crate::task::render_list(&project, view))
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
                    let record = crate::task::drop_task(&project, &id, &reason)?;
                    let view = crate::task::view(&project, record);
                    crate::output::success(
                        Some("dropped"),
                        &serde_json::json!({ "task": view }),
                        &format!("{} dropped: {}\n", view.record.id, reason.trim()),
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
                workflow,
                recipe,
                plain,
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
                let (title, repo, plain) = start_details(existing.as_ref(), title, repo, plain)?;
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
                        plain,
                        workflow,
                        recipe,
                        task_id: task_id.clone(),
                        review_id: String::new(),
                    },
                )?;
                if !task_id.is_empty() {
                    crate::output::insert("task", task_id);
                }
                crate::output::insert("id", thread.id.clone());
                crate::output::insert("kind", serde_json::to_value(thread.kind)?);
                crate::output::insert("branch", thread.branch.clone());
                crate::output::insert("pane_id", thread.pane_id.clone());
                let machine = if thread.machine.is_empty() {
                    "local"
                } else {
                    &thread.machine
                };
                crate::output::insert("machine", machine);
                crate::output::insert("placement_reason", thread.placement_reason.clone());
                let mut note = mac_only_brief_note(&ctx.config_dir, &project, machine, &brief_text);
                if thread.prompt_pending && !thread.pane_id.is_empty() {
                    let pending = "brief pending; the ticker delivers it when the agent registers";
                    note = Some(match note {
                        Some(other) => format!("{pending}\n{other}"),
                        None => pending.to_string(),
                    });
                }
                if let Some(note) = &note {
                    crate::output::insert("note", note.clone());
                }
                let result = serde_json::json!({ "id": thread.id, "kind": thread.kind, "branch": thread.branch, "pane_id": thread.pane_id, "machine": machine, "placement_reason": thread.placement_reason });
                println!("{result}");
                if let Some(note) = note {
                    println!("{note}");
                }
                Ok(())
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
                let launch = if current.prompt_pending {
                    "startup is still pending; the ticker resumes it"
                } else {
                    "its brief was delivered"
                };
                crate::output::success(
                    Some("retried"),
                    &result,
                    &format!(
                        "{} attempt {} is in pane {}; {launch}{}\n{}",
                        result.thread,
                        result.attempt,
                        result.pane_id,
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
                plain,
                role,
                passive,
            } => {
                let task = task_file.map(|file| read_text(&file)).transpose()?;
                let thread = adopt::adopt(
                    &ctx,
                    &slug,
                    &pane,
                    &title,
                    task,
                    adopt::AdeAdopt {
                        plain: plain.unwrap_or_default(),
                        role,
                        passive,
                    },
                )?;
                crate::output::insert("id", thread.id.clone());
                crate::output::insert("kind", serde_json::to_value(thread.kind)?);
                crate::output::insert("pane_id", thread.pane_id.clone());
                crate::output::insert("prompt_pending", thread.prompt_pending);
                println!(
                    "{}",
                    serde_json::json!({ "id": thread.id, "kind": thread.kind, "pane_id": thread.pane_id, "prompt_pending": thread.prompt_pending })
                );
                Ok(())
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
        } => lifecycle::delete(&ctx, &slug, github, preview),
        Command::AdoptWorkspace {
            name,
            goal,
            plain,
            pane,
            workspace_cwd,
            session,
        } => adopt::adopt_workspace(
            &ctx,
            &adopt::AdoptWorkspace {
                name,
                goal,
                plain: plain.unwrap_or_default(),
                pane,
                workspace_cwd,
                session: session.into(),
            },
        ),
        Command::Action { id } => actions::run_action(&ctx, &id),
        Command::Recover => crate::ops::recover_box(&ctx),
        Command::Pane { id } => actions::run_pane(&ctx, &id),
        Command::Event { id } if id == "review-advance" => Ok(()),
        Command::Event { id } => bail!("unknown plugin event `{id}`"),
        Command::Done { report, sha } => crate::lane::done(&ctx, &report, &sha),
        Command::Waiting {
            class,
            provider_kind,
            what,
        } => crate::lane::waiting_class(&ctx, &what, class, provider_kind.as_deref()),
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
            crate::output::success(
                Some(if result.healthy {
                    "healthy"
                } else {
                    "unhealthy"
                }),
                &result,
                &result.message,
                "",
            )?;
            if !result.healthy {
                return Err(crate::refusal::error(
                    "some checks failed",
                    "ha doctor --timings (inspect failed checks, fix them, then rerun)",
                ));
            }
            Ok(())
        }
        command @ (Command::Ask { .. } | Command::Plan { .. }) => {
            run_project_commands(&ctx, command)
        }
        Command::Review {
            slug,
            repo,
            command,
        } => {
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
                        "{}: {:?} ({} lanes){}\n",
                        r.id,
                        r.phase,
                        r.members.len(),
                        r.gates_summary()
                    )
                })
                .unwrap_or_else(|| "no pile review running\n".into());
            crate::output::success(
                Some("review"),
                &serde_json::json!({"review": record}),
                &message,
                "",
            )
        }
        Command::Harness { command } => match command {
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

    #[test]
    fn remote_brief_note_skips_mapped_repos_and_local_starts() {
        let root = tempfile::tempdir().unwrap();
        let project = crate::project::create(
            root.path(),
            "demo",
            "goal",
            vec![crate::project::Repo {
                path: "/home/agent/projects/demo".into(),
                box_path: Some("/home/ubuntu/projects/demo".into()),
                ..Default::default()
            }],
        )
        .unwrap();
        let brief = "Read `/home/agent/projects/demo/src/main.rs` and `/private/tmp/audit.md`.";
        assert!(mac_only_brief_note(root.path(), &project, "local", brief).is_none());
        assert_eq!(
            mac_only_brief_note(root.path(), &project, "oci", brief).unwrap(),
            "note: brief names Mac-only paths the lane on oci can't read: /private/tmp/audit.md. Paste their text into the brief, or start with --machine local."
        );
    }

    #[test]
    fn mac_brief_paths_are_distinct_and_limited_to_three() {
        let mapped = [crate::project::Repo {
            path: "/home/agent/projects/demo".into(),
            box_path: Some("/box/demo".into()),
            ..Default::default()
        }];
        assert_eq!(
            mac_only_paths(
                "`/home/agent/projects/demo/a` `/home/agent/projects/demo-other/a` /tmp/a /tmp/a /private/tmp/b /home/agent/file",
                &mapped
            ),
            [
                "/home/agent/projects/demo-other/a",
                "/tmp/a",
                "/private/tmp/b"
            ]
        );
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

    #[test]
    fn thread_start_reuses_task_details_and_keeps_explicit_overrides() {
        let task = crate::task::Task {
            title: "Build the short guide.".into(),
            repo: Some("/code/project".into()),
            ..crate::task::Task::default()
        };
        assert_eq!(
            start_details(Some(&task), None, None, None).unwrap(),
            (
                "Build the short guide.".into(),
                Some("/code/project".into()),
                "Build the short guide.".into(),
            )
        );
        assert_eq!(
            start_details(
                Some(&task),
                Some("Review the guide.".into()),
                Some("/code/other".into()),
                Some("Check the changed guide.".into()),
            )
            .unwrap(),
            (
                "Review the guide.".into(),
                Some("/code/other".into()),
                "Check the changed guide.".into(),
            )
        );
        assert!(start_details(None, None, None, None).is_err());

        assert!(
            Cli::try_parse_from([
                "herdr-ade",
                "thread",
                "start",
                "demo",
                "--job",
                "job-0001",
                "--task-file",
                "brief.md",
            ])
            .is_ok()
        );
        let start = [
            "herdr-ade",
            "thread",
            "start",
            "demo",
            "--job",
            "job-0001",
            "--task-file",
            "brief.md",
            "--recipe",
            "test_claude",
        ];
        assert!(Cli::try_parse_from(start).is_ok());
        let mut with_basis = start.to_vec();
        with_basis.extend(["--basis", "old quote"]);
        assert!(Cli::try_parse_from(with_basis).is_err());
    }

    #[test]
    fn enum_flags_name_values_in_help_and_errors() {
        for (path, flag, values) in [
            (
                &["plan", "set"][..],
                "--kind",
                "screen, command, background, document, picture, number, finding",
            ),
            (&["note", "add"][..], "--kind", "memory, instruction"),
            (
                &["failed"][..],
                "--class",
                "provider, lost_connection, process_gone, work_failed, unknown",
            ),
        ] {
            let mut help = vec!["herdr-ade"];
            help.extend(path);
            help.push("--help");
            let rendered = Cli::try_parse_from(help).err().unwrap().to_string();
            assert!(
                rendered.contains("[possible values:"),
                "{path:?}: {rendered}"
            );
            for value in values.split(", ") {
                assert!(rendered.contains(value), "{path:?}: {value}");
            }
            let mut invalid = vec!["herdr-ade"];
            invalid.extend(path);
            invalid.extend([flag, "not-a-valid-value"]);
            let error = Cli::try_parse_from(invalid).err().unwrap().to_string();
            assert!(error.contains("[possible values:"), "{path:?}: {error}");
            for value in values.split(", ") {
                assert!(error.contains(value), "{path:?}: {value}");
            }
        }
    }

    #[test]
    fn project_positionals_use_project_in_every_command_usage() {
        fn visit(command: &clap::Command) {
            for arg in command.get_arguments().filter(|arg| arg.get_id() == "slug") {
                assert_eq!(
                    arg.get_value_names().unwrap()[0].as_str(),
                    "PROJECT",
                    "{}",
                    command.get_name()
                );
            }
            for subcommand in command.get_subcommands() {
                visit(subcommand);
            }
        }
        visit(&Cli::command());
        assert!(Cli::try_parse_from(["herdr-ade", "inbox", "list", "demo"]).is_ok());
        assert!(
            Cli::try_parse_from(["herdr-ade", "inbox", "done", "demo", "--kind", "note"]).is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "herdr-ade",
                "note",
                "retire",
                "demo",
                "n-0001",
                "--request",
                "q-1",
                "--reason",
                "Outdated."
            ])
            .is_ok()
        );
        assert!(
            Cli::try_parse_from([
                "herdr-ade",
                "ask",
                "close",
                "demo",
                "a-1",
                "--choice",
                "Take the first option."
            ])
            .is_ok()
        );
    }

    #[test]
    fn project_scoped_commands_take_the_slug_as_a_positional() {
        let cases: &[&[&str]] = &[
            &["plan", "show", "demo"],
            &[
                "plan",
                "set",
                "demo",
                "--kind",
                "screen",
                "--does",
                "Show the result.",
                "--expect",
                "0",
            ],
            &["plan", "step", "add", "demo", "Ship it.", "--expect", "0"],
            &[
                "plan",
                "step",
                "edit",
                "demo",
                "s-1",
                "Ship it now.",
                "--expect",
                "1",
            ],
            &[
                "plan", "step", "link", "demo", "s-1", "--task", "job-0001", "--expect", "1",
            ],
            &[
                "plan",
                "step",
                "unlink",
                "demo",
                "s-1",
                "--task",
                "job-0001",
                "--why",
                "No longer needed.",
                "--expect",
                "1",
            ],
            &["plan", "step", "remove", "demo", "s-1", "--expect", "1"],
            &[
                "plan", "step", "move", "demo", "s-1", "--before", "s-2", "--expect", "1",
            ],
            &["plan", "sync", "demo"],
            &[
                "ask",
                "demo",
                "Can this run now?",
                "--choice",
                "Run it now.",
                "--choice",
                "Wait for later.",
            ],
            &[
                "ask",
                "close",
                "demo",
                "a-1",
                "--withdraw",
                "No longer needed.",
            ],
            &["ask", "close", "demo", "a-1", "--choice", "1"],
        ];
        for args in cases {
            let mut argv = vec!["herdr-ade"];
            argv.extend_from_slice(args);
            assert!(Cli::try_parse_from(argv).is_ok(), "{args:?}");
        }

        let refused_old_forms: &[&[&str]] = &[
            &["ask", "withdraw", "a-1", "No longer needed."],
            &[
                "plan",
                "step",
                "edit",
                "s-1",
                "Ship it now.",
                "--expect",
                "1",
            ],
            &["plan", "show"],
            &["overview"],
            &["say", "--what", "This was checked."],
            &["review"],
            &[
                "plan", "step", "link", "demo", "s-1", "--thread", "t-0001", "--expect", "1",
            ],
            &[
                "task",
                "add",
                "demo",
                "--title",
                "Do it.",
                "--request",
                "q-1",
                "--acceptance",
                "It works.",
                "--plan-step",
                "s-1",
            ],
        ];
        for args in refused_old_forms {
            let mut argv = vec!["herdr-ade"];
            argv.extend_from_slice(args);
            assert!(Cli::try_parse_from(argv).is_err(), "{args:?}");
        }

        assert!(Cli::try_parse_from(["herdr-ade", "plan", "show", "--project", "demo"]).is_err());
        assert!(Cli::try_parse_from(["herdr-ade", "event", "review-advance"]).is_ok());
    }
}
