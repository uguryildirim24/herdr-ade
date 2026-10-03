# Operations and development

How Herdr ADE works, what it writes where, and how to run threads on other machines.

## How it works

- **The coordinator is an ordinary agent** in a Herdr pane that follows a skill (`herdr-ade skill` prints it). It plans and routes work; the binary executes those decisions.
- **The binary does mechanics.** Starting or restarting a thread, copying reports, marking inbox items handled: each is one deterministic subcommand. It talks to Herdr through Herdr's CLI. The coordinator prompt hook records your own messages as request authority.
- **Files are the record, prompts are wake-ups.** Thread and review records own their state; `context` renders it directly at the start of every turn. The inbox holds only messages such as courier deliveries and machine notices. A missed prompt loses nothing.
- **One ticker per projects root** checks every 15 seconds: thread state and groups, pending prompts, changed reports, pending cleanup. Remote machines are polled once a minute.
- **Tools are found even under a bare `PATH`.** A Herdr server started outside a login shell gives its plugins a minimal `PATH`; the binary appends `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin` and `~/.cargo/bin` to its own, so the ticker finds `gh`, `rsync` and friends. `ticker status` and `doctor` show what resolved.
- **Cleanup follows a landed review.** The binary prunes finished lane and closed review branches after worktree cleanup, with a lease for published refs. Text from reports and command output is never placed in a prompt.

## Where things live

```
~/.herdr-ade/<project>/
  PROJECT.md              editable settings; never replaced by the harness
  scratch/                the coordinator's temporary files
  library/<id>/           files a thread produced for you
  .state/
    page.md              generated current page (atomic replacement)
    notes/<id>.json       immutable facts and instructions with request ids and replacements
    retirements/<id>.json immutable note retirements
    deliveries/<id>/      atomic delivery entries (historical JSONL journals still load)
    tasks/job-NNNN.toml   stable tasks: request, acceptance, links and evidence
    threads/<id>.toml     thread record          artifacts/<hash>  sealed final report
    threads/<id>.task.md  the task as given      threads/<id>/     project-owned lane folder
    inbox/, inbox/done/   messages with no thread or review home
    asks/                historical questions, answers and withdrawals (read-only)
    events/, ops/        proof, delivery and recovery records
    reviews/              pile review records     history/           archived old documents
~/.herdr-ade/.ticker.lock  .ticker.log  .trash/
~/.config/herdr-ade/config.toml             executable recipes, editable routing, dispatch placement, machines and harness repositories; any coordinator may edit it
```

New notes, retirements, delivery entries, receipts and import markers publish complete files atomically without overwrite. Historical `notes.jsonl`, `retirements.jsonl` and delivery JSONL journals still load; interrupted final lines are named, never joined to a new write, and interrupted note ids are reserved. Recovery rebuilds incomplete historical receipts and import markers from their immutable events, while conflicting complete evidence remains an error. Box recovery errors surface through the courier.

Content folders in this tree are created on their first write; a new project has only `PROJECT.md` and `.state/`. All binary-owned records live under `.state/`. A project-owned lane folder stays where its thread record says it is.

## Safety and cleanup

ADE is not a sandbox. Shipped recipes bypass agent permission prompts, and their declared bypass flags are required. Agents have the launching account's filesystem, network and credential access. Git worktrees isolate changes, not privileges. Trust repositories, briefs and selected providers before dispatch; do not copy provider credentials between machines. Missing process or publication evidence is not success. Seal, merge, push, install and delivery remain separate facts.

Deletion verifies owned-resource scope and requires a platform trash tool on every affected machine before effects. Shared repositories stay; GitHub deletion requires explicit `--github`. Use `delete --preview` to inspect scope or `archive` for a reversible lifecycle change. Lane cleanup follows the checks below, not an agent permission dialog.

