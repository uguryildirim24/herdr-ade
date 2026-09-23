# Operations and development

How Herdr ADE works, what it writes where, what its safety settings do and don't stop, and how to run threads on other machines.

## How it works

- **The coordinator is an ordinary agent** in a Herdr pane that follows a skill (`herdr-ade skill` prints it). Plugin code does not route messages, plan work or decide anything.
- **The binary does mechanics.** Starting or restarting a thread, copying reports, marking inbox items handled: each is one deterministic subcommand. It talks to Herdr through Herdr's CLI. The one exception is `focus`/`unfocus`: Herdr 0.9.1 has no CLI command for `agent.view.set`, so those two send one JSON line to the project's socket.
- **Files are the record, prompts are nudges.** Thread and round records own their state; `context` renders it directly at the start of every turn. The inbox holds only messages such as courier deliveries, machine notices and routine runs. A missed prompt loses nothing.
- **One ticker per projects root** checks every 15 seconds: thread state and groups, pending prompts, changed reports, pending cleanup, pull requests (every two minutes), and routines. Remote machines are polled once a minute.
- **Tools are found even under a bare `PATH`.** A Herdr server started outside a login shell gives its plugins a minimal `PATH`; the binary appends `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin` and `~/.cargo/bin` to its own, so the ticker finds `gh`, `rsync` and friends. `ticker status` and `doctor` show what resolved.
- **Destructive work is explicit.** The binary never deletes a branch and only removes a worktree or merges in response to the matching command. Text from reports, pull requests and command output is never placed in a prompt.

## Where things live

```
~/.herdr-ade/<project>/
  PROJECT.md              one current page: editable settings, then a binary-written view
  routines/<name>.md      routines; the coordinator's
  scratch/                the coordinator's temporary files
  library/<id>/           files a thread produced for Rolf
  .state/
    notes.jsonl           dated facts and instructions with request ids and replacements
    tasks/job-NNNN.toml   stable tasks: request, acceptance, links and evidence
    threads/<id>.toml     thread record          artifacts/<hash>  sealed final report
    threads/<id>.task.md  the task as given      threads/<id>/     project-owned lane folder
    inbox/, inbox/done/   messages with no thread or round home
    asks/, events/, ops/  proof, delivery and recovery records
    rounds/               pinned round state     history/           archived old documents
~/.herdr-ade/.ticker.lock  .ticker.log  .trash/
~/.config/herdr-ade/config.toml             executable recipes, editable routing, dispatch placement, machines and harness repositories; any coordinator may edit it
~/.config/herdr-ade/approved-routines.json  written only by `routine approve`
```

Content folders in this tree are created on their first write; a new project has only `PROJECT.md` and `.state/`. All binary-owned records live under `.state/`. A project-owned lane folder stays where its thread record says it is.

Every ADE lane works from a plain git worktree at `<repo>/.worktrees/<thread-id>/`, opened as a tab in the coordinator workspace (not as a Herdr worktree workspace). `tab create` sets `HERDR_ADE_LAUNCH`. The lane is primed with `Run <prefix> skill <role>, then read tasks/<id>.md and do what it says.` The thread directory is `<worktree>/.herdr-project/<project>-<id>/`: the agent writes `report.md` and creates `library/` only for real deliverables; a tab thread with no repository gets its `brief.md` there instead. That folder, and `.worktrees/`, are added to `info/exclude`. `done` seals the report once as `.state/artifacts/<hash>`; thread and task views find it from the thread record. Unmatched historical `.state/threads/<id>.md` reports remain readable. Resolving removes a finished worktree with `git worktree remove` without `--force`, after copying real deliverables. A remote lane's rebuildable Cargo folder is removed in the same operation. Uncommitted tracked or untracked changes refuse resolution. Ignored data keeps both the worktree and its build folder but does not stop resolution; the typed `ignored_data` reason names each folder and its size. The branch is always kept.

Ignored files are disposable only when their path is covered by the editable global setting below or by `disposable` on that repository's row in `PROJECT.md`. A harness repository row in `config.toml` may carry the same list. Repository lists are added to the global list only for their own repository. With no matching setting, every ignored file is treated as data. A one-part name matches that path component anywhere in the worktree; a path containing `/` matches from the worktree root. `*` matches within one path part (`runs/pytest-*` covers `runs/pytest-cancel` but not `runs/seed-1`). A nested Git checkout is always data, even inside a disposable folder.

