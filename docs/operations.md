# Operations and development

How Herdr ADE works, what it writes where, and how to run threads on other machines.

## How it works

- **The coordinator is an ordinary agent** in a Herdr pane that follows a skill (`herdr-ade skill` prints it). Plugin code does not route messages, plan work or decide anything.
- **The binary does mechanics.** Starting or restarting a thread, copying reports, marking inbox items handled: each is one deterministic subcommand. It talks to Herdr through Herdr's CLI. The coordinator prompt hook also records Rolf's own messages.
- **Files are the record, prompts are wake-ups.** Thread and review records own their state; `context` renders it directly at the start of every turn. The inbox holds only messages such as courier deliveries and machine notices. A missed prompt loses nothing.
- **One ticker per projects root** checks every 15 seconds: thread state and groups, pending prompts, changed reports, pending cleanup. Remote machines are polled once a minute.
- **Tools are found even under a bare `PATH`.** A Herdr server started outside a login shell gives its plugins a minimal `PATH`; the binary appends `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin` and `~/.cargo/bin` to its own, so the ticker finds `gh`, `rsync` and friends. `ticker status` and `doctor` show what resolved.
- **Cleanup follows a landed review.** The binary prunes finished lane and closed review branches after worktree cleanup, with a lease for published refs. A one-time ticker sweep prunes leftovers only for resolved lanes whose tips are merged into main; dirty worktrees are kept. Text from reports and command output is never placed in a prompt.

## Where things live

```
~/.herdr-ade/<project>/
  PROJECT.md              one current page: editable settings, then a binary-written view
  scratch/                the coordinator's temporary files
  library/<id>/           files a thread produced for Rolf
  .state/
    notes.jsonl           dated facts and instructions with request ids and replacements
    tasks/job-NNNN.toml   stable tasks: request, acceptance, links and evidence
    threads/<id>.toml     thread record          artifacts/<hash>  sealed final report
    threads/<id>.task.md  the task as given      threads/<id>/     project-owned lane folder
    inbox/, inbox/done/   messages with no thread or review home
    asks/, events/, ops/  proof, delivery and recovery records
    reviews/              pile review records     history/           archived old documents
~/.herdr-ade/.ticker.lock  .ticker.log  .trash/
~/.config/herdr-ade/config.toml             executable recipes, editable routing, dispatch placement, machines and harness repositories; any coordinator may edit it
```

Content folders in this tree are created on their first write; a new project has only `PROJECT.md` and `.state/`. All binary-owned records live under `.state/`. A project-owned lane folder stays where its thread record says it is.

Every ADE lane works from a plain git worktree at `<repo>/.worktrees/<thread-id>/`, opened as a tab in the coordinator workspace (not as a Herdr worktree workspace). `tab create` sets `HERDR_ADE_LAUNCH`. The lane is primed with “Run the shell command `<prefix> skill <role>`, then read `.herdr-project/<project>-<id>/brief.md` and do what it says.” The frozen brief is a content-addressed project artifact tied to the worktree's exact base commit and materialized only in that ignored runtime folder. The agent writes `report.md` there and creates `library/` only for real deliverables. `.worktrees/` is added to `info/exclude`. `done` seals the report once as `.state/artifacts/<hash>`; thread and task views find it from the thread record. Unmatched historical `.state/threads/<id>.md` reports remain readable. Resolving removes a finished worktree with `git worktree remove` without `--force`, after copying real deliverables. A remote lane's rebuildable Cargo folder is removed in the same operation. Uncommitted tracked or untracked changes refuse resolution. Ignored data keeps both the worktree and its build folder but does not stop resolution; the typed `ignored_data` reason names each folder and its size. The branch is pruned after a clean worktree is removed; a kept worktree keeps its branch.

Ignored files are disposable only when their path is covered by the editable global setting below or by `disposable` on that repository's row in `PROJECT.md`. A harness repository row in `config.toml` may carry the same list. Repository lists are added to the global list only for their own repository. With no matching setting, every ignored file is treated as data. A one-part name matches that path component anywhere in the worktree; a path containing `/` matches from the worktree root. `*` matches within one path part (`runs/pytest-*` covers `runs/pytest-cancel` but not `runs/seed-1`). A nested Git checkout is always data, even inside a disposable folder.