Every ADE lane works from a plain git worktree at `<repo>/.worktrees/<thread-id>/`, opened as a tab in the coordinator workspace (not as a Herdr worktree workspace). `tab create` sets `HERDR_ADE_LAUNCH`. The lane is primed with “Run the shell command `<prefix> skill <role>`, then read `.herdr-project/<project>-<id>/brief.md` and do what it says.” The frozen brief is a content-addressed project artifact tied to the worktree's exact base commit and materialized only in that ignored runtime folder. The agent writes `report.md` there and creates `library/` only for real deliverables. `.worktrees/` is added to `info/exclude`. `done` seals the report once as `.state/artifacts/<hash>`; thread and task views find it from the thread record. Unmatched historical `.state/threads/<id>.md` reports remain readable. Resolving removes a finished worktree with `git worktree remove` without `--force`, after copying real deliverables. A remote lane's rebuildable Cargo folder is removed in the same operation. Uncommitted tracked or untracked changes refuse resolution. Ignored data keeps both the worktree and its build folder but does not stop resolution; the typed `ignored_data` reason names each folder and its size. The branch is pruned after a clean worktree is removed; a kept worktree keeps its branch.

Ignored files are disposable only when their path is covered by the editable global setting below or by `disposable` on that repository's row in `PROJECT.md`. A harness repository row in `config.toml` may carry the same list. Repository lists are added to the global list only for their own repository. With no matching setting, every ignored file is treated as data. A one-part name matches that path component anywhere in the worktree; a path containing `/` matches from the worktree root. `*` matches within one path part (`runs/pytest-*` covers `runs/pytest-cancel` but not `runs/seed-1`). A nested Git checkout is always data, even inside a disposable folder.

```toml
[worktrees]
disposable = ["target", ".target", "zig-out", ".zig-cache", "node_modules"]
```

`doctor` reports remote build folders with no open thread and free space on the local machine and every saved remote machine. Branch and finished-worktree scans do not run in doctor. The failure threshold is editable and defaults to 12 GB:

```toml
[doctor]
min_free_disk_gb = 12
```

Settings remain in the hand-edited front matter of `PROJECT.md`. The harness never replaces this file, including on existing projects: historical bodies are left intact, not migrated into notes. The separate generated `.state/page.md` is rebuilt atomically from the records and shows the goal, what you get, running work and its waits, plan, each open task on one status-and-next-action line, current task notes, instructions and facts, recent completions. An edit during rendering remains saved; the next refresh updates the view. `task show` carries the task's full acceptance conditions and evidence. `PROJECT.md` settings: `name` (the Herdr workspace label; a slug-like name such as `herdr-ade` is stored and shown as `Herdr Ade`, plain title case, so write `GTM AI` yourself if you want capitals; an edited name renames the workspace on the next `open`), `goal`, `repos` (`path`, optional `machine`, `box_path`, `publish_url`, `disposable`, integration `branch`, allowed `push_remote`, repository `gates`). Each gate is `{ command = "...", paths = ["src/**", "tests/**"], env = { NAME = "value" } }`; optional `paths` selects the gate only when the reviewed diff changes a matching repository-relative file. Glob syntax: `*` and `?` within one path segment, `**` as a whole segment for zero or more directories (no character classes, negation or absolute paths). `doctor` flags globs that match no tracked file. Omit `paths` to always run; omitted `gates` means not configured while `gates = []` explicitly makes that repository gate-free. A project-wide `gates` key is removed. Projects have no thread-count limit. Kind and args come from `[recipes.<id>]` (`kind`, `provider`, `args`, `env`, `ready_timeout_ms`, `enabled`, `plain`). The ordered `[routing]` table selects a recipe by workflow or task product.

The title is the new lane's description. `thread start --job` takes the title and repository from the stable task; `--title` and `--repo` override real lane differences. A newly created task and `thread adopt` need an explicit title; adoption uses `--workflow` like start. The default workflow is `lane`. Adoption is only for agent panes outside Git repositories; use `thread start` for repository work. `--passive` on adopt sends no primer. Lane `parent` tokens nest panes under their current coordinator in herdr's sidebar (with `agent_parent_nesting` enabled). ADE requires `agent_parent_notify = false` in herdr's `[experimental]` config on every machine: the same token otherwise makes herdr push GONE/BLOCKED, bypassing ADE's durable notice outbox. Sealed lanes and deliberate cleanup do not produce GONE; a confirmed unsealed crash still does. Automated notices wait in a durable outbox for a 120-second idle batch (one line per notice), or go straight to a mid-turn coordinator. Your input is never batched. A missing context receipt does not repeat a notice on the same coordinator binding.