```toml
[worktrees]
disposable = ["target", ".target", "zig-out", ".zig-cache", "node_modules"]
```

`doctor` reports resolved worktrees retained for ignored data separately from clean resolved worktrees that were accidentally left behind. It lists remote build folders with no open thread, including their sizes, in the finished-worktree row. An open thread's worktree is never called finished; unreadable thread state makes the row unknown. Closed-round review worktrees use the same check. It also reports free space on the local machine and every saved remote machine. The failure threshold is editable and defaults to 12 GB:

```toml
[doctor]
min_free_disk_gb = 12
```

Only the front matter of `PROJECT.md` is hand-edited. Its body is rebuilt atomically from the records and shows the goal, what Rolf gets, waits, running work, plan, each open task on one status-and-next-action line, current task notes, instructions and facts, recent decisions and recent completions. `task show` carries the task's full acceptance conditions and evidence. A rewrite compares the front matter again before replacing the page, so it will not overwrite a concurrent settings edit. `PROJECT.md` settings: `name` (the Herdr workspace label; a slug-like name such as `herdr-ade` is stored and shown as `Herdr Ade`, plain title case, so write `GTM AI` yourself if you want capitals; an edited name renames the workspace on the next `open`), `goal`, `task_states` (ordered task milestones; defaults to `finished`, `reviewed`, `merged`), `repos` (`path`, optional `machine`, `box_path`, `publish_url`, `disposable`, integration `branch`, allowed `push_remote`, repository `gates`, and a repository-specific `task_states` override), `talk` (default: on for a `claude` coordinator), `nudge` (`false`). Each gate is `{ command = "...", env = { NAME = "value" } }`; omitted `gates` means not configured while `gates = []` explicitly makes that repository gate-free. A project-wide `gates` key is removed. Projects have no thread-count limit. `max_parallel_threads`, `coordinator_agent`, `thread_agent` and the two `*_agent_args` keys are gone; `doctor` refuses a `PROJECT.md` that still has them. `[roles.*]` is gone from both files and is refused. Kind and args come from `[recipes.<id>]` (`kind`, `provider`, `args`, `env`, `ready_timeout_ms`, `enabled`, `plain`). The ordered `[routing]` table selects a recipe by workflow or task product.

A thread birth sentence is required. `thread start --job` takes it, the title and the repository from the stable task; `--plain`, `--title` and `--repo` override only real lane differences. A newly created task still needs an explicit title, while `thread adopt` still takes `--plain`. Internal thread and round sentences keep their one-sentence structure without vocabulary or length checks. The default workflow is `lane`. `--passive` on adopt sets the parent token and sends no primer.

`config.toml` also carries the harness repositories under `[harness] repos` (the same repository-row shape). Every project may start a lane or open a round on a harness repository, listed in `PROJECT.md` or not; a repository that is neither listed nor a harness repository is refused. After `round merge` pushes the configured integration ref, repositories whose task milestones include `installed` use the harness installer: it builds each harness repository into `~/.local/bin`, then does the same on a saved box. `harness install` remains the standalone repair command.

## Commands

Every command accepts the global `--json` flag. It returns one record with an
`outcome`, the command name, the ordinary human `message`, and useful ids under
`data`. A refusal exits non-zero and includes its `reason` in that record. Every command that targets one project takes its slug as an explicit positional argument; it never guesses from the number of remaining words or the current workspace.

