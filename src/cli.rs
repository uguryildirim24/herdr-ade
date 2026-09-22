use std::collections::BTreeMap;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand, ValueEnum};

use crate::coordinator::{self, OpenOptions};
use crate::paths::{self, Ctx, Env, SessionFlags};
use crate::project::{self, Project, Status};
use crate::runner::RealRunner;
use crate::threads::{self, ResolveArgs, StartArgs};
use crate::{actions, adopt, doctor, inbox, lifecycle, overview, routine, ticker};

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

#[derive(Clone, Copy, Debug, ValueEnum)]
enum DecisionClass {
    WhatYouGet,
    Money,
    Undo,
    Routine,
}

impl DecisionClass {
    fn as_str(self) -> &'static str {
        match self {
            Self::WhatYouGet => "what-you-get",
            Self::Money => "money",
            Self::Undo => "undo",
            Self::Routine => "routine",
        }
    }

    fn needs_basis(self) -> bool {
        !matches!(self, Self::Routine)
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
        slug: String,
        /// Send the priming prompt again
        #[arg(long)]
        reprime: bool,
        /// Move the project to this session when its recorded socket no longer exists
        #[arg(long)]
        rebind: bool,
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Print the digest the coordinator reads at the start of every turn
    Context {
        slug: String,
        /// Print without recording the inbox items as seen
        #[arg(long)]
        peek: bool,
    },
    /// Print threads grouped by what needs you
    Overview {
        slug: Option<String>,
        /// Wait for Enter before exiting (only when on a terminal; used by the popup)
        #[arg(long)]
        wait: bool,
    },
    /// Show only one project's panes in the sidebar, sorted by attention
    Focus { slug: Option<String> },
    /// Clear the sidebar view (herdr holds one, so this clears any tool's view)
    Unfocus {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Observed harness failures in the current project
    Ledger {
        #[command(subcommand)]
        command: LedgerCommand,
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
    Thread {
        #[command(subcommand)]
        command: ThreadCommand,
    },
    /// Hold or release new box-lane starts on a saved machine (SPEC-remote §2.4)
    Machine {
        #[command(subcommand)]
        command: MachineCommand,
    },
    /// Routines: scheduled prompts and watched commands
    Routine {
        #[command(subcommand)]
        command: RoutineCommand,
    },
    /// Pause a project: the ticker skips it and `thread start` is refused
    Pause { slug: String },
    /// Make a paused project active again
    Resume { slug: String },
    /// Archive a project: paused, hidden, tokens cleared, `open` refused
    Archive { slug: String },
    /// Make an archived project active again
    Unarchive { slug: String },
    /// Delete a project everywhere; use `archive` to keep a reversible copy
    Delete {
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
        /// The adopted thread's birth sentence (SPEC-ADE D17 item 6)
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
    /// Run inside a plugin popup pane
    #[command(hide = true)]
    Pane { id: String },
    /// Safety settings
    Safety {
        #[command(subcommand)]
        command: SafetyCommand,
    },
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
    Close { slug: String },
    /// Plain-language checking and native correction hooks
    Plain {
        #[command(subcommand)]
        command: PlainCommand,
    },
    /// Check the setup: versions, tools, root, ticker and each project's session
    Doctor {
        #[command(flatten)]
        session: SessionArgs,
    },
    /// Rounds: open, admit lanes, review, merge with its checkpoint (SPEC-ADE D6)
    Round {
        #[command(subcommand)]
        command: RoundCommand,
    },
    /// Build and install the harness repositories after a merge
    Harness {
        #[command(subcommand)]
        command: HarnessCommand,
    },
    /// The spec dialogue between a drafter and a critic (SPEC-ADE D7)
    Dialogue {
        #[command(subcommand)]
        command: DialogueCommand,
    },
    /// Write HANDOFF.md and HANDOFF.json as one commit (the save-state port)
    Checkpoint {
        slug: String,
        /// The coordinator pane (default: $HERDR_PANE_ID, then the record)
        #[arg(long)]
        pane: Option<String>,
        #[arg(long, value_name = "DIR")]
        repo: Option<String>,
        #[arg(long)]
        branch: Option<String>,
        /// Print the generated Herdr section; write nothing
        #[arg(long, conflicts_with = "check")]
        print: bool,
        /// Check the current HANDOFF.md; write nothing
        #[arg(long)]
        check: bool,
    },
    /// Re-link live threads and print start lines for gone ones; --start restarts them
    Pickup {
        #[arg(
            value_name = "PROJECT",
            required_unless_present = "all",
            conflicts_with = "all"
        )]
        slug: Option<String>,
        /// Cover every active project under the root
        #[arg(long)]
        all: bool,
        /// Restart gone threads under a project whose start_threads is `auto`
        #[arg(long)]
        start: bool,
        #[arg(long)]
        pane: Option<String>,
        #[arg(long)]
        dry_run: bool,
    },
    /// Ask Rolf a question with two to four choices he can picture
    #[command(
        args_conflicts_with_subcommands = true,
        allow_missing_positional = true
    )]
    Ask {
        /// Project slug; omit to use the current project
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
        #[arg(long)]
        round: Option<String>,
        /// Ask an existing question again as its next revision
        #[arg(long, value_name = "ASK_ID")]
        reask: Option<String>,
    },
    /// The plan card: goal, end result and steps (SPEC-talk §6.5)
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
    /// The choices the coordinator made for Rolf (SPEC-talk §6.6)
    #[command(
        args_conflicts_with_subcommands = true,
        allow_missing_positional = true
    )]
    Decide {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        #[command(subcommand)]
        command: Option<DecideCommand>,
        line: Option<String>,
        #[arg(
            long,
            value_name = "CLASS",
            value_enum,
            long_help = "Choice class:\n  what-you-get  A choice about taste, direction, or the result Rolf gets; requires --basis.\n  money         A choice to spend money; requires --basis.\n  undo          A choice that cannot be undone; requires --basis.\n  routine       An ordinary choice with a sensible default or one that can be undone; no --basis required."
        )]
        class: Option<DecisionClass>,
        #[arg(long, value_name = "KEY")]
        key: Option<String>,
        /// Permission reference required by what-you-get, money, and undo
        #[arg(long, value_name = "REFERENCE")]
        basis: Option<String>,
        #[arg(long, value_name = "DECISION_ID")]
        replaces: Option<String>,
        #[arg(long, value_name = "REQUEST_ID")]
        request: Option<String>,
    },
    /// Tell Rolf one checked line on the board and the talk tab
    Say {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        #[arg(long)]
        what: String,
        #[arg(long)]
        means: Option<String>,
        /// Attach this line as landing evidence for a merged round
        #[arg(long, value_name = "ROUND")]
        landed_round: Option<String>,
    },
    /// Print a name's recorded sentence and where it was born
    #[command(allow_missing_positional = true)]
    Explain {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        name: String,
    },
    /// Glossary terms
    Term {
        #[command(subcommand)]
        command: TermCommand,
    },
    /// The talk tab: Rolf's checked conversation with the coordinator (SPEC-ADE D18)
    Talk {
        slug: String,
        /// Print the journal once and exit; sends nothing
        #[arg(long, conflicts_with_all = ["accepted", "open_tab"])]
        replay: bool,
        /// Record that the coordinator's turn ended after a submission (the correction hook)
        #[arg(long, hide = true, conflicts_with = "open_tab")]
        accepted: bool,
        /// Create the talk tab in the coordinator workspace when talk is on
        #[arg(long)]
        open_tab: bool,
    },
    /// Publish the board rows now, or print them
    Board {
        slug: String,
        /// Print one lane's board line (names its machine, SPEC-remote §5)
        #[arg(long, value_name = "THREAD")]
        thread: Option<String>,
        #[arg(long)]
        print: bool,
    },
    /// The background ticker
    Ticker {
        #[command(subcommand)]
        command: TickerCommand,
    },
}