`config.toml` also carries the harness repositories under `[harness] repos` (the same repository-row shape). Every project may start a lane or review a pile on a harness repository, listed in `PROJECT.md` or not; a repository that is neither listed nor a harness repository is refused. A harness repository row may declare `gates` with the same command, paths and environment fields as a project row. Pile reviews use the `[harness]` row's `gates` first, otherwise the reviewing project's listed row's `gates`. If both omit `gates`, the review refuses with `harness_gates_missing` and asks for gates on the repository's `[harness]` row in `config.toml`. An explicit `gates = []` is a declared gate-free policy. Non-harness repositories may still omit gates; their review record, verdict, context and landing line say "no gates declared". After a pile lands and pushes the integration ref, harness repositories use the installer: it builds and installs locally and on the saved box.

## Commands

Every command accepts the global `--json` flag. It returns one record with an
`outcome`, the command name, the ordinary human `message`, and useful ids under
`data`. A refusal exits non-zero and includes its `reason` in that record. Every command that targets one project takes its slug as an explicit positional argument; it never guesses from the number of remaining words or the current workspace.

| Command | What it does |
| --- | --- |
| `new <name> [--goal] [--repo PATH[@MACHINE]]...` | Create a project folder with its one current page. |
| `list [--all]` | Projects with status and thread counts by group. |
| `open <project> [--recipe ID --basis request:<id>] [--reprime] [--session N \| --socket P] [--rebind]` | Workspace, coordinator tab and coordinator agent; focuses it when it already runs. A basis may name another project's request as `request:<project>/<id>`. The binding keeps the chosen recipe and basis when its process relaunches. |
| `context <project> [--peek] [--full]` | Shows up to 20 unread changes and counts any overflow for the next read. The first read adds project orientation. `--full` shows all current items without the row limit; `--peek` records nothing. Failed output consumes nothing. |
| `handoff <project> [--note-file <path\|->]` | Fresh, non-consuming snapshot with a 16,000-character budget. With a session note (or stdin via `-`), writes `.state/handoffs/<UTC>.md`, keeps the newest 20, and prints the saved handoff. |
| `inbox done <project> <item>... \| --all` | Mark inbox items handled. |
| `task add`, `task show`, `task list`, `task drop` | Stable intent and acceptance conditions. State comes from the lane seal and pile review: open, working, finished, merged, installed. A no-change seal finishes immediately. `task drop --acceptance N --reason` withdraws a replaced condition. |
| `note add <project> <text> --kind memory\|instruction --request <id> [--task <job>] [--replaces <id>]` | The only fact and instruction writer. It may scope the row to a task or explicitly replace an older row. |
| `thread start <project> --job TASK --task-file F [--title T] [--repo PATH] [--machine M] [--base BRANCH] [--attach PATH]...` | The stable task supplies title and repository. Override flags describe a real lane difference. Omitting `--job` creates a task only when `--title`, `--request` and `--acceptance` are present. Routing is resolved before any worktree or tab exists. The frozen brief artifact carries the stable task, applicable current instructions and facts, repository, machine, pinned repository gates and finish paths; its hash and the exact code base are recorded before the worktree and tab are created. Start returns the thread id after validation and freezing, before checkout creation, placement or agent launch. JSON includes the id, branch, machine and `state = "starting"`. The ticker drives checkout preparation, terminal binding, the lane card, launch and initial input; retry and parked reopen use the same sequence. Later startup failures deliver a FAILED notice with a runnable retry command. Repeat `--attach PATH` to freeze only the named files in the artifact store; placement writes them under `.herdr-project/<project>-<thread>/attachments/<basename>` and the brief lists those lane-local paths. Missing files, duplicate basenames and attachments over the existing 200 MiB linked-artifact bound are refused synchronously. Retry and reopen retain the frozen attachments. |
| `thread prompt`, `thread list`, `thread show` | Everyday follow-up and inspection. `thread --help` groups the remaining recovery and administration commands. |
| `thread attest <project> <id> --reason S` | Seal `done` from a resolved, uncancelled lane's preserved report draft or unmatched historical report after its bytes match the recorded hash; records the coordinator and reason. |
| `thread resolve <project> <id> [--skip-copy] [--discard-uncopied] [--keep-pane] [--reopen]` | Resolve after the final copy and close the pane and tab. A landed worktree is removed only when it has no changes or non-disposable ignored data. Changes refuse resolution; ignored data resolves the thread but keeps the worktree with folder sizes. Cleanup prunes the branch if the worktree is removed; a kept worktree keeps its branch. |
| `overview <project> [--history]` | Active threads grouped by state and attention. `--history` also shows resolved threads. |
| `plan show <project>`, `plan set <project>`, `plan step add\|edit\|link\|unlink\|remove\|move <project>`, `plan sync <project>` | The plan card: goal, end result and any number of steps. `plan step add <project> "<text>" --under <step>` adds a subtask one level deep. New links use repeatable `--task`; a task may link to several steps or subtasks. Historical thread and task-side step bindings still load. |
| `review <project> [--repo PATH]` | Start the ready pile now, enable automatic reviews project-wide, or display an existing review. |
| `review retry <project> [--repo PATH]` | Replace a stuck or dead reviewer in its checkout, or resume a landed review's publication, install and cleanup. |
| `review cancel <project> [--repo PATH]` | Cancel an unlanded review and return its lanes to the pile. |
| `harness install` | Uses the landing's checked installer: record every project's plan counts and records-load, install the Mac, then check with the installed binary. A done-count drop or newly unreadable records restores the previous Mac binaries and ticker; boxes stay untouched. Only a passing Mac proceeds to the saved boxes and hook rebinding. Changed Rundown binaries reopen only proven single-pane ADE Rundown tabs. Prints the same install-result line as REVIEW; count-only evidence is in `harness-install.json` under the config folder. |
| `pause`, `resume`, `archive`, `unarchive`, `delete [--preview] [--github]` | Project lifecycle. `delete` checks platform trash tools on every affected machine before effects, stops project-owned processes and sends owned files to the platform trash; shared resources stay. macOS uses `/usr/bin/trash`; Linux uses `gio trash` or `trash-put`. There is no permanent-file-deletion fallback. GitHub deletion is explicit. |
| `ticker start \| run \| stop \| status`, `doctor`, `skill` | Housekeeping. |