| Command | What it does |
| --- | --- |
| `new <name> [--goal] [--repo PATH[@MACHINE]]...` | Create a project folder with its one current page. |
| `list [--all]` | Projects with status and thread counts by group. |
| `open <project> [--recipe ID] [--reprime] [--session N \| --socket P] [--rebind]` | Workspace, coordinator tab and coordinator agent; focuses it when it already runs. `--recipe` chooses a configured recipe for a new coordinator, and that binding keeps the choice when its process relaunches. |
| `context <project> [--peek]` | The `PROJECT.md` body followed by new messages from Rolf, unhandled inbox items, current failures, work needing action and the compact recipe list. `--peek` records nothing. |
| `inbox done <project> <item>... \| --all` | Mark inbox items handled. |
| `task add`, `task show`, `task list`, `task evidence`, `task drop` | Create and inspect stable tasks and record per-condition verification. Each `--acceptance` value stays one condition even when it contains several short sentences, which are checked separately. When a newer choice replaces one condition, withdraw it with `task drop --acceptance N --reason` and name that choice in the reason. State is derived from linked events and rounds. A task without a repository needs only `finished` and `verified`, and may go straight to verification without an attempt. |
| `note add <project> <text> --kind memory\|instruction --request <id> [--task <job>] [--replaces <id>]` | The only fact and instruction writer. It may scope the row to a task or explicitly replace an older row. |
| `thread start <project> --job TASK --task-file F [--title T] [--plain S] [--repo PATH] [--machine M] [--base BRANCH]` | The stable task supplies title, birth sentence and repository. Override flags describe a real lane difference. Omitting `--job` creates a task only when `--title`, `--request` and `--acceptance` are present. Routing is resolved before any worktree or tab exists. The frozen brief `tasks/<id>.md` carries the stable task, applicable current instructions and facts, repository, machine, pinned repository gates and finish paths; it is committed before the worktree and tab are created. The command returns before the agent is up. |
| `thread prompt`, `thread list`, `thread show` | Everyday follow-up and inspection. `thread --help` groups the remaining recovery and administration commands. |
| `thread attest <project> <id> --reason S` | Seal `done` from a resolved, uncancelled lane's preserved report draft or unmatched historical report after its bytes match the recorded hash; records the coordinator and reason. |
| `thread resolve <project> <id> [--skip-copy] [--discard-uncopied] [--keep-pane] [--reopen]` | Resolve after the final copy and close the pane and tab. A landed or closed-round worktree is removed only when it has no changes or non-disposable ignored data. Changes refuse resolution; ignored data resolves the thread but keeps the worktree with folder sizes. The branch is always retained. |
| `pickup [<project>] [--all] [--start] [--dry-run]` | Re-link live threads to the coordinator pane: local lanes from the session, box lanes from the courier's box-local lists (one SSH per machine). Gone threads print start lines, or restart through their launch records with `--start` when the project's `start_threads` is `auto`. `--all` covers every active project. |
| `overview <project> [--history] [--wait]`, `focus <project>`, `unfocus` | Active threads grouped by what needs you. `--history` also shows resolved threads. |
| `plan show <project>`, `plan set <project>`, `plan step add\|edit\|link\|unlink\|remove\|move <project>`, `plan sync <project>` | The plan card: goal, end result and up to seven steps. New links use repeatable `--task`; a task may link to several steps. Historical thread, round and task-side step bindings still load and show. |
| `decide <project> "<line>" --class <what-you-get\|money\|undo\|routine>`, `decide list <project>`, `decide show <project> <id>` | The log of choices the coordinator made without asking. |
| `decide overturn <project> <id> "<reason>"` | Overturn a choice by id, keeping the original and recording who (`USER`), when and why. The screen and context show it as overturned. |
| `ask <project> "<question>?" --choice "<sentence>" --choice "<sentence>"` | Record a question; return one line with its id first. A normalized duplicate of another open question is refused with the existing id. |
| `ask withdraw <project> <id> "<reason>"` | Remove an open question from the board, retaining its record and withdrawal reason, actor (`USER`) and time. Answered questions cannot be withdrawn. |
| `say <project> --what S [--means S] [--landed-round R]` | One checked line on the board and in talk, returning its say id. `--landed-round` marks it as landing evidence for a merged round. |
| `talk <project> [--replay]` | The project screen, or a conversation-only text replay for copying. |
| `routine list`, `routine approve`, `safety show` | Routines and safety settings. |
| `round open <project> [<round>] [<thread>...] [--repo DIR] [--branch BRANCH] --plain S` | Open one round past the highest number already used by a record, local review branch, or committed review file, and admit the supplied lanes at once. Lanes infer their single repository; an open without lanes refuses an ambiguous project. |
| `round merge <project> <round>` | Merge, checkpoint, push to the repository's allowed remote, install when required, and close the round. Retry resumes a pending push or install without merging twice. |
| `round show <project> [<round>]` | Show one round; without a round, list every round in the project. |
| `harness install` | Standalone repair: build every repository in `[harness]`, install it into `~/.local/bin`, then the same on the saved box. |
| `pause`, `resume`, `archive`, `unarchive`, `delete [--preview] [--github]` | Project lifecycle. `delete` stops project-owned processes and sends owned local files to the macOS Trash; shared resources stay. GitHub deletion is explicit. |
| `ticker start \| run \| stop \| status`, `doctor`, `skill` | Housekeeping. |