```toml
[worktrees]
disposable = ["target", ".target", "zig-out", ".zig-cache", "node_modules"]
```

`doctor` reports remote build folders with no open thread and free space on the local machine and every saved remote machine. Branch and finished-worktree scans no longer run in doctor; the ticker sweeps landed, resolved leftovers once after install. The failure threshold is editable and defaults to 12 GB:

```toml
[doctor]
min_free_disk_gb = 12
```

Only the front matter of `PROJECT.md` is hand-edited. Its body is rebuilt atomically from the records and shows the goal, what Rolf gets, waits, running work, plan, each open task on one status-and-next-action line, current task notes, instructions and facts, recent completions. `task show` carries the task's full acceptance conditions and evidence. A rewrite compares the front matter again before replacing the page, so it will not overwrite a concurrent settings edit. `PROJECT.md` settings: `name` (the Herdr workspace label; a slug-like name such as `herdr-ade` is stored and shown as `Herdr Ade`, plain title case, so write `GTM AI` yourself if you want capitals; an edited name renames the workspace on the next `open`), `goal`, `repos` (`path`, optional `machine`, `box_path`, `publish_url`, `disposable`, integration `branch`, allowed `push_remote`, repository `gates`). Each gate is `{ command = "...", paths = ["src/**", "tests/**"], env = { NAME = "value" } }`; optional `paths` selects the gate only when the reviewed diff changes a matching repository-relative file. Glob syntax: `*` and `?` within one path segment, `**` as a whole segment for zero or more directories (no character classes, negation or absolute paths). `doctor` flags globs that match no tracked file. Omit `paths` to always run; omitted `gates` means not configured while `gates = []` explicitly makes that repository gate-free. A project-wide `gates` key is removed. Projects have no thread-count limit. `max_parallel_threads`, `coordinator_agent`, `thread_agent` and the two `*_agent_args` keys are gone; `doctor` refuses a `PROJECT.md` that still has them. `[roles.*]` is gone from both files and is refused. Kind and args come from `[recipes.<id>]` (`kind`, `provider`, `args`, `env`, `ready_timeout_ms`, `enabled`, `plain`). The ordered `[routing]` table selects a recipe by workflow or task product.

A thread birth sentence is required. `thread start --job` takes it, the title and the repository from the stable task; `--plain`, `--title` and `--repo` override only real lane differences. A newly created task still needs an explicit title, while `thread adopt` still takes `--plain`. Internal thread and review descriptions keep technical detail without vocabulary or length checks. The default workflow is `lane`. `--passive` on adopt sets the parent token and sends no primer.

`config.toml` also carries the harness repositories under `[harness] repos` (the same repository-row shape). Every project may start a lane or review a pile on a harness repository, listed in `PROJECT.md` or not; a repository that is neither listed nor a harness repository is refused. After a pile lands and pushes the integration ref, harness repositories use the installer: it builds and installs locally and on the saved box. `harness install` remains the standalone repair command.

## Commands

Every command accepts the global `--json` flag. It returns one record with an
`outcome`, the command name, the ordinary human `message`, and useful ids under
`data`. A refusal exits non-zero and includes its `reason` in that record. Every command that targets one project takes its slug as an explicit positional argument; it never guesses from the number of remaining words or the current workspace.