An early `agent_not_ready` startup block keeps the brief pending and shows as starting while the recipe's ready window remains open. Only a still-blocked agent at the end of that window fails with a visible-screen excerpt. Claude's default window is five minutes; recipes can set `ready_timeout_ms` explicitly. `thread retry` refuses a still-starting or working agent and shows its pane text. Groups, first match wins: Resolved; Working while starting; **Needs attention** (failed, a launch stuck for 60 seconds, a process gone with no report, or blocked for 30 seconds); **Unknown** for a box lane that has not been polled; **Working**; **Ready for review** (a report exists and you haven't acknowledged it); Idle. A report-only lane whose sealed commit equals its base closes after its final copy. A changed lane joins its repository pile. Coordinator retries are not limited by the automatic retry budget.

## Plans and questions

The project keeps a plan card. Reading it never changes a plan or resolves a lane.

- **The plan card** is `<project>/.state/plan.toml`, written under `<project>/.state/plan.lock` with a revision guard and an atomic rename. It holds the goal copied exactly from `PROJECT.md`, the authored outcome (`plan set <project> --does "<outcome>"`) and any number of ordered steps. Historical outcome fields still load. New bindings name stable tasks in each step, so one task may support several steps. A step's state is projected from those tasks. Historical thread bindings and task-side `plan_step` links still project from current lane/task records. Old round links are ignored. `plan sync` is the manual refresh.

Coordinators ask decisions in chat, as short choices between concrete outcomes, while keeping unrelated work moving.

Your coordinator messages get request ids from the prompt-submit hook under `.state/requests/`. Historical request ids in the old talk journal remain readable; nothing writes that journal now. Automated harness prompts are not requests. `context` shows the ids; tasks require that recorded authority even when started from the CLI.

The idle-plan ticker sends one nudge, then stays quiet until the plan revision, lane set or latest request changes, or a lane runs. It suggests only work that can proceed without your reply and permits waiting in chat.

The live ask workflow is removed: no creation or closure commands, notifications, publication retries or waiting widgets. Historical ask revisions, answers and withdrawals remain read-only on disk. Notes and tasks citing `ask:a-N@revision` still resolve a current, nonzero answer as authority. Old publication and counter files are left untouched but are no longer read or written. Rundown reads the shared project view: running, waiting and unplanned work, current reviews and holds, plus a `Needs you` line for pending personal input or login. Planned-step counts stay separate and unchanged; short panes prioritize unfinished work and show how many steps are hidden.

**Recovery.** A plan write is atomic, so a reader sees an old or a new complete record. A stale `--expect` fails and changes nothing. A failed plan refresh is reported on its own line and never rolls back a merge; the project page reads the authoritative records and shows that the plan needs to catch up until persistence catches up.

## Pile review

A project's pile contains its open sealed lanes for one repository with changes and no recorded merge. `done` compares the lane's own HEAD against its recorded base once and seals `has_changes`; the SHA must still equal that checkout's HEAD. Output published in a different repository is described in the report. No-change lanes finish without review, and their plan steps count as done.

`review <project> [--repo PATH]` starts the ready pile immediately, even while lanes are working, or displays its existing review. Every explicit call enables automatic reviews project-wide. Before that opt-in, the ticker never starts a review of parked work. Afterwards it starts a non-empty pile when no lane of the repository is working. Only one review of a repository runs at a time; later completions wait for the next pile.

`.state/reviews/review-N.toml` records the member seals, integration base, candidate branch, reviewer, gate policy, verdict, and completion of fast-forward, push, install, close and prune. Each effect is idempotent and resumes automatically on a later pass. Review output and context show merge, publication (verified or no remote configured), and install (pending, completed or not required) separately; lane cleanup debt and notice delivery remain separate. No Git polling is needed while a review waits for its reviewer. Task state and plan completion use the seal and this record; a harness task is installed only after installation succeeds.

The harness merges the pile into a candidate off the integration tip. One reviewer resolves conflicts, fixes small issues and runs the path-selected gates once on the combined result, including paths changed by its fixes. It seals MERGE, MERGE with `without = { lane = "reason" }`, or REJECT. Excluded lanes must be absent from candidate ancestry. Excluded and rejected lanes stay open for a follow-up and a fresh seal. On MERGE, the harness fast-forwards, pushes the configured remote, installs harness repositories, closes the merged lanes and prunes their branches. Kept worktrees still keep their branches to protect data.

If the integration tip moves, the same reviewer gets one request to merge it and rerun gates. A second move cancels that review and starts a fresh pile. `review retry` replaces the reviewer while preserving its checkout; `review cancel` releases an unlanded pile. After fast-forward, recovery finishes publication and cleanup rather than cancelling landed work.

Existing round, checkpoint and hold records remain untouched; the current pile workflow does not read or migrate them. Historical task installation entries still show installed. Open historical seals with unknown changes are classified once when first considered for review; work already on the integration branch is recorded as merged.

## Agent and machine adapters

Agent behavior lives in `[adapters.<kind>]`. A complete row declares `binary`, `launch_flags`, `ready_timeout_ms`, `coordinator`, `capabilities`, required flags and effort names, a doctor readiness driver and argument template (`{args}` expands to the routed recipe), and its hook path, JSON shape, events and prompt event. A kind may coordinate only when its native hooks expose prompt submission, so your typed requests cannot disappear. Shipped rows use the same declaration type. A new kind needs only this row unless its provider has a non-command readiness protocol.

No remote machine is declared by default. Copy the neutral example in [`assets/default-machines.toml`](../assets/default-machines.toml) into your own `config.toml` and set your paths. Existing complete machine rows in that config keep their values. Machine facts live in `[machines.<name>]`: `target`, `session`, `home`, `root`, `worktrees`, `build`, `path`, `ade_bin`, `pi_bin`, `kinds`, and `repos`. `kinds` is the list of adapter kinds the machine runs, such as `kinds = ["pi"]`; an empty list runs no agent jobs there. Omitting `kinds` leaves an existing user machine unrestricted, so every adapter kind may run there. Each repo row names `path`, `box_path`, and `publish_url`. Placement, doctor probes, lane environment, start lines, courier paths and cleanup resolve the selected machine row; another box does not add a code branch.

## Task-based routing

`thread start --job <task> --task-file <full brief>` reuses the task's title and repository. It has no `--model`; that argument is refused. `--workflow` selects instruction text and is available to routing rules. Optional task front matter may set `product = "code"`, `"spec"` or `"web-research"`, and `capability = "<name>"`. The selected recipe must declare that capability. The title and body do not select a recipe.

Routing and executable recipes live together in `~/.config/herdr-ade/config.toml`. Rules are checked in order; every field present on a rule must match. A brief-hash pin wins over the matched rule or default. Unknown keys, empty defaults, unknown or disabled recipe names, malformed pins and rules without a matcher are errors. `doctor` validates the table and flags an enabled recipe with neither a route nor a command. `context` prints one line per recipe with its plain use, capabilities and the rule or choice that reaches it; command syntax stays in the coordinator skill. Disabled recipes have no route.

The coordinator uses routing by default. When you explicitly choose the coordinator recipe for a project, `open <project> --recipe <id> --basis request:<id>` starts it and stores that exact recipe and request for process relaunches; a request from another project is `request:<project>/<id>`. When routing's choice does not fit a lane, the coordinator can start it with any enabled `--recipe <id>`. The lane launch record keeps the recipe with routing rule `explicit`; its basis and request are empty.

The starting table is:

```toml
[routing]
default = "pi_codex_sol_high"
retries = 1

[[routing.rules]]
workflow = "coordinator"
recipe = "claude_coordinator_opus"

[[routing.rules]]
product = "spec"
recipe = "claude_fable_xhigh"

[[routing.rules]]
product = "web-research"
recipe = "agy_gemini_flash"

[[routing.rules]]
capability = "native-chat"
recipe = "claude_fable_xhigh"

[routing.pins]
# "SHA256-of-exact-task-file-bytes" = "recipe-id"
```

A rule may override the global recovery policy with `retries = N`. `herdr-ade failed "<failure and evidence>"` reports failed work and retries that same recipe up to the bound. `--class provider --provider-kind <kind>` and `--class lost_connection` also use bounded same-recipe retries; `process_gone` restarts the attempt within the same bound; `unknown` waits for the coordinator. Exhausted recovery stays failed.

Each launch record and dispatch journal row says `pin`, `default`, `explicit` or `rule[n]`, so the reason for selection stays inspectable. Coordinator rows also carry `recipe_basis` and `recipe_request`; lane rows leave them empty. Historical launch and dispatch records without those fields still load.

## Inbox notifications

The ticker shows one Herdr notification for each set of new inbox items. The coordinator reads those items in its next context; the ticker never types an inbox or next-step prompt into its pane. If the coordinator pane is missing but its session responds, the ticker attempts to relaunch it with its saved recipe at most once per hour.

## Brief delivery

`brief_submitted` records a staged send, not proof that the agent received it. The ticker waits for activity or the attempt's bootstrap receipt without repeating an ambiguous paste. If the agent is still ready at the end of its configured ready window after staging (five minutes when unset), delivery becomes `brief_delivery_failed` and the coordinator gets a diagnostic with the transport result and pane screen. The pane and worktree stay intact. `thread retry --reason` accepts this failure, keeps the recipe, and still refuses a fresh working agent. Late activity or a receipt settles the original delivery without restarting it.

## Lane completion deliveries

A lane's typed event line is the wake-up: the ticker types it once into the coordinator's ready pane. `context` reads the current attempt's sealed completion evidence directly, alongside thread and review records. Local and courier completions write no duplicate inbox item; a replacement coordinator reads the same sealed work in context. Only a command the bound coordinator runs (`context`, or `inbox done` for messages) acknowledges a delivery; `--peek` and automation never do. Old thread/round inbox projections are ignored on read, not migrated.

## Threads on other machines

Save the machine with `herdr machine add --label <label> <ssh target>` (both machines need Herdr 0.9.1), then list a repo as `--repo /path/on/machine@<label>` or pass `thread start --machine <label>`. The home machine owns the project; only outbound SSH from home is needed, in batch mode, so set up key-based login first.

A lane or review tries a remote machine by default when it has a repository, `[dispatch] machine = "buildbox"` in `~/.config/herdr-ade/config.toml`, a matching `[machines.buildbox]` declaration, and its recipe kind is allowed by that machine's `kinds` (or `kinds` is omitted). A recipe whose kind is excluded runs locally without a box sign-in check. The repository needs both `box_path` and `publish_url` in its `PROJECT.md`, `[harness]` or machine `repos` row. Without the dispatch key it stays local. `thread start --machine local` keeps one start on the Mac, and `--machine <label>` names a saved machine only when its declaration allows the recipe kind. When a default box start finds the kind excluded, the box held, unreachable, unready, or unable to place the repository, it runs on the Mac instead and says why; an explicit `--machine <label>` still fails.

On a box, `herdr-ade done` publishes the lane's or reviewer's own branch to the recorded `publish_url` with a non-force push, verifies the published ref, and only then seals. A failed push refuses with the Git error; retry after resolving it. Mac `done` does not publish.

Coordinator compaction's `coordinator-handoff` mod forks only the session note, then passes it on stdin to the resolved ADE command prefix's `handoff <project> --note-file -`. The harness renders live records with the same page and action renderers as `context`, includes the latest 12 request files verbatim (oldest first, with ids), and limits finished/dropped history to ten one-line rows with `herdr-ade task list <project>` for the rest. Neither a snapshot nor a saved handoff consumes inbox items, deliveries, wake receipts or the context cursor.

The 16,000-character budget includes the header, session note and cut notice. Whole sections are cut in this order: recent finished work, inbox, recipes, repositories, facts, task notes, plan. The notice names the cuts and points to `herdr-ade context <project> --peek --full`. Your messages, standing instructions, open tasks and running work are never cut; goal, waits, action rows, reviews and the session note also stay intact. If protected content alone exceeds the budget, it is preserved with an explicit overflow notice. The mod retains its folder guard, subagent pass-through, precompute skip and 120-second fork deadline; exceptions, nonzero exits and empty stdout fall back to normal compaction.

When a coordinator binding changes, the ticker re-links verified live lanes under its new pane. A missing or mismatched process is not reparented. Use `thread retry` for a gone lane; there is no handoff document pair to maintain.

- The worktree, the brief and the report live on the remote machine. The home ticker polls it once a minute and copies a changed report with `scp` and the thread's `library/` with `rsync -rt` (symbolic links are never followed or copied; a library over 50 MB is not copied and the thread's copy notes say so).
- The box needs `herdr-ade` (`ade_bin`) for lane starts and seals, and `herdr-pi` (`pi_bin`) for pi `setup`, `login`, `doctor` and `check`; no pi verb runs through `herdr-ade`.
- Every command the home machine runs on a box goes over SSH with the box machine's configured `path` in front, so it does not depend on login-shell `PATH` edits.
- A machine that doesn't answer is left alone: no state is read, threads keep their last group, and it is skipped for about two minutes. After ten minutes you get one `outage` inbox item, and one more when it is back.
- A blocked remote thread needs input, not necessarily approval. Inspect its current question on that machine: select the machine in Herdr's sidebar, or run `herdr --remote <ssh target>`.
- A start without `--repo` uses the project's sole listed repository. With no listed repository or several, it refuses until the repository is specified.