Groups, first match wins: Resolved; Working while starting; **Waiting on you** (failed, a launch stuck for 60 seconds, a process gone with no report, or blocked for 30 seconds); **Unknown** for a box lane that has not been polled; **Working**; **Landing** (pull request open and approved); **Ready for review** (a report exists and either its pull request is open or you haven't acknowledged it); Idle. A report-only lane whose sealed commit equals its base closes after its final copy. A changed lane stays visible until a round carries it.

`focus` replaces any sidebar view another tool has set, and `unfocus` clears whatever view is set, because Herdr holds a single one. `focus` covers local threads only.

## Project screen

`ha talk <project>` opens an alternate screen: goal, end result, completed-step count, running work, five recent landings, and questions with five current choices. The conversation stays below. At 80 pane columns the overview scrolls separately above chat; below 80 (or in a short pane) the overview and chat share one scroll area, starting at the goal. Open questions stay beside chat or above the composer. Every active work row remains reachable; closing a handed-in lane does not make its work land.

- `F2`: full overview, without losing input or prior scroll positions.
- `F6`: change scroll area; `Page up`, `Page down`, `Home`, `End`: move within it. `End` in chat resumes following new messages. The wheel scrolls the area under it; clicking a question selects it, never answers it.
- `Tab`: next open question, returning from full overview if needed. Selection stays put when another question arrives.
- `0` to the displayed choice count: answer only with an empty composer and a visible selected card. A pasted digit, a digit after a space, or a digit with cards hidden is message text. Answers bind to the revision actually drawn, not a replacement arriving later.
- `Enter`: send; `Esc`: clear; arrows and `Backspace`: edit; `Ctrl+C`: restore the normal terminal and leave the shell usable.

Type an ordinary message to change a recorded choice. `queued`, `sent`, `unsure`, and `accepted` describe delivery, not completion. `/…`, `!stop`, `!native`, and `!back` keep their existing handling; suspension shows `other tab`.

The screen is read-only until an explicit message, answer or command. It reuses the plan projection, current decision fold, durable thread/round records, published asks and verified landing references. Remote work is marked `box last seen`: the courier's stored state is not a fresh remote probe. Local records refresh every 250 ms; one shared herdr poll every three seconds supplies local live state. Missing records show honest empty or error text, not guessed completion.

Colours come from herdr's `[theme.custom]` in `$XDG_CONFIG_HOME/herdr/config.toml`, otherwise its platform config directory. Invalid tokens fall back individually to the carried catppuccin palette; the pane background remains the terminal default.

For copying, run `ha talk <project> --replay`: folded conversation only, no live overview, network calls or resending. A running project screen hands over to a newly installed binary itself. If it prints that the handover failed, exit and run `ha talk <project>` again in the same shell.

## Plans and choices

The project screen reads two records the coordinator keeps. They are ordinary files in the project folder; reading them never creates a plan, makes a decision, resolves a lane or answers a question.

- **The plan card** is `<project>/.state/plan.toml`, written under `<project>/.state/plan.lock` with a revision guard and an atomic rename. It holds the goal copied exactly from `PROJECT.md`, one of seven end-result kinds and up to seven ordered steps. New bindings name stable tasks in each step, so one task may support several steps. A step's state is projected from those tasks. Historical cards with thread or round bindings, and historical tasks with `plan_step`, still load, show and project without becoming new write paths. `plan sync` is the manual refresh.
- **The decision log** is `<project>/.state/decisions.jsonl`, appended under `<project>/.state/decisions.lock`; it is history, not the conversation journal. Each internal line keeps exact files, identifiers and long technical detail; technical views wrap or collapse it rather than applying the audience prose check. A line has class `what-you-get`, `money`, `undo` or `routine`, and optional retry `key`, `basis`, `replaces` and `request` links. A `replaces` record preserves the original instead of editing it; the old choice stops being current only when the replacement is valid. A `--key` retry with the same payload returns the existing record, and the same key with different content fails.

Every message Rolf sends the coordinator is a request with an id in `.state/talk/journal.jsonl`. A talk-tab message gets its id when the screen queues it; a message Rolf types straight into the coordinator pane gets one from the prompt-submit hook (`UserPromptSubmit` for a Claude Code coordinator), which keeps his text verbatim and prints the id. The same hook starts the turn record. `ha say` and `ha ask` are the only authored paths to the talk tab: they publish under the say id or ask id and revision, record each sink result, and leave a receipt for that turn. The stop hook checks only that receipt. It never reads or publishes reply prose, and sends the coordinator back to run `ha say` when the receipt is missing. `context` lists the latest request ids under "Latest messages from Rolf". Harness lines (priming, nudges, `DONE`/`WAITING`/`FAILED` events, cross-session messages and idle notices) are automated and never count as Rolf's request.

**Authority boundary.** `what-you-get`, `money` and `undo` are consequential: the coordinator asks before acting, and records one only with `--basis request:<id>` for an existing human message or `--basis ask:<id>@<revision>` for a current, nonzero answered ask. The plugin checks the reference exists and has that provenance; it cannot check that the permission really covers the choice, so the coordinator must. A `no` answer authorizes nothing. `routine` needs no basis. This is not an approval bypass.

**The three-ask cap.** At most three open asks may exist at once, counting a recorded ask whose publication is still pending. Creation, re-asking and answering serialize through `<project>/.state/asks/.open.lock`, so concurrent writers cannot each claim the last slot; a fourth creation fails and records nothing. The newest open ask is re-asked as one merged question: its identifier stays and its revision advances. A project already above the cap can create nothing new until the count is within bounds; its existing asks stay visible.

**Recovery.** A plan or decision write is atomic, so a reader sees an old or a new complete record. A stale `--expect` fails and changes nothing. A decision log whose last line is cut is not read as a decision and blocks further appends until the file is repaired. A failed plan refresh is reported on its own line and never rolls back a merge; the screen reads the authoritative records and shows that the plan needs to catch up until persistence catches up.

When Rolf asks in chat to change a recorded choice, the coordinator treats it like any other message, records the replacement with its `request` link, and says in plain words what will change. Message acceptance is not completion, and a replacement's wording must not claim finished work before it exists.

## Rounds

`.state/rounds/rNN.toml` owns each round's phase, pins, review output intent, accepted verdict and merge/checkpoint transaction. Old round files migrate in place on read; the old merge sidecar is absorbed once and removed. Branches, committed briefs, verdict files and checkpoints are checked outputs of that record. `round show` displays its phase.

A round is a set of lanes that are reviewed and merged together. Its selected repository row pins the gate commands and environment at open; the review verdict must cover every pinned command with its exit status, while the report retains the actual output. `round merge` checkpoints, pushes the integration ref to `push_remote`, runs installation when `task_states` includes `installed`, then makes the final copy, closes every member lane and reviewer, and removes clean worktrees while retaining branches. Its result names the pushed ref and prints the installer's binary versions and running-process proof, just like `harness install`; JSON keeps that typed proof under `effects`. A failed push or install leaves the round merged with that step pending; the same command resumes only the outstanding step. A cleanup failure does not undo the merge or cancellation: the thread shows cleanup pending and the ticker retries it. `hp round advance <slug>` starts the reviewer on its own, including after a REJECT once `hp round review <slug> <round>` makes the next revision. Rounds may be reviewed side by side: later lane tasks, review briefs, verdict files and HANDOFF checkpoints are bookkeeping, so they do not make an earlier verdict stale. When `hp round merge <slug> <round>` finds any other changed path after the brief commit, it makes the next `review/<round>-<n>` revision on the new base and starts a repair reviewer. That reviewer's task names the earlier candidate C and verdict commit V, so it merges C, including the earlier reviewer's fixes, over the new base instead of merging the raw lane shas. This happens even when Git reports a clean merge: textual compatibility does not establish that two independently reviewed changes work together. Merge transactions on the same integration branch take one durable turn at a time; if one is interrupted, retry that round's merge before merging another. Recovery uses the same four verbs as threads: `round retry` replaces the reviewer attempt without making a duplicate, `round cancel` stops the round and cleans its processes, `round rebind` binds a verified live reviewer thread, and `round adopt` accepts valid sealed lane work or a verdict whose candidate, manifest and policy hashes match. A start that does not take says so and remains bounded by routing.

## Agent and machine adapters

Agent behavior lives in `[adapters.<kind>]`. A complete row declares `binary`, `launch_flags`, `ready_timeout_ms`, `coordinator`, `talk`, `capabilities`, required flags and effort names, a doctor readiness driver and argument template (`{args}` expands to the routed recipe), and its hook path, JSON shape, events, prompt event and block response. A kind may coordinate only when its native hooks expose prompt submission, so text Rolf types into its pane cannot disappear. Shipped rows use the same declaration type. A new kind needs only this row unless its provider has a non-command readiness protocol.

Machine facts live in `[machines.<name>]`: `target`, `session`, `home`, `root`, `worktrees`, `build`, `path`, `ade_bin`, `pi_bin`, `kinds`, and `repos`. `kinds` is the list of adapter kinds the machine runs, such as `kinds = ["pi"]`; an empty list runs no agent jobs there. Omitting `kinds` leaves an existing user machine unrestricted, so every adapter kind may run there. Each repo row names `path`, `box_path`, and `publish_url`. Placement, doctor probes, lane environment, start lines, courier paths and cleanup resolve the selected machine row; another box does not add a code branch.

## Task-based routing

`thread start --job <task> --task-file <full brief>` reuses the task's title, birth sentence and repository. It has no `--role` or `--model`; those arguments are refused. `--workflow` selects instruction text and is available to routing rules. Optional task front matter may set `product = "code"`, `"spec"` or `"web-research"`, and `capability = "<name>"`. The selected recipe must declare that capability. The title and body do not select a recipe.

Routing and executable recipes live together in `~/.config/herdr-ade/config.toml`. Rules are checked in order; every field present on a rule must match. A brief-hash pin wins over the matched rule or default. Unknown keys, empty defaults, unknown or disabled recipe names, malformed pins and rules without a matcher are errors. `doctor` validates the table and flags an enabled recipe with neither a route nor a command. `context` prints one line per recipe with its plain use, capabilities and the rule or choice that reaches it; command syntax stays in the coordinator skill. Disabled recipes have no route.

A coordinator still never chooses a model. When Rolf names the coordinator recipe for a project, `open <project> --recipe <id>` starts it and stores that exact recipe for process relaunches. When Rolf names one for a single lane, the task must cite his request and the start uses `--recipe <id> --basis "<Rolf's exact words>"`. The lane launch record, context and decision log keep the recipe, quote and request. A non-default exact lane choice is recorded as a money decision rather than guessing prices in core logic. Pro is command-only and Mac-only: run `herdr-pro start --name <n> --cwd <dir> &`, then `herdr-pro turn <n> --brief <f> --out <f> --notify <coordinator-agent> [--attach <f>]`; never type into its pane.

The starting table is:

```toml
[routing]
default = "pi_codex_sol_high"
retries = 1
fallback = []

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

A rule may override the global recovery policy with `retries = N` and `fallback = ["recipe-a", "recipe-b"]`. `ha failed "<failure and evidence>"` reports failed work by default: it retries up to that bound, then tries each fallback once in order. `--class provider --provider-kind <kind>` and `--class lost_connection` use only bounded same-recipe retries; `process_gone` restarts the attempt within the same bound; `unknown` waits for the coordinator. When recovery is exhausted, no other recipe is guessed.

Each launch record and dispatch-ledger row says `pin`, `default`, `explicit` or `rule[n]`, so the reason for selection stays inspectable. An explicit row also carries `recipe_basis` and `recipe_request`. Historical launch, dispatch and round records without those fields still load.

## Safety settings

Set per project in `~/.config/herdr-ade/config.toml`; `safety show <project>` prints the table header to use.

```toml
[safety."/Users/you/.herdr-ade/billing"]
start_threads = "auto"             # default; use "propose" to wait before starts
routine_commands = false           # true lets approved routines run shell commands

[coordinator]
idle_nudge_minutes = 20             # minimum time between continue prompts

[dispatch]
machine = "oci"                    # default placement for repositories with a box clone
```

The table is keyed by the project folder's canonical path. It stays when you delete the project and applies to a new project at the same path.

## The allow-list for your coordinator

The coordinator runs the binary every turn, so allow-list it in your agent **by subcommand, never the bare binary**. `context` prints the exact prefix (`Commands: <binary> --root <root>`); the patterns must start with it. For Claude Code, in the project folder's `.claude/settings.local.json`:

```json
{ "permissions": { "allow": [
  "Bash(<binary> --root <root> skill:*)",
  "Bash(<binary> --root <root> context:*)",
  "Bash(<binary> --root <root> inbox done:*)",
  "Bash(<binary> --root <root> list:*)",
  "Bash(<binary> --root <root> overview:*)",
  "Bash(<binary> --root <root> safety show:*)",
  "Bash(<binary> --root <root> routine list:*)",
  "Bash(<binary> --root <root> thread list:*)",
  "Bash(<binary> --root <root> thread show:*)",
  "Bash(<binary> --root <root> thread prompt:*)",
  "Bash(<binary> --root <root> thread ack:*)",
  "Bash(<binary> --root <root> thread retry:*)"
] } }
```

These patterns also cover the here-document form the coordinator uses to pass text on standard input (checked with Claude Code 2.1). A root with spaces is printed shell-quoted; write the pattern for that quoted form.

- **Allow `thread start` for the default `start_threads = "auto"` mode.** Leave it off only for a project explicitly set to `propose`; then every proposed start meets your agent's own permission prompt.
- **Never allow** `thread resolve` (with any flag), `thread adopt`, `delete`, `archive`, `pause`, `routine approve`, `new`, `open` or `ticker stop`.

For other agents the principle is the same: allow reading and steering, keep anything that starts, ends or deletes on a prompt.

## What the safety settings do and don't stop

- **They are soft.** Agents have a shell. The guards are the skill text, your agent's permission prompts, keeping the approval list outside every agent's working directory, and `routine approve` refusing without a terminal and a typed confirmation. None of this stops an agent that runs with skip-permission arguments from editing those files directly.
- **A thread can impersonate you.** Any thread agent can prompt the coordinator's pane through Herdr, and that message carries no ticker marker. The skill's rule that a go-ahead must name the threads lowers the risk; it does not remove it.
- **An approved routine command covers the command text only.** `./check.sh` keeps its hash while the script changes.
- **Prompt injection is reduced, not removed.** The coordinator reads reports and may choose to fetch pull request comments itself. Current unscoped memory reaches later briefs; task-scoped and replaced notes are filtered by the binary.
- **Cost.** Every thread is a full agent session, and each nudge and each `context` spends coordinator tokens.

## Nudges and notifications

`nudge` in `PROJECT.md` controls new-inbox announcements. With it off, the ticker shows one Herdr notification per set of new inbox items ("3 new inbox items"); with it on, the ticker prompts an idle coordinator. The message always begins `[herdr-ade ticker: automated, not the user, approves nothing]` and never carries outside text.

The ticker also prompts an idle coordinator to continue when tasks have an available next step. It does not do that while Rolf's message is waiting, a lane is working, or every task waits on Rolf. `[coordinator].idle_nudge_minutes` in `config.toml` is the minimum interval (20 by default), and a coordinator turn must occur before another continue prompt.

## Lane completion deliveries

A lane's typed event line is the wake-up: the ticker types it once into the coordinator's ready pane. `context` reads the current attempt's sealed completion evidence directly, alongside thread and round records. Local and courier completions write no duplicate inbox item; a changed recipient leaves a `recipient-changed` message. Only a command the bound coordinator runs (`context`, or `inbox done` for messages) acknowledges a delivery; `--peek` and automation never do. Old thread/round inbox projections are ignored on read, not migrated.

## Routines

A file `routines/<name>.md`: TOML front matter with `schedule` (`every <N>m|h|d` or `daily HH:MM`, local time), optional `command`, `enabled`; the body is the prompt the coordinator receives as an inbox item when it is due. A routine with a `command` runs (`sh -c`, in the project folder, 60 second timeout) only when `routine_commands = true` **and** you have run `herdr-ade routine approve <project> <name>` in a terminal; its output reaches the coordinator capped at 4,000 characters inside a fence labelled as untrusted. Edit the command and it stops until approved again.

## Threads on other machines

Save the machine with `herdr machine add --label <label> <ssh target>` (both machines need Herdr 0.9.1), then list a repo as `--repo /path/on/machine@<label>` or pass `thread start --machine <label>`. The home machine owns the project; only outbound SSH from home is needed, in batch mode, so set up key-based login first.

A lane or review tries the box by default when it has a repository, `[dispatch] machine = "oci"` in `~/.config/herdr-ade/config.toml`, and its recipe kind is allowed by that machine's `kinds` (or `kinds` is omitted). The shipped `oci` declaration runs `pi`; Claude and agy jobs therefore start on the Mac and their box sign-ins are not checked. The repository needs both `box_path` and `publish_url` in its `PROJECT.md` row, or a complete committed default mapping. Without the dispatch key it stays local. `thread start --machine local` keeps one start on the Mac, and `--machine <label>` names a saved machine only when its declaration allows the recipe kind. When a default box start finds the kind excluded, the box held, unreachable, unready, or unable to place the repository, it runs on the Mac instead and says why; an explicit `--machine <label>` still fails.

When you close your Mac session and start a new one, a box lane keeps running on the box but loses the link to its coordinator. `ha pickup` reads each machine once through the courier, re-links the living box lanes under the new coordinator pane, and prints a `herdr --machine <label>` start line for each gone one. `ha pickup --all --start` does that for every active project and restarts the gone lanes, but only where `start_threads = "auto"`.

- The worktree, the brief and the report live on the remote machine. The home ticker polls it once a minute and copies a changed report with `scp` and the thread's `library/` with `rsync -rt` (symbolic links are never followed or copied; a library over 50 MB is not copied and the thread's copy notes say so).
- The box holds two plugin binaries: `/home/ubuntu/.local/bin/herdr-ade` (lane start and `ha`) and `/home/ubuntu/.local/bin/herdr-pi` (pi `setup`, `login`, `doctor` and `check`); no pi verb runs through `herdr-ade`.
- Every command the home machine runs on a box goes over SSH with the fixed box `PATH` `/home/ubuntu/.local/bin:/home/ubuntu/.cargo/bin:/usr/local/bin:/usr/bin:/bin` in front, so a fresh box needs no login-shell `PATH` edits for the plugin.
- A machine that doesn't answer is left alone: no state is read, threads keep their last group, and it is skipped for about two minutes. After ten minutes you get one `outage` inbox item, and one more when it is back.
- A blocked remote thread needs you in its pane on that machine: select the machine in Herdr's sidebar, or run `herdr --remote <ssh target>`.
- `focus` does not cover remote threads: their sidebar tokens are set on the remote Herdr server. They appear in `overview`, `thread list` and `context`.
- Tasks with no repository always run locally, as tabs.

## Laptop-closed operation

No plugin code is involved: install Herdr and this plugin on an always-on machine, keep the projects root there, open the project there, and attach from your laptop with `herdr --remote <ssh target>` (add `--session <name>` for a named session). The ticker runs on that machine. If Herdr asks whether to restart a remote server "that may not survive SSH connection loss", answering `n` keeps its panes. Checked on a Linux (aarch64) machine from a Mac.

## Development

```bash
cargo test
```

Never develop against your default session or `~/.herdr-ade`. Use `HERDR_ADE_ROOT` and `XDG_CONFIG_HOME` under `/var/tmp`.

`scripts/migration/swap-binary.sh` is the state-dependent install of `~/.local/bin/herdr`. Tests must set `HERDR_ADE_SWAP_DIR` so they never write `~/.local/bin`. Run `scripts/migration/swap-binary.test.sh` on this Mac.