#[derive(Subcommand)]
enum RoundCommand {
    /// Open a round: its record, gate list and policy hash
    Open {
        slug: String,
        round: String,
        #[arg(long)]
        branch: String,
        /// One sentence that says what the round does (required)
        #[arg(long)]
        plain: Option<String>,
        #[arg(long, value_name = "DIR")]
        repo: Option<String>,
    },
    /// Admit a lane to the round's manifest
    Admit {
        slug: String,
        round: String,
        thread: String,
    },
    /// Remove a lane from the round's manifest
    Remove {
        slug: String,
        round: String,
        thread: String,
    },
    /// Replace this round's reviewer attempt without starting a duplicate
    Retry {
        slug: String,
        round: String,
        #[arg(long)]
        reason: String,
    },
    /// Stop the round, its lanes and reviewer, and retry pending cleanup
    Cancel {
        slug: String,
        round: String,
        #[arg(long)]
        reason: String,
    },
    /// Bind an existing live reviewer thread to this round
    Rebind {
        slug: String,
        round: String,
        #[arg(long, value_name = "THREAD")]
        thread: String,
    },
    /// Accept a lane's sealed work or a reviewer's valid sealed verdict
    Adopt {
        slug: String,
        round: String,
        #[arg(long, value_name = "THREAD")]
        thread: String,
    },
    /// Manual repair only: commit the review brief B, freeze the manifest,
    /// create the review branch. `round advance` does this on its own.
    Review { slug: String, round: String },
    /// Merge on an exact MERGE verdict, then checkpoint; resumes after a crash
    Merge {
        slug: String,
        round: String,
        /// Test-only fault injection: stop after ref, merged, intent or commit
        #[arg(long, hide = true, value_name = "PHASE")]
        stop_after: Option<String>,
    },
    /// Start the review and reviewer for every ready round; announce verdicts
    Advance {
        /// Project slug; omit to use the hook's workspace
        slug: Option<String>,
    },
    /// Show one round's record and merge phase; omit ROUND to list all rounds
    Show { slug: String, round: Option<String> },
    /// Run the ticker's pass for rounds, asks, talk and the board once
    Tick { slug: String },
}

#[derive(Subcommand)]
enum HarnessCommand {
    /// Build every repository in `[harness]` and install it, then the saved box
    Install,
}

#[derive(Subcommand)]
enum DialogueCommand {
    /// Record a dialogue and print the lines that start its two sides
    Start {
        slug: String,
        topic: String,
        #[arg(long)]
        drafter: String,
        /// A role, or `pro`
        #[arg(long)]
        critic: String,
        #[arg(long)]
        plain: Option<String>,
        #[arg(long, value_name = "DIR")]
        repo: Option<String>,
        /// The integration branch turn files are committed on
        #[arg(long)]
        branch: Option<String>,
    },
    /// Record the critic's pane after checking who is in it
    Critic {
        slug: String,
        topic: String,
        #[arg(long)]
        pane: String,
    },
    /// Pin the next turn and send the TURN line
    Turn {
        slug: String,
        topic: String,
        #[arg(long)]
        resend: bool,
    },
    /// Commit the expected turn file, then advance
    Commit { slug: String, topic: String, n: u32 },
}

#[derive(Subcommand)]
enum PlanCommand {
    /// Print the plan card; a missing card prints revision zero
    Show {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
    },
    /// Set the end-result kind and its one sentence
    Set {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        #[arg(long, value_name = "KIND")]
        kind: String,
        #[arg(long)]
        does: String,
        #[arg(long)]
        expect: u64,
    },
    /// Add, edit, link, unlink, remove or move one step
    Step {
        #[command(subcommand)]
        command: PlanStepCommand,
    },
    /// Derive step states from the bound work and write only on change
    Sync {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
    },
}

#[derive(Subcommand)]
enum PlanStepCommand {
    /// Add a step, optionally bound to threads or rounds
    #[command(allow_missing_positional = true)]
    Add {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        text: String,
        #[arg(long = "thread", value_name = "ID")]
        threads: Vec<String>,
        #[arg(long = "round", value_name = "ID")]
        rounds: Vec<String>,
        #[arg(long)]
        expect: u64,
    },
    /// Replace one step's sentence
    Edit {
        /// Project slug when TEXT is also present; otherwise the step id
        #[arg(value_name = "PROJECT_OR_ID")]
        project_or_id: String,
        /// Step id when TEXT is present; otherwise the replacement text
        #[arg(value_name = "ID_OR_TEXT")]
        id_or_text: String,
        /// Replacement text when the project slug is present
        text: Option<String>,
        #[arg(long)]
        expect: u64,
    },
    /// Add required work to a step
    #[command(allow_missing_positional = true)]
    Link {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        id: String,
        #[arg(long = "thread", value_name = "ID")]
        threads: Vec<String>,
        #[arg(long = "round", value_name = "ID")]
        rounds: Vec<String>,
        #[arg(long)]
        expect: u64,
    },
    /// Remove required work from a step
    #[command(allow_missing_positional = true)]
    Unlink {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        id: String,
        #[arg(long = "thread", value_name = "ID")]
        threads: Vec<String>,
        #[arg(long = "round", value_name = "ID")]
        rounds: Vec<String>,
        #[arg(long)]
        why: String,
        #[arg(long)]
        expect: u64,
    },
    /// Remove one step (identifiers are never reused)
    #[command(allow_missing_positional = true)]
    Remove {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        id: String,
        #[arg(long)]
        why: String,
        #[arg(long)]
        expect: u64,
    },
    /// Change display order only
    #[command(allow_missing_positional = true)]
    Move {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        id: String,
        #[arg(long, value_name = "ID")]
        before: String,
        #[arg(long)]
        expect: u64,
    },
}

#[derive(Subcommand)]
enum DecideCommand {
    /// Overturn a decision, keeping its history
    Overturn {
        /// Project slug when REASON is also present; otherwise the decision id
        #[arg(value_name = "PROJECT_OR_ID")]
        project_or_id: String,
        /// Decision id when REASON is present; otherwise the reason
        #[arg(value_name = "ID_OR_REASON")]
        id_or_reason: String,
        /// Reason when the project slug is present
        reason: Option<String>,
    },
    /// List the current choices, newest first
    List {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
    },
    /// Show one decision and whether it is still current
    #[command(allow_missing_positional = true)]
    Show {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        id: String,
    },
}

#[derive(Subcommand)]
enum AskCommand {
    /// Withdraw an open question, keeping its history
    Withdraw {
        /// Project slug when REASON is also present; otherwise the ask id
        #[arg(value_name = "PROJECT_OR_ID")]
        project_or_id: String,
        /// Ask id when REASON is present; otherwise the reason
        #[arg(value_name = "ID_OR_REASON")]
        id_or_reason: String,
        /// Reason when the project slug is present
        reason: Option<String>,
    },
    /// Answer an ask by id and revision
    Answer {
        /// Project slug when CHOICE is also present; otherwise the ask id
        #[arg(value_name = "PROJECT_OR_ID")]
        project_or_id: String,
        /// Ask id when CHOICE is present; otherwise the choice
        #[arg(value_name = "ID_OR_CHOICE")]
        id_or_choice: String,
        #[arg(long)]
        revision: u32,
        choice: Option<u32>,
    },
}

#[derive(Subcommand)]
enum TermCommand {
    /// Add a term with its one sentence
    #[command(allow_missing_positional = true)]
    Add {
        /// Project slug; omit to use the current project
        #[arg(value_name = "PROJECT")]
        slug: Option<String>,
        name: String,
        #[arg(long)]
        plain: Option<String>,
        /// Where the term is explained (brief or spec path)
        #[arg(long)]
        path: Option<String>,
    },
}

