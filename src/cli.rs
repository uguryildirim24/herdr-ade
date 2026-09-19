use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};

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
    /// Inbox items
    Inbox {
        #[command(subcommand)]
        command: InboxCommand,
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
    /// Move a project folder to the trash (no worktree, branch or PR is touched)
    Delete {
        slug: String,
        /// Delete even though coordinator or thread panes are alive
        #[arg(long)]
        force: bool,
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
    Waiting { what: String },
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
    #[command(args_conflicts_with_subcommands = true)]
    Ask {
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
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
    /// The plan card: goal, end result and steps (SPEC-talk §6.5)
    Plan {
        #[command(subcommand)]
        command: PlanCommand,
    },
    /// The choices the coordinator made for Rolf (SPEC-talk §6.6)
    #[command(args_conflicts_with_subcommands = true)]
    Decide {
        #[command(subcommand)]
        command: Option<DecideCommand>,
        line: Option<String>,
        #[arg(long, value_name = "CLASS")]
        class: Option<String>,
        #[arg(long, value_name = "KEY")]
        key: Option<String>,
        #[arg(long, value_name = "REFERENCE")]
        basis: Option<String>,
        #[arg(long, value_name = "DECISION_ID")]
        replaces: Option<String>,
        #[arg(long, value_name = "REQUEST_ID")]
        request: Option<String>,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
    /// Tell Rolf one checked line on the board and the talk tab
    Say {
        #[arg(long)]
        what: String,
        #[arg(long)]
        means: Option<String>,
        /// Attach this line as landing evidence for a merged round
        #[arg(long, value_name = "ROUND")]
        landed_round: Option<String>,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
    /// Print a name's recorded sentence and where it was born
    Explain {
        name: String,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
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
    /// Commit the review brief B, freeze the manifest, create the review branch
    Review { slug: String, round: String },
    /// Record the reviewer thread whose sealed done sha is the verdict commit V
    Reviewer {
        slug: String,
        round: String,
        thread: String,
    },
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
    /// Show the round's record and merge phase
    Show { slug: String, round: String },
    /// Run the ticker's pass for rounds, asks, talk and the board once
    Tick { slug: String },
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
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
    /// Set the end-result kind and its one sentence
    Set {
        #[arg(long, value_name = "KIND")]
        kind: String,
        #[arg(long)]
        does: String,
        #[arg(long)]
        expect: u64,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
    /// Add, edit, link, unlink, remove or move one step
    Step {
        #[command(subcommand)]
        command: PlanStepCommand,
    },
    /// Derive step states from the bound work and write only on change
    Sync {
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
}

#[derive(Subcommand)]
enum PlanStepCommand {
    /// Add a step, optionally bound to threads or rounds
    Add {
        text: String,
        #[arg(long = "thread", value_name = "ID")]
        threads: Vec<String>,
        #[arg(long = "round", value_name = "ID")]
        rounds: Vec<String>,
        #[arg(long)]
        expect: u64,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
    /// Replace one step's sentence
    Edit {
        id: String,
        text: String,
        #[arg(long)]
        expect: u64,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
    /// Add required work to a step
    Link {
        id: String,
        #[arg(long = "thread", value_name = "ID")]
        threads: Vec<String>,
        #[arg(long = "round", value_name = "ID")]
        rounds: Vec<String>,
        #[arg(long)]
        expect: u64,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
    /// Remove required work from a step
    Unlink {
        id: String,
        #[arg(long = "thread", value_name = "ID")]
        threads: Vec<String>,
        #[arg(long = "round", value_name = "ID")]
        rounds: Vec<String>,
        #[arg(long)]
        why: String,
        #[arg(long)]
        expect: u64,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
    /// Remove one step (identifiers are never reused)
    Remove {
        id: String,
        #[arg(long)]
        why: String,
        #[arg(long)]
        expect: u64,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
    /// Change display order only
    Move {
        id: String,
        #[arg(long, value_name = "ID")]
        before: String,
        #[arg(long)]
        expect: u64,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
}

#[derive(Subcommand)]
enum DecideCommand {
    /// List the current choices, newest first
    List {
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
    /// Show one decision and whether it is still current
    Show {
        id: String,
        #[arg(long)]
        json: bool,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
}

#[derive(Subcommand)]
enum AskCommand {
    /// Answer an ask by id and revision
    Answer {
        id: String,
        #[arg(long)]
        revision: u32,
        choice: u32,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
    },
}

#[derive(Subcommand)]
enum TermCommand {
    /// Add a term with its one sentence
    Add {
        name: String,
        #[arg(long)]
        plain: Option<String>,
        /// Where the term is explained (brief or spec path)
        #[arg(long)]
        path: Option<String>,
        #[arg(long, value_name = "SLUG")]
        project: Option<String>,
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
                println!(
                    "{thread} removed from {id}; manifest revision {}",
                    r.manifest.revision
                );
                Ok(())
            }
            RoundCommand::Review { slug, round: id } => {
                let o = round::review(ctx, &slug, &id)?;
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
                    "next: start the reviewer thread in that worktree with role reviewer, then `round reviewer {slug} {id} <thread>`"
                );
                Ok(())
            }
            RoundCommand::Reviewer {
                slug,
                round: id,
                thread,
            } => {
                round::bind_reviewer(ctx, &slug, &id, &thread)?;
                println!("{thread} reviews {id}");
                Ok(())
            }
            RoundCommand::Merge {
                slug,
                round: id,
                stop_after,
            } => {
                let stop = stop_after.map(|s| s.parse()).transpose()?;
                match round::merge(ctx, &slug, &id, stop)? {
                    round::MergeOutcome::Checkpointed { head, lanes } => {
                        println!("merged and checkpointed: H {head}");
                        for l in lanes {
                            println!("  {l}");
                        }
                    }
                    round::MergeOutcome::NoOp { head } => {
                        println!("already checkpointed at H {head}; nothing to do")
                    }
                    round::MergeOutcome::Stopped { phase } => {
                        println!("stopped (test fault injection) at phase {phase:?}")
                    }
                }
                Ok(())
            }
            RoundCommand::Advance { slug } => match slug {
                Some(slug) => round::advance(ctx, &slug),
                None => round::advance_event(ctx),
            },
            RoundCommand::Show { slug, round: id } => {
                print!("{}", round::show(ctx, &slug, &id)?);
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
                    &crate::launch::DialoguePair(crate::launch::parse_launch_config(
                        &ctx.config_dir,
                    )?),
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
            command,
            question,
            choices,
            what,
            means,
            round: r,
            reask,
            project,
        } => match command {
            Some(AskCommand::Answer {
                id,
                revision,
                choice,
                project,
            }) => {
                let slug = slug_of(project)?;
                let a = ask::answer(ctx, &slug, &id, revision, choice, "command")?;
                println!("{id} revision {revision}: {} ({})", a.choice, a.text);
                Ok(())
            }
            None => {
                let slug = slug_of(project)?;
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
                println!("ask {} revision {}", a.id, a.revision);
                print!("{}", ask::numbered(&a));
                Ok(())
            }
        },
        Command::Plan { command } => match command {
            PlanCommand::Show { json, project } => {
                let slug = slug_of(project)?;
                print!("{}", plan::show(ctx, &slug, json)?);
                Ok(())
            }
            PlanCommand::Set {
                kind,
                does,
                expect,
                project,
            } => {
                let slug = slug_of(project)?;
                let p = plan::set(ctx, &slug, &kind, &does, expect)?;
                println!("plan revision {} set to `{}`", p.revision, p.kind);
                Ok(())
            }
            PlanCommand::Step { command } => match command {
                PlanStepCommand::Add {
                    text,
                    threads,
                    rounds,
                    expect,
                    project,
                } => {
                    let slug = slug_of(project)?;
                    let p = plan::step_add(ctx, &slug, &text, threads, rounds, expect)?;
                    let id = p.steps.last().map(|s| s.id.clone()).unwrap_or_default();
                    println!("plan revision {}: added {id}", p.revision);
                    Ok(())
                }
                PlanStepCommand::Edit {
                    id,
                    text,
                    expect,
                    project,
                } => {
                    let slug = slug_of(project)?;
                    let p = plan::step_edit(ctx, &slug, &id, &text, expect)?;
                    println!("plan revision {}: edited {id}", p.revision);
                    Ok(())
                }
                PlanStepCommand::Link {
                    id,
                    threads,
                    rounds,
                    expect,
                    project,
                } => {
                    let slug = slug_of(project)?;
                    let p = plan::step_link(ctx, &slug, &id, threads, rounds, expect)?;
                    println!("plan revision {}: linked {id}", p.revision);
                    Ok(())
                }
                PlanStepCommand::Unlink {
                    id,
                    threads,
                    rounds,
                    why,
                    expect,
                    project,
                } => {
                    let slug = slug_of(project)?;
                    let p = plan::step_unlink(ctx, &slug, &id, threads, rounds, &why, expect)?;
                    println!("plan revision {}: unlinked {id}", p.revision);
                    Ok(())
                }
                PlanStepCommand::Remove {
                    id,
                    why,
                    expect,
                    project,
                } => {
                    let slug = slug_of(project)?;
                    let p = plan::step_remove(ctx, &slug, &id, &why, expect)?;
                    println!("plan revision {}: removed {id}", p.revision);
                    Ok(())
                }
                PlanStepCommand::Move {
                    id,
                    before,
                    expect,
                    project,
                } => {
                    let slug = slug_of(project)?;
                    let p = plan::step_move(ctx, &slug, &id, &before, expect)?;
                    println!("plan revision {}: moved {id}", p.revision);
                    Ok(())
                }
            },
            PlanCommand::Sync { project } => {
                let slug = slug_of(project)?;
                match plan::sync(ctx, &slug)? {
                    plan::SyncOutcome::Missing => println!("no plan is written down yet"),
                    plan::SyncOutcome::Unchanged { revision } => {
                        println!("plan revision {revision}: no step state changed")
                    }
                    plan::SyncOutcome::Changed { revision } => {
                        println!("plan revision {revision}: step states refreshed")
                    }
                }
                Ok(())
            }
        },
        Command::Decide {
            command,
            line,
            class,
            key,
            basis,
            replaces,
            request,
            project,
        } => match command {
            Some(DecideCommand::List { json, project }) => {
                let slug = slug_of(project)?;
                print!("{}", decide::list(ctx, &slug, json)?);
                Ok(())
            }
            Some(DecideCommand::Show { id, json, project }) => {
                let slug = slug_of(project)?;
                print!("{}", decide::show(ctx, &slug, &id, json)?);
                Ok(())
            }
            None => {
                let slug = slug_of(project)?;
                let line = line.context("a decision line is required")?;
                let class = class.context("a decision needs --class")?;
                let d = decide::decide(
                    ctx,
                    &slug,
                    decide::NewDecision {
                        line: &line,
                        class: &class,
                        key: key.as_deref(),
                        basis: basis.as_deref(),
                        replaces: replaces.as_deref(),
                        request: request.as_deref(),
                    },
                )?;
                println!("{} {}", d.id, d.class);
                Ok(())
            }
        },
        Command::Say {
            what,
            means,
            landed_round,
            project,
        } => {
            let slug = slug_of(project)?;
            match landed_round {
                Some(round) => ask::say_landed(ctx, &slug, &what, means.as_deref(), &round)?,
                None => ask::say(ctx, &slug, &what, means.as_deref())?,
            }
            println!("said");
            Ok(())
        }
        Command::Explain { name, project } => {
            let slug = slug_of(project)?;
            print!("{}", glossary::explain(ctx, &slug, &name)?);
            Ok(())
        }
        Command::Term { command } => match command {
            TermCommand::Add {
                name,
                plain,
                path,
                project,
            } => {
                let slug = slug_of(project)?;
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
                talk::run(ctx, &slug)
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
        /// Birth sentence (SPEC-ADE D17 item 6)
        #[arg(long)]
        plain: Option<String>,
        /// Role from the roles table (default: lane)
        #[arg(long, value_name = "ROLE")]
        role: Option<String>,
        /// Pin this recipe from the role's allowed list
        #[arg(long, value_name = "ID")]
        recipe: Option<String>,
    },
    /// Bring back a thread whose pane is gone or whose start failed
    Restart { slug: String, id: String },
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
        #[arg(long, conflicts_with_all = ["remove_worktree", "skip_copy", "discard_uncopied", "keep_pane"])]
        reopen: bool,
        /// Also remove the worktree (never forced; the branch is kept)
        #[arg(long)]
        remove_worktree: bool,
        /// Resolve even though the final copy cannot be made
        #[arg(long)]
        skip_copy: bool,
        /// With --remove-worktree: accept losing what could not be copied
        #[arg(long, requires = "remove_worktree")]
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
    Run,
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

pub fn run() -> Result<()> {
    let cli = Cli::parse();
    let env = Env::from_process()?;
    let config_dir = env.config_dir();
    let root = paths::resolve_root(cli.root.as_deref(), &env, &config_dir)?;
    let runner = RealRunner;
    let ctx = Ctx {
        env: &env,
        root,
        config_dir,
        runner: &runner,
        detached_ticker: true,
    };

    match cli.command {
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
        Command::Inbox { command } => match command {
            InboxCommand::Done { slug, ids, all } => {
                let project = Project::load(&ctx.root, &slug)?;
                let record = project.coordinator();
                let pane = std::env::var("HERDR_PANE_ID").ok();
                let binding = record
                    .as_ref()
                    .and_then(|record| pane.as_deref().map(|pane| (pane, record.attempt())));
                let moved = inbox::done_bound(&project, &ids, all, binding)?;
                println!("{moved} item(s) moved to inbox/done");
                Ok(())
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
        Command::Thread { command } => match command {
            ThreadCommand::Start {
                slug,
                title,
                repo,
                machine,
                base,
                task_file,
                plain,
                role,
                recipe,
            } => {
                let task = read_text(&task_file)?;
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
                        role,
                        recipe,
                    },
                )?;
                println!(
                    "{}",
                    serde_json::json!({ "id": thread.id, "kind": thread.kind, "branch": thread.branch, "pane_id": thread.pane_id })
                );
                Ok(())
            }
            ThreadCommand::Restart { slug, id } => {
                let thread = threads::restart(&ctx, &slug, &id)?;
                println!(
                    "{} is back in pane {}; the ticker launches its agent",
                    thread.id, thread.pane_id
                );
                Ok(())
            }
            ThreadCommand::Prompt {
                slug,
                id,
                text_file,
            } => {
                let text = read_text(&text_file)?;
                let state = threads::prompt(&ctx, &slug, &id, &text)?;
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
                println!(
                    "{}",
                    serde_json::json!({ "id": thread.id, "kind": thread.kind, "pane_id": thread.pane_id, "prompt_pending": thread.prompt_pending })
                );
                Ok(())
            }
            ThreadCommand::List { slug } => threads::print_list(&ctx, &slug),
            ThreadCommand::Show { slug, id } => threads::print_show(&ctx, &slug, &id),
            ThreadCommand::Ack { slug, id } => threads::ack(&ctx, &slug, &id),
            ThreadCommand::Resolve {
                slug,
                id,
                reopen,
                remove_worktree,
                skip_copy,
                discard_uncopied,
                keep_pane,
            } => threads::resolve(
                &ctx,
                &slug,
                &id,
                &ResolveArgs {
                    reopen,
                    remove_worktree,
                    skip_copy,
                    discard_uncopied,
                    keep_pane,
                },
            ),
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
        Command::Delete { slug, force } => lifecycle::delete(&ctx, &slug, force),
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
        Command::Waiting { what } => crate::lane::waiting(&ctx, &what),
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
            if !doctor::run(&ctx, &session.into())? {
                bail!("some checks failed");
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
        Command::Ticker { command } => match command {
            TickerCommand::Start => ticker::start(&ctx),
            TickerCommand::Run => ticker::run(&ctx),
            TickerCommand::Stop => ticker::stop(&ctx.root),
            TickerCommand::Status => ticker::status(&ctx.root),
        },
    }
}