| Command | What it does |
| --- | --- |
| `new <name> [--goal] [--repo PATH[@MACHINE]]...` | Create a project folder with its one current page. |
| `list [--all]` | Projects with status and thread counts by group. |
| `open <project> [--recipe ID --basis request:<id>] [--reprime] [--session N \| --socket P] [--rebind]` | Workspace, coordinator tab and coordinator agent; focuses it when it already runs. A basis may name another project's request as `request:<project>/<id>`. The binding keeps the chosen recipe and basis when its process relaunches. |
| `context <project> [--peek]` | The `PROJECT.md` body followed by new messages from Rolf, unhandled inbox items, work needing action and the compact recipe list. `--peek` records nothing. |
| `inbox done <project> <item>... \| --all` | Mark inbox items handled. |
| `task add`, `task show`, `task list`, `task drop` | Stable intent and acceptance conditions. State comes from the lane seal and pile review: open, working, finished, merged, installed. A no-change seal finishes immediately. `task drop --acceptance N --reason` withdraws a replaced condition. |
| `note add <project> <text> --kind memory\|instruction --request <id> [--task <job>] [--replaces <id>]` | The only fact and instruction writer. It may scope the row to a task or explicitly replace an older row. |
| `thread start <project> --job TASK --task-file F [--title T] [--plain S] [--repo PATH] [--machine M] [--base BRANCH]` | The stable task supplies title, birth sentence and repository. Override flags describe a real lane difference. Omitting `--job` creates a task only when `--title`, `--request` and `--acceptance` are present. Routing is resolved before any worktree or tab exists. The frozen brief artifact carries the stable task, applicable current instructions and facts, repository, machine, pinned repository gates and finish paths; its hash and the exact code base are recorded before the worktree and tab are created. The command returns before the agent is up. |
| `thread prompt`, `thread list`, `thread show` | Everyday follow-up and inspection. `thread --help` groups the remaining recovery and administration commands. |
| `thread attest <project> <id> --reason S` | Seal `done` from a resolved, uncancelled lane's preserved report draft or unmatched historical report after its bytes match the recorded hash; records the coordinator and reason. |
| `thread resolve <project> <id> [--skip-copy] [--discard-uncopied] [--keep-pane] [--reopen]` | Resolve after the final copy and close the pane and tab. A landed worktree is removed only when it has no changes or non-disposable ignored data. Changes refuse resolution; ignored data resolves the thread but keeps the worktree with folder sizes. Cleanup prunes the branch if the worktree is removed; a kept worktree keeps its branch. |
| `overview <project> [--history] [--wait]` | Active threads grouped by what needs you. `--history` also shows resolved threads. |
| `plan show <project>`, `plan set <project>`, `plan step add\|edit\|link\|unlink\|remove\|move <project>`, `plan sync <project>` | The plan card: goal, end result and any number of steps. `plan step add <project> "<text>" --under <step>` adds a subtask one level deep. New links use repeatable `--task`; a task may link to several steps or subtasks. Historical thread and task-side step bindings still load. |
| `ask <project> "<question>?" --choice "<sentence>" --choice "<sentence>"` | Record a question; return one line with its id first. A normalized duplicate of another open question is refused with the existing id. |
| `ask withdraw <project> <id> "<reason>"` | Withdraw an open question, retaining its record and withdrawal reason, actor (`USER`) and time. Answered questions cannot be withdrawn. |
| `say <project> --what S [--means S]` | One line in the journal, returning its say id. |
| `review <project> [--repo PATH]` | Start or show the repository pile review; this first explicit call opts the project into automatic review. |
| `review retry <project> [--repo PATH]` | Replace a stuck or dead reviewer in its checkout. |
| `review cancel <project> [--repo PATH]` | Cancel an unlanded review and return its lanes to the pile. |
| `harness install` | Standalone repair: build every repository in `[harness]`, install it into `~/.local/bin`, then the same on the saved box. It rewrites every open coordinator's hook binding and names each one. |
| `pause`, `resume`, `archive`, `unarchive`, `delete [--preview] [--github]` | Project lifecycle. `delete` stops project-owned processes and sends owned local files to the macOS Trash; shared resources stay. GitHub deletion is explicit. |
| `ticker start \| run \| stop \| status`, `doctor`, `skill` | Housekeeping. |