fn run_rounds(ctx: &Ctx, command: Command) -> Result<()> {
    use crate::{ask, board, checkpoint, decide, dialogue, glossary, plan, round, talk};
    let slug_of = |slug: Option<String>| overview::require_slug(ctx, slug.as_deref());
    match command {
        Command::Round { command } => match command {
            RoundCommand::Open {
                slug,
                round: id,
                branch,
                plain,
                repo,
            } => {
                let r = round::open(
                    ctx,
                    &slug,
                    round::OpenArgs {
                        round: id,
                        branch,
                        plain,
                        repo,
                    },
                )?;
                crate::output::insert("phase", serde_json::to_value(r.phase)?);
                println!(
                    "opened {} on `{}` (gates: {})",
                    r.round,
                    r.branch,
                    r.gates.len()
                );
                Ok(())
            }
            RoundCommand::Admit {
                slug,
                round: id,
                thread,
            } => {
                let r = round::admit(ctx, &slug, &id, &thread)?;
                crate::output::insert("phase", serde_json::to_value(r.phase)?);
                println!(
                    "{thread} admitted to {id}; manifest revision {}",
                    r.manifest.revision
                );
                Ok(())
            }
            RoundCommand::Remove {
                slug,
                round: id,
                thread,
            } => {
                let r = round::remove(ctx, &slug, &id, &thread)?;
                crate::output::insert("phase", serde_json::to_value(r.phase)?);
                println!(
                    "{thread} removed from {id}; manifest revision {}",
                    r.manifest.revision
                );
                Ok(())
            }
            RoundCommand::Retry {
                slug,
                round: id,
                reason,
            } => {
                let result = round::retry(ctx, &slug, &id, &reason)?;
                crate::output::success(
                    Some(&result.action),
                    &result,
                    &format!("{} {} with {}\n", result.action, id, result.thread),
                    "",
                )
            }
            RoundCommand::Cancel {
                slug,
                round: id,
                reason,
            } => {
                let result = round::cancel(ctx, &slug, &id, &reason)?;
                let pending = result
                    .threads
                    .iter()
                    .filter(|thread| thread.state == "cleanup_pending")
                    .count();
                let message = if pending == 0 {
                    format!("cancelled {id}: {}\n", result.reason)
                } else {
                    format!("cancelled {id}; cleanup pending for {pending} thread(s)\n")
                };
                crate::output::success(
                    Some(if pending == 0 {
                        "cancelled"
                    } else {
                        "cleanup_pending"
                    }),
                    &result,
                    &message,
                    "",
                )
            }
            RoundCommand::Rebind {
                slug,
                round: id,
                thread,
            } => {
                let result = round::rebind(ctx, &slug, &id, &thread)?;
                crate::output::success(
                    Some(&result.action),
                    &result,
                    &format!("{} is bound to {id}\n", result.thread),
                    "",
                )
            }
            RoundCommand::Adopt {
                slug,
                round: id,
                thread,
            } => {
                let result = round::adopt(ctx, &slug, &id, &thread)?;
                crate::output::success(
                    Some(&result.action),
                    &result,
                    &format!("{}: {} into {id}\n", result.action, result.thread),
                    "",
                )
            }
            RoundCommand::Review { slug, round: id } => {
                let o = round::review(ctx, &slug, &id)?;
                crate::output::insert("brief_commit", o.brief_commit.clone());
                crate::output::insert("review_branch", o.review_branch.clone());
                crate::output::insert("manifest_hash", o.manifest_hash.clone());
                println!("brief commit B {} ({})", o.brief_commit, o.brief_path);
                println!(
                    "review branch {} at {}",
                    o.review_branch,
                    o.worktree.display()
                );
                println!(
                    "manifest revision {} frozen, hash {}",
                    o.revision, o.manifest_hash
                );
                println!(
                    "next: `round advance {slug}` starts and binds the reviewer after routing from its full brief and pinned changes; `round retry {slug} {id} --reason <why>` is the recovery command"
                );
                if let (Some(c), Some(v)) = (&o.earlier_candidate, &o.earlier_verdict) {
                    println!(
                        "repair: merge the earlier candidate {c} (verdict {v}) over the new base instead of the pinned shas; `round advance {slug}` starts a reviewer with that task"
                    );
                }
                Ok(())
            }
            RoundCommand::Merge {
                slug,
                round: id,
                stop_after,
            } => {
                let stop = stop_after.map(|s| s.parse()).transpose()?;
                let result = round::merge(ctx, &slug, &id, stop)?;
                let record = round::load(&Project::load(&ctx.root, &slug)?, &id)?;
                let install_required =
                    matches!(
                        &result,
                        round::MergeOutcome::Checkpointed { .. } | round::MergeOutcome::NoOp { .. }
                    ) && crate::harness::is_harness_repo(&ctx.config_dir, &record.repo);
                let mut message = String::new();
                if install_required {
                    // Preserve the established ordering: merge used to print
                    // this line before returning its outcome to the CLI.
                    message.push_str("run ha harness install\n");
                }
                let outcome = match &result {
                    round::MergeOutcome::Checkpointed { head, lanes } => {
                        message.push_str(&format!("merged and checkpointed: H {head}\n"));
                        for lane in lanes {
                            message.push_str(&format!("  {lane}\n"));
                        }
                        "merged"
                    }
                    round::MergeOutcome::NoOp { head } => {
                        message.push_str(&format!(
                            "already checkpointed at H {head}; nothing to do\n"
                        ));
                        "already_checkpointed"
                    }
                    round::MergeOutcome::RepairReviewStarted {
                        review_branch,
                        reviewer: Some(reviewer),
                    } => {
                        message.push_str(&format!(
                            "integration base moved; started repair review {review_branch} with {reviewer}\n"
                        ));
                        "repair_review_started"
                    }
                    round::MergeOutcome::RepairReviewStarted {
                        review_branch,
                        reviewer: None,
                    } => {
                        message.push_str(&format!(
                            "integration base moved; prepared repair review {review_branch}; its reviewer start will retry automatically\n"
                        ));
                        "repair_review_prepared"
                    }
                    round::MergeOutcome::Stopped { phase } => {
                        message.push_str(&format!(
                            "stopped (test fault injection) at phase {phase:?}\n"
                        ));
                        "stopped"
                    }
                };
                crate::output::success(
                    Some(outcome),
                    &serde_json::json!({
                        "merge": result,
                        "phase": record.phase,
                        "harness_install_required": install_required,
                    }),
                    &message,
                    "",
                )
            }
            RoundCommand::Advance { slug } => match slug {
                Some(slug) => {
                    let outcome = round::advance(ctx, &slug)?;
                    crate::output::insert("started", serde_json::to_value(&outcome.started)?);
                    if outcome.started.is_empty() {
                        println!("no reviewer started");
                    } else {
                        for started in outcome.started {
                            println!(
                                "started reviewer {} for {}",
                                started.reviewer, started.round
                            );
                        }
                    }
                    Ok(())
                }
                None => {
                    let outcome = round::advance_event(ctx)?;
                    crate::output::insert("started", serde_json::to_value(&outcome.started)?);
                    println!("rounds advanced for the event's projects");
                    Ok(())
                }
            },
            RoundCommand::Show { slug, round: id } => {
                let project = Project::load(&ctx.root, &slug)?;
                if let Some(id) = id {
                    let record = round::load(&project, &id)?;
                    crate::output::insert("record", serde_json::to_value(&record)?);
                    print!("{}", round::show(ctx, &slug, &id)?);
                } else {
                    let rounds = round::list(&project);
                    crate::output::insert("rounds", serde_json::to_value(&rounds)?);
                    for record in rounds {
                        println!("{}\t{:?}\t{}", record.round, record.phase, record.branch);
                    }
                }
                Ok(())
            }
            RoundCommand::Tick { slug } => {
                let project = Project::load(&ctx.root, &slug)?;
                round::tick(ctx, &project)
            }
        },
        Command::Dialogue { command } => match command {
            DialogueCommand::Start {
                slug,
                topic,
                drafter,
                critic,
                plain,
                repo,
                branch,
            } => {
                let (d, next) = dialogue::start(
                    ctx,
                    &slug,
                    dialogue::StartArgs {
                        topic,
                        drafter,
                        critic,
                        plain,
                        repo,
                        integration: branch,
                    },
                    &crate::launch::DialoguePair,
                )?;
                println!("dialogue {} recorded on {}", d.topic, d.branch);
                print!("{next}");
                Ok(())
            }
            DialogueCommand::Critic { slug, topic, pane } => {
                dialogue::bind_critic(ctx, &slug, &topic, &pane)?;
                println!("critic of {topic} is pane {pane}");
                Ok(())
            }
            DialogueCommand::Turn {
                slug,
                topic,
                resend,
            } => {
                let t = dialogue::turn(ctx, &slug, &topic, resend)?;
                println!("turn {} sent; expected {}", t.n, t.expected_path);
                Ok(())
            }
            DialogueCommand::Commit { slug, topic, n } => {
                let r = dialogue::commit(ctx, &slug, &topic, n)?;
                println!("turn {} committed as {} (sha256 {})", r.n, r.commit, r.hash);
                Ok(())
            }
        },
        Command::Checkpoint {
            slug,
            pane,
            repo,
            branch,
            print,
            check,
        } => {
            let out = checkpoint::checkpoint(
                ctx,
                &slug,
                checkpoint::CheckpointArgs {
                    pane,
                    repo,
                    branch,
                    print,
                    check_only: check,
                },
            )?;
            print!("{out}");
            Ok(())
        }
        Command::Pickup {
            slug,
            all,
            start,
            pane,
            dry_run,
        } => {
            print!(
                "{}",
                checkpoint::pickup(
                    ctx,
                    checkpoint::PickupArgs {
                        slug: slug.as_deref(),
                        pane: pane.as_deref(),
                        dry_run,
                        all,
                        start,
                    },
                )?
            );
            Ok(())
        }
        Command::Ask {
            slug,
            command,
            question,
            choices,
            what,
            means,
            round: r,
            reask,
        } => match command {
            Some(AskCommand::Withdraw {
                project_or_id,
                id_or_reason,
                reason,
            }) => {
                let (slug, id, reason) = match reason {
                    Some(reason) => (Some(project_or_id), id_or_reason, reason),
                    None => (None, project_or_id, id_or_reason),
                };
                let slug = slug_of(slug)?;
                let by = ctx
                    .env
                    .var("USER")
                    .context("USER is required to record who withdrew the ask")?;
                ask::withdraw(ctx, &slug, &id, &reason, by)?;
                crate::output::insert("ask", id.clone());
                println!("{id} withdrawn: {reason}");
                Ok(())
            }
            Some(AskCommand::Answer {
                project_or_id,
                id_or_choice,
                revision,
                choice,
            }) => {
                let (slug, id, choice) = match choice {
                    Some(choice) => (Some(project_or_id), id_or_choice, choice),
                    None => (
                        None,
                        project_or_id,
                        id_or_choice
                            .parse::<u32>()
                            .context("choice must be a number")?,
                    ),
                };
                let slug = slug_of(slug)?;
                let answer = ask::answer(ctx, &slug, &id, revision, choice, "command")?;
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
                let (slug, question) = match (slug, question) {
                    (Some(question), None) => (None, Some(question)),
                    pair => pair,
                };
                let slug = slug_of(slug)?;
                let question = question.context("a question is required")?;
                let a = ask::ask(
                    ctx,
                    &slug,
                    ask::NewAsk {
                        question,
                        choices,
                        what,
                        means,
                        round: r,
                        reask,
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
        },
        Command::Plan { command } => match command {
            PlanCommand::Show { slug } => {
                let slug = slug_of(slug)?;
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
                let slug = slug_of(slug)?;
                let p = plan::set(ctx, &slug, &kind, &does, expect)?;
                crate::output::insert("revision", p.revision);
                println!("plan revision {} set to `{}`", p.revision, p.kind);
                Ok(())
            }
            PlanCommand::Step { command } => match command {
                PlanStepCommand::Add {
                    slug,
                    text,
                    threads,
                    rounds,
                    expect,
                } => {
                    let slug = slug_of(slug)?;
                    let p = plan::step_add(ctx, &slug, &text, threads, rounds, expect)?;
                    let id = p.steps.last().map(|s| s.id.clone()).unwrap_or_default();
                    crate::output::success(
                        None,
                        &serde_json::json!({
                            "operation": "added",
                            "revision": p.revision,
                            "step": id,
                            "record": p.steps.last(),
                            "plan": p,
                        }),
                        &format!("plan revision {}: added {id}\n", p.revision),
                        "",
                    )
                }
                PlanStepCommand::Edit {
                    project_or_id,
                    id_or_text,
                    text,
                    expect,
                } => {
                    let (slug, id, text) = match text {
                        Some(text) => (Some(project_or_id), id_or_text, text),
                        None => (None, project_or_id, id_or_text),
                    };
                    let slug = slug_of(slug)?;
                    let p = plan::step_edit(ctx, &slug, &id, &text, expect)?;
                    crate::output::success(
                        None,
                        &serde_json::json!({
                            "operation": "edited",
                            "revision": p.revision,
                            "step": id,
                            "record": p.steps.iter().find(|step| step.id == id),
                            "plan": p,
                        }),
                        &format!("plan revision {}: edited {id}\n", p.revision),
                        "",
                    )
                }
                PlanStepCommand::Link {
                    slug,
                    id,
                    threads,
                    rounds,
                    expect,
                } => {
                    let slug = slug_of(slug)?;
                    let p = plan::step_link(ctx, &slug, &id, threads, rounds, expect)?;
                    crate::output::success(
                        None,
                        &serde_json::json!({
                            "operation": "linked",
                            "revision": p.revision,
                            "step": id,
                            "record": p.steps.iter().find(|step| step.id == id),
                            "plan": p,
                        }),
                        &format!("plan revision {}: linked {id}\n", p.revision),
                        "",
                    )
                }
                PlanStepCommand::Unlink {
                    slug,
                    id,
                    threads,
                    rounds,
                    why,
                    expect,
                } => {
                    let slug = slug_of(slug)?;
                    let p = plan::step_unlink(ctx, &slug, &id, threads, rounds, &why, expect)?;
                    crate::output::success(
                        None,
                        &serde_json::json!({
                            "operation": "unlinked",
                            "revision": p.revision,
                            "step": id,
                            "record": p.steps.iter().find(|step| step.id == id),
                            "plan": p,
                        }),
                        &format!("plan revision {}: unlinked {id}\n", p.revision),
                        "",
                    )
                }
                PlanStepCommand::Remove {
                    slug,
                    id,
                    why,
                    expect,
                } => {
                    let slug = slug_of(slug)?;
                    let p = plan::step_remove(ctx, &slug, &id, &why, expect)?;
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
                    let slug = slug_of(slug)?;
                    let p = plan::step_move(ctx, &slug, &id, &before, expect)?;
                    crate::output::success(
                        None,
                        &serde_json::json!({
                            "operation": "moved",
                            "revision": p.revision,
                            "step": id,
                            "before": before,
                            "record": p.steps.iter().find(|step| step.id == id),
                            "plan": p,
                        }),
                        &format!("plan revision {}: moved {id}\n", p.revision),
                        "",
                    )
                }
            },
            PlanCommand::Sync { slug } => {
                let slug = slug_of(slug)?;
                match plan::sync(ctx, &slug)? {
                    plan::SyncOutcome::Missing => {
                        crate::output::set_outcome("plan_missing");
                        println!("no plan is written down yet")
                    }
                    plan::SyncOutcome::Unchanged { revision } => {
                        crate::output::set_outcome("unchanged");
                        crate::output::insert("revision", revision);
                        println!("plan revision {revision}: no step state changed")
                    }
                    plan::SyncOutcome::Changed { revision } => {
                        crate::output::insert("revision", revision);
                        println!("plan revision {revision}: step states refreshed")
                    }
                }
                Ok(())
            }
        },
        Command::Decide {
            slug,
            command,
            line,
            class,
            key,
            basis,
            replaces,
            request,
        } => match command {
            Some(DecideCommand::Overturn {
                project_or_id,
                id_or_reason,
                reason,
            }) => {
                let (slug, id, reason) = match reason {
                    Some(reason) => (Some(project_or_id), id_or_reason, reason),
                    None => (None, project_or_id, id_or_reason),
                };
                let slug = slug_of(slug)?;
                let by = ctx
                    .env
                    .var("USER")
                    .context("USER is required to record who overturned the decision")?;
                let record = decide::overturn(ctx, &slug, &id, &reason, by)?;
                crate::output::insert("decision", id.clone());
                println!("{}", decide::status_line(&record));
                Ok(())
            }
            Some(DecideCommand::List { slug }) => {
                let slug = slug_of(slug)?;
                let text = decide::list(ctx, &slug, false)?;
                if crate::output::structured() {
                    let value: serde_json::Value =
                        serde_json::from_str(&decide::list(ctx, &slug, true)?)?;
                    crate::output::insert("result", value);
                }
                print!("{text}");
                Ok(())
            }
            Some(DecideCommand::Show { slug, id }) => {
                let slug = slug_of(slug)?;
                let text = decide::show(ctx, &slug, &id, false)?;
                if crate::output::structured() {
                    let value: serde_json::Value =
                        serde_json::from_str(&decide::show(ctx, &slug, &id, true)?)?;
                    crate::output::insert("result", value);
                }
                print!("{text}");
                Ok(())
            }
            None => {
                let (slug, line) = match (slug, line) {
                    (Some(line), None) => (None, Some(line)),
                    pair => pair,
                };
                let mut missing = Vec::new();
                if line.is_none() {
                    missing.push("a decision line");
                }
                match class {
                    None => missing.push(
                        "--class (choose what-you-get, money, undo, or routine)",
                    ),
                    Some(class) if class.needs_basis() && basis.is_none() => missing.push(
                        "--basis <request:<id>|ask:<id>@<revision>> with the permission this choice rests on",
                    ),
                    Some(_) => {}
                }
                if replaces.is_some() && request.is_none() {
                    missing.push("--request <REQUEST_ID> with --replaces");
                }
                if !missing.is_empty() {
                    return Err(crate::refusal::error(format!(
                        "decision_requirements: missing {}",
                        missing.join("; ")
                    )));
                }
                let slug = slug_of(slug)?;
                let line = line.expect("checked above");
                let class = class.expect("checked above").as_str();
                let d = decide::decide(
                    ctx,
                    &slug,
                    decide::NewDecision {
                        line: &line,
                        class,
                        key: key.as_deref(),
                        basis: basis.as_deref(),
                        replaces: replaces.as_deref(),
                        request: request.as_deref(),
                    },
                )?;
                crate::output::insert("decision", d.id.clone());
                println!("{} {}", d.id, d.class);
                Ok(())
            }
        },
        Command::Say {
            slug,
            what,
            means,
            landed_round,
        } => {
            let slug = slug_of(slug)?;
            match landed_round {
                Some(round) => ask::say_landed(ctx, &slug, &what, means.as_deref(), &round)?,
                None => ask::say(ctx, &slug, &what, means.as_deref())?,
            }
            println!("said");
            Ok(())
        }
        Command::Explain { slug, name } => {
            let slug = slug_of(slug)?;
            print!("{}", glossary::explain(ctx, &slug, &name)?);
            Ok(())
        }
        Command::Term { command } => match command {
            TermCommand::Add {
                slug,
                name,
                plain,
                path,
            } => {
                let slug = slug_of(slug)?;
                let t = glossary::add_term(ctx, &slug, &name, plain.as_deref(), path.as_deref())?;
                println!("- {}: {}", t.name, t.sentence);
                Ok(())
            }
        },
        Command::Talk {
            slug,
            replay,
            accepted,
            open_tab,
        } => {
            if replay {
                print!("{}", talk::replay(ctx, &slug)?);
                Ok(())
            } else if accepted {
                let project = Project::load(&ctx.root, &slug)?;
                println!("{} request(s) accepted", talk::mark_accepted(&project)?);
                Ok(())
            } else if open_tab {
                let project = Project::load(&ctx.root, &slug)?;
                match talk::ensure_tab(ctx, &project)? {
                    Some(tab) => println!("talk tab {} (pane {})", tab.tab_id, tab.pane_id),
                    None => println!("talk is off for this project"),
                }
                Ok(())
            } else {
                talk::screen::run(ctx, &slug)
            }
        }
        Command::Board {
            slug,
            thread,
            print,
        } => {
            let project = Project::load(&ctx.root, &slug)?;
            if let Some(id) = thread {
                let lane = crate::thread::load(&project, &id)?;
                let machine = if lane.is_remote() && !lane.machine.is_empty() {
                    format!("on machine `{}`", lane.machine)
                } else {
                    "on this Mac".to_string()
                };
                let since = if lane.is_remote() {
                    crate::events::remote_state(&project, lane.machine_route()).last_pass
                } else {
                    lane.updated.clone()
                };
                let age = crate::board::age(&since)
                    .map(|age| format!("{age} ago"))
                    .unwrap_or_else(|| "not heard yet".into());
                println!("{}\t{}\t{}\t{age}", lane.id, lane.last_group, machine);
                return Ok(());
            }
            if print {
                for (k, v) in board::compute(ctx, &project) {
                    let verdict = match board::check_value(&project, &v) {
                        Ok(()) => "ok".to_string(),
                        Err(e) => format!("refused: {e:#}"),
                    };
                    println!("{k}\t{v}\t{verdict}");
                }
                println!(
                    "not understood so far\t{}",
                    ask::not_understood_count(&project)
                );
            } else {
                for (k, why) in board::refresh(ctx, &project)? {
                    println!("{k} kept its previous value: {why}");
                }
            }
            Ok(())
        }
        _ => unreachable!("run_rounds only receives this lane's commands"),
    }
}

#[derive(Subcommand)]
enum LedgerCommand {
    /// Open failures, worst repeat count first
    List,
    /// Show a failure and its evidence
    Show { id: String },
    /// Close a failure
    Done { id: String },
    /// Print an actionable task brief to standard output
    Task { id: String },
}

#[derive(Subcommand)]
enum InboxCommand {
    /// Move handled items to inbox/done/
    Done {
        slug: String,
        #[arg(value_name = "ITEM_ID", required_unless_present = "all")]
        ids: Vec<String>,
        #[arg(long, conflicts_with = "ids")]
        all: bool,
    },
}

#[derive(Subcommand)]
enum TaskCommand {
    /// Add a task tied to Rolf's request and plain acceptance conditions
    Add {
        slug: String,
        #[arg(long)]
        title: String,
        #[arg(long = "request", required = true)]
        requests: Vec<String>,
        #[arg(long = "acceptance", required = true)]
        acceptance: Vec<String>,
        #[arg(long, value_name = "PATH")]
        repo: Option<String>,
        #[arg(long, value_name = "STEP")]
        plan_step: Option<String>,
        /// Older note, instruction, decision or task this task replaces
        #[arg(long)]
        replaces: Option<String>,
    },
    /// Show one task and its derived state
    Show { slug: String, id: String },
    /// List tasks and their derived state
    List { slug: String },
    /// Add a dated note without changing state
    Note {
        slug: String,
        id: String,
        text: String,
        /// Request id behind this note
        #[arg(long)]
        request: String,
        /// Note or instruction this explicitly replaces
        #[arg(long)]
        replaces: Option<String>,
    },
    /// Drop a task, or withdraw acceptance conditions replaced by a newer choice
    Drop {
        slug: String,
        id: String,
        #[arg(long = "acceptance")]
        acceptance: Vec<usize>,
        #[arg(long)]
        reason: String,
    },
    /// Link a thread and its historical rounds to this task
    Adopt {
        slug: String,
        id: String,
        #[arg(long)]
        thread: String,
    },
    /// Record per-condition verification evidence
    Evidence {
        slug: String,
        id: String,
        #[arg(long, value_enum)]
        kind: crate::task::EvidenceKind,
        #[arg(long)]
        command: String,
        #[arg(long = "acceptance")]
        acceptance: Vec<usize>,
    },
}

#[derive(Subcommand)]
enum NoteCommand {
    /// Add a memory note or standing instruction with its provenance
    Add {
        slug: String,
        text: String,
        #[arg(long, value_enum)]
        kind: crate::note::Kind,
        /// Request id behind this note
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
    /// Start a thread: a worktree workspace for --repo, else a tab in the project workspace
    Start {
        slug: String,
        #[arg(long)]
        title: String,
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
        /// Exact configured recipe Rolf named for this one lane
        #[arg(long, value_name = "ID", requires = "basis")]
        recipe: Option<String>,
        /// Verbatim words from Rolf's request for the exact recipe
        #[arg(long, requires = "recipe")]
        basis: Option<String>,
        /// Birth sentence (SPEC-ADE D17 item 6)
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
        /// Plan step for a task created with this lane
        #[arg(long, value_name = "STEP", requires = "requests")]
        plan_step: Option<String>,
    },
    /// Replace a failed, blocked, or stuck attempt through bounded routing
    Retry {
        slug: String,
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Stop a thread, close its pane and remove its clean worktree
    Cancel {
        slug: String,
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Point this thread at the verified live process already doing its work
    Rebind {
        slug: String,
        id: String,
        #[arg(long, value_name = "PANE")]
        pane: String,
    },
    /// Seal completion from a resolved lane's verified stored report
    Attest {
        slug: String,
        id: String,
        #[arg(long)]
        reason: String,
    },
    /// Send a follow-up to a thread's agent
    Prompt {
        slug: String,
        id: String,
        /// The text; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        text_file: String,
    },
    /// List threads with live state and group
    List { slug: String },
    /// Show one thread's record
    Show { slug: String, id: String },
    /// Record an existing local agent pane as a thread of this project
    Adopt {
        slug: String,
        #[arg(long, value_name = "ID")]
        pane: String,
        #[arg(long)]
        title: String,
        /// Optional task; `-` reads standard input
        #[arg(long, value_name = "FILE")]
        task_file: Option<String>,
        /// Birth sentence (SPEC-ADE D17 item 6)
        #[arg(long)]
        plain: Option<String>,
        #[arg(long, value_name = "ROLE")]
        role: Option<String>,
        /// Do not send a primer (SPEC-ADE D7)
        #[arg(long)]
        passive: bool,
    },
    /// Record that the user has seen the current report
    Ack { slug: String, id: String },
    /// Resolve a thread (final copy first), or reopen a resolved one
    Resolve {
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
enum RoutineCommand {
    /// Approve a routine's command (a person at a terminal only)
    Approve { slug: String, name: String },
    /// List routines with their approval status
    List { slug: String },
}

#[derive(Subcommand)]
enum SafetyCommand {
    /// Print the effective safety settings and the config.toml table to edit
    Show { slug: String },
}

#[derive(Subcommand)]
enum TickerCommand {
    /// Start the ticker if it is not running (does nothing when there are no projects)
    Start,
    /// Run the ticker loop in the foreground
    Run {
        /// Wait for the previous ticker during an internal zero-gap handoff
        #[arg(long, hide = true)]
        handoff: bool,
    },
    /// Ask the running ticker to exit and wait for it
    Stop,
    /// Show the running ticker's version, root and tool resolution
    Status,
}

#[derive(Subcommand)]
enum PlainCommand {
    /// Check text using the deterministic identifier and vocabulary rules
    Check {
        #[arg(long, value_name = "FILE")]
        text_file: String,
    },
    /// Native CLI end-of-turn hook
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
}

fn machine_outcome(command: &str) -> String {
    let outcome = match command {
        "open" | "round open" => "opened",
        "context" | "round show" | "plan show" | "decide show" => "shown",
        "thread start" => "started",
        "thread retry" | "round retry" => "retried",
        "thread cancel" | "round cancel" => "cancelled",
        "thread rebind" | "round rebind" => "rebound",
        "thread attest" => "attested",
        "thread adopt" | "round adopt" => "adopted",
        "thread prompt" => "prompted",
        "thread resolve" => "resolved",
        "thread ack" => "acknowledged",
        "thread list" | "decide list" | "ledger list" => "listed",
        "thread show" | "ledger show" | "ledger task" => "shown",
        "ledger done" => "closed",
        "ask" => "asked",
        "ask answer" => "answered",
        "ask withdraw" => "withdrawn",
        "decide" => "decided",
        "decide overturn" => "overturned",
        "say" => "said",
        "done" | "waiting" | "failed" => "sealed",
        "round advance" => "advanced",
        "round admit" => "admitted",
        "round remove" => "removed",
        "round review" => "review_prepared",
        "round merge" => "merged",
        "round tick" => "ticked",
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
            "slug", "project", "round", "thread", "id", "name", "sha", "report", "branch", "pane",
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
    let mut leaf = &matches;
    let mut command_path = Vec::new();
    let mut explicit_slug = None;
    loop {
        if let Ok(Some(slug)) = leaf.try_get_one::<String>("slug") {
            explicit_slug = Some(slug.clone());
        }
        match leaf.subcommand() {
            Some((name, args)) => {
                command_path.push(name);
                leaf = args;
            }
            None => break,
        }
    }
    let mut command_name = command_path.join(" ");
    // Identify the object of a refusal/retry, not just its verb. Do not copy
    // task text, prompts, flags or environment into the CLI-level subject.
    for key in ["round", "id", "name"] {
        if let Ok(Some(value)) = leaf.try_get_one::<String>(key) {
            command_name.push(' ');
            command_name.push_str(value);
        }
    }
    let env = Env::from_process()?;
    let config_dir = env.config_dir();
    let root = paths::resolve_root(cli.root.as_deref(), &env, &config_dir)?;
    let runner = crate::ledger::RecordingRunner(&RealRunner);
    let ctx = Ctx {
        env: &env,
        root,
        config_dir,
        runner: &runner,
        detached_ticker: true,
    };

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
    let _scope = crate::ledger::Scope::new(&observed_project.iter().collect::<Vec<_>>());
    let subject = format!("ha {command_name}");
    let result = dispatch(ctx, cli.command, observed_project.as_ref());
    record_command_outcome(observed_project.as_ref(), &subject, &result);
    if result.is_ok() {
        crate::output::finish_success()?;
    }
    result
}

/// Only unexpected command errors belong in the failure ledger. A designed
/// refusal still returns its error to the caller, but its structural marker
/// keeps the safety/authority outcome out of defect counts.
fn record_command_outcome(project: Option<&Project>, subject: &str, result: &Result<()>) {
    match result {
        Err(error) if crate::refusal::is(error) => {
            crate::output::set_outcome("refused");
            crate::output::set_failure_class(None);
        }
        Err(error) => {
            if let Some(project) = project {
                crate::output::set_outcome("failed");
                crate::output::set_failure_class(Some("unknown"));
                crate::ledger::observe(project, "command-failed", subject, &format!("{error:#}"));
            } else {
                // With no project binding this is a rejected invocation (for
                // example a missing project), not evidence of failed work.
                crate::output::set_outcome("refused");
                crate::output::set_failure_class(None);
            }
        }
        Ok(()) => {
            if let Some(project) = project {
                crate::ledger::recovered(project, "command-failed", subject);
            }
        }
    }
}

fn dispatch(ctx: Ctx<'_>, command: Command, observed_project: Option<&Project>) -> Result<()> {
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
            session,
        } => coordinator::open(
            &ctx,
            &slug,
            &OpenOptions {
                session: session.into(),
                reprime,
                rebind,
            },
        ),
        Command::Context { slug, peek } => coordinator::context(&ctx, &slug, peek),
        Command::Overview { slug, wait } => overview::run(&ctx, slug.as_deref(), wait),
        Command::Focus { slug } => overview::focus(&ctx, slug.as_deref()),
        Command::Unfocus { session } => overview::unfocus(&ctx, &session.into()),
        Command::Ledger { command } => {
            let project = observed_project
                .as_ref()
                .context("no project resolves from this workspace or lane")?;
            match command {
                LedgerCommand::List => {
                    let entries = crate::ledger::list(project)?;
                    if crate::output::structured() {
                        crate::output::insert("result", serde_json::to_value(&entries)?);
                    }
                    for entry in entries {
                        println!("{}", crate::ledger::summary(&entry));
                    }
                }
                LedgerCommand::Show { id } => {
                    let record = crate::ledger::show(project, &id)?;
                    let message = format!("{}\n", serde_json::to_string_pretty(&record)?);
                    crate::output::success(
                        Some("shown"),
                        &serde_json::json!({ "record": record }),
                        &message,
                        "",
                    )?;
                }
                LedgerCommand::Done { id } => {
                    let closed = crate::ledger::done(project, &id)?;
                    let (outcome, message) = if closed.changed {
                        ("closed", format!("{} closed\n", closed.record.id))
                    } else {
                        (
                            "already_closed",
                            format!("{} was already closed\n", closed.record.id),
                        )
                    };
                    crate::output::success(
                        Some(outcome),
                        &serde_json::json!({
                            "record": closed.record,
                            "changed": closed.changed,
                        }),
                        &message,
                        "",
                    )?;
                }
                LedgerCommand::Task { id } => print!(
                    "{}",
                    crate::ledger::task(&crate::ledger::show(project, &id)?)
                ),
            }
            Ok(())
        }
        Command::Inbox { command } => match command {
            InboxCommand::Done { slug, ids, all } => {
                let project = Project::load(&ctx.root, &slug)?;
                let record = project.coordinator();
                let pane = std::env::var("HERDR_PANE_ID").ok();
                let binding = record
                    .as_ref()
                    .and_then(|record| pane.as_deref().map(|pane| (pane, record.attempt())));
                let result = inbox::done_bound(&project, &ids, all, binding)?;
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
                crate::output::success(
                    Some("noted"),
                    &serde_json::json!({ "note": note }),
                    &format!("noted {}\n", note.id),
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
                plan_step,
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
                let record = crate::task::add(
                    &project, &title, authority, acceptance, repo, plan_step, replaces,
                )?;
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
                let view = crate::task::view(&project, crate::task::load(&project, &id)?);
                let attestation = crate::task::attestation(&project, &view.record);
                let mut message = crate::task::render(&view);
                if let Some(attestation) = &attestation {
                    message.push_str(&format!(
                        "attested: {}: {}\n",
                        attestation.coordinator, attestation.reason
                    ));
                }
                crate::output::success(
                    Some("shown"),
                    &serde_json::json!({ "task": view, "attestation": attestation }),
                    &message,
                    "",
                )
            }
            TaskCommand::List { slug } => {
                let project = Project::load(&ctx.root, &slug)?;
                let (views, errors) = crate::task::views(&project);
                if let Some(error) = errors.first() {
                    return Err(anyhow::anyhow!("task_unreadable: {error:#}"));
                }
                let message = views
                    .iter()
                    .map(crate::task::render)
                    .collect::<Vec<_>>()
                    .join("");
                crate::output::success(
                    Some("listed"),
                    &serde_json::json!({ "tasks": views }),
                    &message,
                    "",
                )
            }
            TaskCommand::Note {
                slug,
                id,
                text,
                request,
                replaces,
            } => {
                let project = Project::load(&ctx.root, &slug)?;
                let record =
                    crate::task::note(&project, &id, &text, &request, replaces.as_deref())?;
                let view = crate::task::view(&project, record);
                crate::output::success(
                    Some("noted"),
                    &serde_json::json!({ "task": view }),
                    &format!("noted {}\n", view.record.id),
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
            TaskCommand::Adopt { slug, id, thread } => {
                let project = Project::load(&ctx.root, &slug)?;
                let record = crate::task::adopt(&project, &id, &thread)?;
                let view = crate::task::view(&project, record);
                crate::output::success(
                    Some("adopted"),
                    &serde_json::json!({ "task": view, "thread": thread }),
                    &format!("{thread} adopted into {}\n", view.record.id),
                    "",
                )
            }
            TaskCommand::Evidence {
                slug,
                id,
                kind,
                command,
                acceptance,
            } => {
                let project = Project::load(&ctx.root, &slug)?;
                let record =
                    crate::task::record_evidence(&project, &id, kind, &command, acceptance)?;
                let view = crate::task::view(&project, record);
                crate::output::success(
                    Some(view.state.word()),
                    &serde_json::json!({ "task": view }),
                    &format!("{} is {}\n", view.record.id, view.state.word()),
                    "",
                )
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
                basis,
                plain,
                job,
                requests,
                acceptance,
                plan_step,
            } => {
                let task = read_text(&task_file)?;
                let project = Project::load(&ctx.root, &slug)?;
                let task_id = match job {
                    Some(id) => {
                        crate::task::load(&project, &id)?;
                        id
                    }
                    None if workflow.as_deref() == Some("reviewer") => String::new(),
                    None if requests.is_empty() => {
                        return Err(crate::refusal::error(
                            "task_missing: a lane must pass --job <task>, or create one with --request and --acceptance",
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
                            plan_step,
                            None,
                        )?
                        .id
                    }
                };
                let thread = threads::start(
                    &ctx,
                    &slug,
                    StartArgs {
                        title,
                        repo,
                        machine,
                        base,
                        task,
                        plain: plain.unwrap_or_default(),
                        workflow,
                        recipe,
                        recipe_basis: basis,
                        task_id: task_id.clone(),
                    },
                )?;
                if !task_id.is_empty() {
                    crate::output::insert("task", task_id);
                }
                crate::output::insert("id", thread.id.clone());
                crate::output::insert("kind", serde_json::to_value(thread.kind)?);
                crate::output::insert("branch", thread.branch.clone());
                crate::output::insert("pane_id", thread.pane_id.clone());
                println!(
                    "{}",
                    serde_json::json!({ "id": thread.id, "kind": thread.kind, "branch": thread.branch, "pane_id": thread.pane_id })
                );
                Ok(())
            }
            ThreadCommand::Retry { slug, id, reason } => {
                let result = threads::retry(&ctx, &slug, &id, &reason)?;
                crate::output::success(
                    Some("retried"),
                    &result,
                    &format!(
                        "{} attempt {} is in pane {}; the ticker launches its agent\n",
                        result.thread, result.attempt, result.pane_id
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
                let state = threads::prompt(&ctx, &slug, &id, &text)?;
                crate::output::insert("agent_state", state.clone());
                println!("sent to {id} (agent was {state})");
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
        Command::Routine { command } => match command {
            RoutineCommand::Approve { slug, name } => {
                let project = Project::load(&ctx.root, &slug)?;
                routine::approve(&ctx.config_dir, &project, &name)
            }
            RoutineCommand::List { slug } => {
                let project = Project::load(&ctx.root, &slug)?;
                let commands = project.safety(&ctx.config_dir)?.routine_commands;
                routine::print_list(&ctx.config_dir, &project, commands);
                Ok(())
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
        Command::Safety { command } => match command {
            SafetyCommand::Show { slug } => {
                let project = Project::load(&ctx.root, &slug)?;
                let safety = project.safety(&ctx.config_dir)?;
                println!("Effective safety settings for `{slug}`:");
                println!("  start_threads = {:?}", safety.start_threads);
                println!("  routine_commands = {}", safety.routine_commands);
                println!();
                println!(
                    "To change one, edit {} by hand and add:",
                    ctx.config_dir.join("config.toml").display()
                );
                println!();
                println!("[safety.{:?}]", project.canonical_dir().to_string_lossy());
                Ok(())
            }
        },
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
        Command::Plain { command } => match command {
            PlainCommand::Check { text_file } => {
                let text = read_text(&text_file)?;
                let result = crate::plain::check(&text, &crate::plain::Glossary::default());
                if result.passed() {
                    println!("pass");
                    Ok(())
                } else {
                    for violation in result.violations {
                        eprintln!(
                            "{} {}..{}: {}",
                            violation.rule.code(),
                            violation.span.start,
                            violation.span.end,
                            violation.fix
                        );
                    }
                    bail!("plain check failed")
                }
            }
            PlainCommand::Hook {
                kind,
                project,
                binding,
                phase,
            } => crate::hook::run(&ctx, &kind, &project, &binding, &phase),
        },
        Command::Doctor { session } => {
            let result = doctor::run(&ctx, &session.into())?;
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
                return Err(crate::refusal::error("some checks failed"));
            }
            Ok(())
        }
        command @ (Command::Round { .. }
        | Command::Dialogue { .. }
        | Command::Checkpoint { .. }
        | Command::Pickup { .. }
        | Command::Ask { .. }
        | Command::Plan { .. }
        | Command::Decide { .. }
        | Command::Say { .. }
        | Command::Explain { .. }
        | Command::Term { .. }
        | Command::Talk { .. }
        | Command::Board { .. }) => run_rounds(&ctx, command),
        Command::Harness { command } => match command {
            HarnessCommand::Install => {
                let result = crate::harness::install(&ctx)?;
                crate::output::success(
                    Some("installed"),
                    &serde_json::json!({ "install": result }),
                    &result.message(),
                    &result.warnings(),
                )
            }
        },
        Command::Ticker { command } => match command {
            TickerCommand::Start => ticker::start(&ctx),
            TickerCommand::Run { handoff } => ticker::run(&ctx, handoff),
            TickerCommand::Stop => ticker::stop(&ctx.root),
            TickerCommand::Status => ticker::status(&ctx.root),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn the_installed_hook_keeps_its_machine_interface() {
        let cli = Cli::try_parse_from([
            "ha",
            "--root",
            "/home/agent/.herdr-ade",
            "plain",
            "hook",
            "--kind",
            "claude",
            "--project",
            "adeherdr",
            "--binding",
            "w1G:p1",
            "--phase",
            "prompt",
        ])
        .unwrap();
        assert!(matches!(
            cli.command,
            Command::Plain {
                command: PlainCommand::Hook {
                    kind,
                    project,
                    binding,
                    phase,
                }
            } if kind == "claude"
                && project == "adeherdr"
                && binding == "w1G:p1"
                && phase == "prompt"
        ));
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
            &["plan", "step", "link", "demo", "s-1", "--expect", "1"],
            &[
                "plan",
                "step",
                "unlink",
                "demo",
                "s-1",
                "--why",
                "No longer needed.",
                "--expect",
                "1",
            ],
            &[
                "plan",
                "step",
                "remove",
                "demo",
                "s-1",
                "--why",
                "No longer needed.",
                "--expect",
                "1",
            ],
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
            &["ask", "withdraw", "demo", "a-1", "No longer needed."],
            &["ask", "answer", "demo", "a-1", "--revision", "1", "1"],
            &["decide", "demo", "Keep this choice.", "--class", "routine"],
            &["decide", "overturn", "demo", "d-1", "Use the other choice."],
            &["decide", "list", "demo"],
            &["decide", "show", "demo", "d-1"],
            &["say", "demo", "--what", "This was checked."],
            &["explain", "demo", "name"],
            &[
                "term",
                "add",
                "demo",
                "name",
                "--plain",
                "This is one name.",
            ],
        ];
        for args in cases {
            let mut argv = vec!["herdr-ade"];
            argv.extend_from_slice(args);
            assert!(Cli::try_parse_from(argv).is_ok(), "{args:?}");
        }

        let omitted_cases: &[&[&str]] = &[
            &["ask", "withdraw", "a-1", "No longer needed."],
            &["decide", "overturn", "d-1", "Use the other choice."],
            &[
                "plan",
                "step",
                "edit",
                "s-1",
                "Ship it now.",
                "--expect",
                "1",
            ],
        ];
        for args in omitted_cases {
            let mut argv = vec!["herdr-ade"];
            argv.extend_from_slice(args);
            assert!(Cli::try_parse_from(argv).is_ok(), "{args:?}");
        }

        assert!(Cli::try_parse_from(["herdr-ade", "plan", "show", "--project", "demo"]).is_err());
    }

    #[test]
    fn an_unhealthy_doctor_reports_without_recording_itself() {
        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let _scope = crate::ledger::Scope::new(&[&project]);
        // Probe failures remain observable; only the completed doctor's own
        // nonzero report is exempt, not its children or arbitrary errors.
        let runner = crate::ledger::RecordingRunner(&world.runner);
        let mut ctx = world.ctx();
        ctx.runner = &runner;
        let result = dispatch(
            ctx,
            Command::Doctor {
                session: SessionArgs::default(),
            },
            Some(&project),
        );
        let error = result.as_ref().unwrap_err();
        assert_eq!(error.to_string(), "some checks failed");
        assert!(crate::refusal::is(error));
        let before = crate::ledger::list(&project).unwrap();
        assert!(!before.is_empty());
        for _ in 0..2 {
            record_command_outcome(Some(&project), "ha doctor", &result);
        }
        let after = crate::ledger::list(&project).unwrap();
        assert_eq!(before.len(), after.len());
        assert!(after.iter().all(|entry| entry.subject != "ha doctor"));

        // Neither the command name nor the message text suppresses real faults.
        let failure = Err(anyhow::anyhow!("some checks failed"));
        record_command_outcome(Some(&project), "ha doctor", &failure);
        let rows = crate::ledger::list(&project).unwrap();
        assert!(
            rows.iter()
                .any(|entry| { entry.kind == "command-failed" && entry.subject == "ha doctor" })
        );
        record_command_outcome(Some(&project), "ha doctor", &result);
        assert_eq!(rows.len(), crate::ledger::list(&project).unwrap().len());
    }

    #[test]
    fn repeating_round_cancel_does_not_record_a_failure() {
        let fx = crate::round::testkit::fixture();
        let ctx = fx.world.ctx();
        crate::round::open(
            &ctx,
            "demo",
            crate::round::OpenArgs {
                round: "r1".into(),
                branch: "main".into(),
                plain: Some("This round checks the work.".into()),
                repo: None,
            },
        )
        .unwrap();
        crate::round::cancel(&ctx, "demo", "r1", "no longer needed").unwrap();
        let result = crate::round::cancel(&ctx, "demo", "r1", "again").map(|_| ());
        assert!(result.is_ok());
        record_command_outcome(Some(&fx.project), "ha round cancel", &result);
        assert!(crate::ledger::list(&fx.project).unwrap().is_empty());
    }

    #[test]
    fn designed_refusals_are_not_failures_but_real_command_errors_are() {
        let root = tempfile::tempdir().unwrap();
        let project = crate::project::create(root.path(), "demo", "", vec![]).unwrap();
        let refusal: Result<()> = Err(crate::refusal::error(
            "harness_install_stale_self: run the command again",
        ));
        record_command_outcome(Some(&project), "ha harness install", &refusal);
        assert!(crate::ledger::list(&project).unwrap().is_empty());

        let failure: Result<()> = Err(anyhow::anyhow!("compiler process crashed"));
        record_command_outcome(Some(&project), "ha harness install", &failure);
        let entries = crate::ledger::list(&project).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, "command-failed");
        assert_eq!(entries[0].subject, "ha harness install");
        assert!(entries[0].detail.contains("compiler process crashed"));

        // A later guard refusal is neither a retry nor proof that the real
        // failure recovered. A successful re-entry closes the same entry.
        record_command_outcome(Some(&project), "ha harness install", &refusal);
        assert_eq!(crate::ledger::list(&project).unwrap().len(), 1);
        record_command_outcome(Some(&project), "ha harness install", &Ok(()));
        assert!(crate::ledger::list(&project).unwrap().is_empty());
        let closed = crate::ledger::show(&project, &entries[0].id).unwrap();
        assert!(closed.closed_at.is_some());
        assert_eq!(closed.count, 1);
    }
}