## Laptop-closed operation

No plugin code is involved: install Herdr and this plugin on an always-on machine, keep the projects root there, open the project there, and attach from your laptop with `herdr --remote <ssh target>` (add `--session <name>` for a named session). The ticker runs on that machine. Checked on a Linux (aarch64) machine from a Mac.

## Development

Rust 1.89 is the supported minimum (`Cargo.toml`). Check the Rust gates at that version with `cargo +1.89.0 …` as well as the installed toolchain. CI runs these gates on Linux and macOS:

```bash
cargo fmt --check
cargo test
cargo clippy --all-targets -- -D warnings
cargo build --bin herdr-ade
node --test tests/pi_extension.test.mjs
claude plugin test mods/coordinator-handoff
git diff --check
```

The extension suite needs Node 24 and the built debug `herdr-ade` binary
(`CARGO_TARGET_DIR` is honored). The handoff suite uses Claude Code 2.1.287's
`plugin test` command: it runs each `*.test.ts` in a child of the Claude binary,
with the mod-hook environment and the `claude-code/testing` kit. It is not a
Node test-runner suite and needs no provider login or live model calls. CI
installs that version via `npm install --global @anthropic-ai/claude-code@2.1.287`.

Never develop against your default session or `~/.herdr-ade`. Use `HERDR_ADE_ROOT` and `XDG_CONFIG_HOME` under `/var/tmp`.