An early `agent_not_ready` startup block keeps the brief pending and shows as starting while the recipe's ready window remains open. Only a still-blocked agent at the end of that window fails with a visible-screen excerpt. Claude's default window is five minutes; recipes can set `ready_timeout_ms` explicitly. `thread retry` refuses a still-starting or working agent and shows its pane text. Groups, first match wins: Resolved; Working while starting; **Waiting on you** (failed, a launch stuck for 60 seconds, a process gone with no report, or blocked for 30 seconds); **Unknown** for a box lane that has not been polled; **Working**; **Ready for review** (a report exists and you haven't acknowledged it); Idle. A report-only lane whose sealed commit equals its base closes after its final copy. A changed lane joins its repository pile. Coordinator retries are not limited by the automatic retry budget.

## Plans and questions

The project keeps a plan card. Reading it never changes a plan, resolves a lane or answers a question.

- **The plan card** is `<project>/.state/plan.toml`, written under `<project>/.state/plan.lock` with a revision guard and an atomic rename. It holds the goal copied exactly from `PROJECT.md`, one of seven end-result kinds and any number of ordered steps. New bindings name stable tasks in each step, so one task may support several steps. A step's state is projected from those tasks. Historical thread bindings and task-side `plan_step` links still project from current lane/task records. Old round links are ignored. `plan sync` is the manual refresh.

Every message Rolf sends the coordinator is a request with an id in `.state/talk/journal.jsonl`. A prompt-submit hook records text Rolf types into the coordinator pane; `ha say` and `ha ask` publish authored journal entries and question notifications. Harness prompts are automated and never count as Rolf's request. `context` lists the latest request ids.

There is no cap on plan steps or open questions. Ask creation, re-asking and answering serialize through `.state/asks/.open.lock`; duplicate open questions remain refused.

**Recovery.** A plan write is atomic, so a reader sees an old or a new complete record. A stale `--expect` fails and changes nothing. A failed plan refresh is reported on its own line and never rolls back a merge; the project page reads the authoritative records and shows that the plan needs to catch up until persistence catches up.

## Pile review

A project's pile contains its open sealed lanes for one repository with changes and no recorded merge. `done` compares the lane's own HEAD against its recorded base once and seals `has_changes`; the SHA must still equal that checkout's HEAD. Output published in a different repository is described in the report. No-change lanes finish without review, and their plan steps count as done.

`review <project> [--repo PATH]` starts or shows one review of that pile. Its first explicit use enables automatic reviews in the project. Before that opt-in, the ticker never starts a review of parked work. Afterwards it starts a non-empty pile when no lane of the repository is working. Only one review of a repository runs at a time; later completions wait for the next pile.

`.state/reviews/review-N.toml` records the member seals, integration base, candidate branch, reviewer, gate policy, verdict, and completion of fast-forward, push, install, close and prune. Each effect is idempotent and resumes on a later pass. No Git polling is needed while a review waits for its reviewer. Task state and plan completion use the seal and this record; a harness task is installed only after installation succeeds.

The harness merges the pile into a candidate off the integration tip. One reviewer resolves conflicts, fixes small issues and runs the path-selected gates once on the combined result, including paths changed by its fixes. It seals MERGE, MERGE with `without = { lane = "reason" }`, or REJECT. Excluded lanes must be absent from candidate ancestry. Excluded and rejected lanes stay open for a follow-up and a fresh seal. On MERGE, the harness fast-forwards, pushes the configured remote, installs harness repositories, closes the merged lanes and prunes their branches. Kept worktrees still keep their branches to protect data.

If the integration tip moves, the same reviewer gets one request to merge it and rerun gates. A second move cancels that review and starts a fresh pile. `review retry` replaces the reviewer while preserving its checkout; `review cancel` releases an unlanded pile. After fast-forward, recovery finishes publication and cleanup rather than cancelling landed work.

Cutover is installed only after all old reviews in every project have finished. Existing round, checkpoint and hold records are left untouched and are not read or migrated. Historical task installation entries still show installed. Open historical seals with unknown changes are classified once when first considered for review; work already on the integration branch is recorded as merged. The coordinator removes obsolete `task_states` settings after install; the parser ignores them.

## Agent and machine adapters

Agent behavior lives in `[adapters.<kind>]`. A complete row declares `binary`, `launch_flags`, `ready_timeout_ms`, `coordinator`, `capabilities`, required flags and effort names, a doctor readiness driver and argument template (`{args}` expands to the routed recipe), and its hook path, JSON shape, events and prompt event. A kind may coordinate only when its native hooks expose prompt submission, so text Rolf types into its pane cannot disappear. Shipped rows use the same declaration type. A new kind needs only this row unless its provider has a non-command readiness protocol.

No remote machine is declared by default. Copy the neutral example in [`assets/default-machines.toml`](../assets/default-machines.toml) into your own `config.toml` and set your paths. Existing complete machine rows in that config keep their values. Machine facts live in `[machines.<name>]`: `target`, `session`, `home`, `root`, `worktrees`, `build`, `path`, `ade_bin`, `pi_bin`, `kinds`, and `repos`. `kinds` is the list of adapter kinds the machine runs, such as `kinds = ["pi"]`; an empty list runs no agent jobs there. Omitting `kinds` leaves an existing user machine unrestricted, so every adapter kind may run there. Each repo row names `path`, `box_path`, and `publish_url`. Placement, doctor probes, lane environment, start lines, courier paths and cleanup resolve the selected machine row; another box does not add a code branch. Optional `load_limit = 1.5` is the one-minute load per online core above which new box lanes queue (default 1.5). The ticker retries queued starts without a provisioning timeout. Agents and their child commands run in user slices named `herdr-ade-<project>.slice` (slug dashes become underscores, keeping all projects siblings beneath `herdr-ade.slice`), with equal CPUWeight; pane shells and the Herdr server stay outside the slices. Optional `[machines.buildbox.project_caps.my-project]` sets `cpu_percent = 400` (systemd CPUQuota, 100% = one core) and `memory_mib = 16384` (MemoryMax). Caps apply to the entire project's slice, not each lane. `ha doctor` shows one-minute load and active project slices' measured CPU percentages. The box needs a working systemd user manager with cgroup delegation and `systemd-run`; a failed slice setup refuses the start rather than running it uncontained.

## Task-based routing

`thread start --job <task> --task-file <full brief>` reuses the task's title, birth sentence and repository. It has no `--role` or `--model`; those arguments are refused. `--workflow` selects instruction text and is available to routing rules. Optional task front matter may set `product = "code"`, `"spec"` or `"web-research"`, and `capability = "<name>"`. The selected recipe must declare that capability. The title and body do not select a recipe.

Routing and executable recipes live together in `~/.config/herdr-ade/config.toml`. Rules are checked in order; every field present on a rule must match. A brief-hash pin wins over the matched rule or default. Unknown keys, empty defaults, unknown or disabled recipe names, malformed pins and rules without a matcher are errors. `doctor` validates the table and flags an enabled recipe with neither a route nor a command. `context` prints one line per recipe with its plain use, capabilities and the rule or choice that reaches it; command syntax stays in the coordinator skill. Disabled recipes have no route.

The coordinator uses routing by default. When Rolf names the coordinator recipe for a project, `open <project> --recipe <id> --basis request:<id>` starts it and stores that exact recipe and request for process relaunches; a request from another project is `request:<project>/<id>`. When Rolf names one for a single lane, the task must cite his request and the start uses `--recipe <id> --basis "<Rolf's exact words>"`. The lane launch record and context keep the recipe, quote and request.

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

A rule may override the global recovery policy with `retries = N`. `ha failed "<failure and evidence>"` reports failed work and retries that same recipe up to the bound. `--class provider --provider-kind <kind>` and `--class lost_connection` also use bounded same-recipe retries; `process_gone` restarts the attempt within the same bound; `unknown` waits for the coordinator. Exhausted recovery stays failed.

Each launch record and dispatch journal row says `pin`, `default`, `explicit` or `rule[n]`, so the reason for selection stays inspectable. An explicit row also carries `recipe_basis` and `recipe_request`. Historical launch and dispatch records without those fields still load.

## The allow-list for your coordinator

The coordinator runs the binary every turn, so allow-list it in your agent **by subcommand, never the bare binary**. `context` prints the exact prefix (`Commands: <binary> --root <root>`); the patterns must start with it. For Claude Code, in the project folder's `.claude/settings.local.json`:

```json
{ "permissions": { "allow": [
  "Bash(<binary> --root <root> skill:*)",
  "Bash(<binary> --root <root> context:*)",
  "Bash(<binary> --root <root> inbox done:*)",
  "Bash(<binary> --root <root> list:*)",
  "Bash(<binary> --root <root> overview:*)",
  "Bash(<binary> --root <root> thread list:*)",
  "Bash(<binary> --root <root> thread show:*)",
  "Bash(<binary> --root <root> thread prompt:*)",
  "Bash(<binary> --root <root> thread ack:*)",
  "Bash(<binary> --root <root> thread retry:*)"
] } }
```

These patterns also cover the here-document form the coordinator uses to pass text on standard input (checked with Claude Code 2.1). A root with spaces is printed shell-quoted; write the pattern for that quoted form.

- **Allow `thread start`** if the coordinator should start work without asking your CLI permission.
- **Never allow** `thread resolve` (with any flag), `thread adopt`, `delete`, `archive`, `pause`, `new`, `open` or `ticker stop`.

For other agents the principle is the same: allow reading and steering, keep anything that starts, ends or deletes on a prompt.

## Inbox notifications

The ticker shows one Herdr notification for each set of new inbox items. The coordinator reads those items in its next context; the ticker never types an inbox or next-step prompt into its pane. If the coordinator pane is missing but its session responds, the ticker attempts to relaunch it with its saved recipe at most once per hour.

## Lane completion deliveries

A lane's typed event line is the wake-up: the ticker types it once into the coordinator's ready pane. `context` reads the current attempt's sealed completion evidence directly, alongside thread and review records. Local and courier completions write no duplicate inbox item; a replacement coordinator reads the same sealed work in context. Only a command the bound coordinator runs (`context`, or `inbox done` for messages) acknowledges a delivery; `--peek` and automation never do. Old thread/round inbox projections are ignored on read, not migrated.

## Threads on other machines

Save the machine with `herdr machine add --label <label> <ssh target>` (both machines need Herdr 0.9.1), then list a repo as `--repo /path/on/machine@<label>` or pass `thread start --machine <label>`. The home machine owns the project; only outbound SSH from home is needed, in batch mode, so set up key-based login first.

A lane or review tries a remote machine by default when it has a repository, `[dispatch] machine = "buildbox"` in `~/.config/herdr-ade/config.toml`, a matching `[machines.buildbox]` declaration, and its recipe kind is allowed by that machine's `kinds` (or `kinds` is omitted). A recipe whose kind is excluded runs locally without a box sign-in check. The repository needs both `box_path` and `publish_url` in its `PROJECT.md`, `[harness]` or machine `repos` row. Without the dispatch key it stays local. `thread start --machine local` keeps one start on the Mac, and `--machine <label>` names a saved machine only when its declaration allows the recipe kind. When a default box start finds the kind excluded, the box held, unreachable, unready, or unable to place the repository, it runs on the Mac instead and says why; an explicit `--machine <label>` still fails.

On a box, `ha done` publishes the lane's or reviewer's own branch to the recorded `publish_url` with a non-force push, verifies the published ref, and only then seals. A failed push refuses with the Git error; retry after resolving it. Mac `done` does not publish.

When a coordinator binding changes, the ticker re-links verified live lanes under its new pane. A missing or mismatched process is not reparented. Use `thread retry` for a gone lane; there is no handoff document pair to maintain.

- The worktree, the brief and the report live on the remote machine. The home ticker polls it once a minute and copies a changed report with `scp` and the thread's `library/` with `rsync -rt` (symbolic links are never followed or copied; a library over 50 MB is not copied and the thread's copy notes say so).
- The box needs `herdr-ade` (`ade_bin`) for lane starts and `ha`, and `herdr-pi` (`pi_bin`) for pi `setup`, `login`, `doctor` and `check`; no pi verb runs through `herdr-ade`.
- Every command the home machine runs on a box goes over SSH with the box machine's configured `path` in front, so it does not depend on login-shell `PATH` edits.
- A machine that doesn't answer is left alone: no state is read, threads keep their last group, and it is skipped for about two minutes. After ten minutes you get one `outage` inbox item, and one more when it is back.
- A blocked remote thread needs you in its pane on that machine: select the machine in Herdr's sidebar, or run `herdr --remote <ssh target>`.
- A start without `--repo` uses the project's sole listed repository. With no listed repository or several, it refuses until the repository is specified.

## Laptop-closed operation

No plugin code is involved: install Herdr and this plugin on an always-on machine, keep the projects root there, open the project there, and attach from your laptop with `herdr --remote <ssh target>` (add `--session <name>` for a named session). The ticker runs on that machine. If Herdr asks whether to restart a remote server "that may not survive SSH connection loss", answering `n` keeps its panes. Checked on a Linux (aarch64) machine from a Mac.

## Development

```bash
cargo test
```

Never develop against your default session or `~/.herdr-ade`. Use `HERDR_ADE_ROOT` and `XDG_CONFIG_HOME` under `/var/tmp`.

`scripts/migration/swap-binary.sh` is the state-dependent install of `~/.local/bin/herdr`. Tests must set `HERDR_ADE_SWAP_DIR` so they never write `~/.local/bin`.
