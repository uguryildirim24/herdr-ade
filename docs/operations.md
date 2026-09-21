# Operations and development

How Herdr ADE works, what it writes where, what its safety settings do and don't stop, and how to run threads on other machines.

## How it works

- **The coordinator is an ordinary agent** in a Herdr pane that follows a skill (`herdr-ade skill` prints it). Plugin code does not route messages, plan work or decide anything.
- **The binary does mechanics.** Starting or restarting a thread, copying reports, marking inbox items handled: each is one deterministic subcommand. It talks to Herdr through Herdr's CLI. The one exception is `focus`/`unfocus`: Herdr 0.9.1 has no CLI command for `agent.view.set`, so those two send one JSON line to the project's socket.
- **Files are the record, prompts are nudges.** Thread and round records own their state; `context` renders it directly at the start of every turn. The inbox holds only messages such as courier deliveries, machine notices and routine runs. A missed prompt loses nothing.
- **One ticker per projects root** checks every 15 seconds: thread state and groups, pending prompts, changed reports, pull requests (every two minutes), routines, auto-resolve. Remote machines are polled once a minute.
- **Tools are found even under a bare `PATH`.** A Herdr server started outside a login shell gives its plugins a minimal `PATH`; the binary appends `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin` and `~/.cargo/bin` to its own, so the ticker finds `gh`, `rsync` and friends. `ticker status` and `doctor` show what resolved.
- **Destructive work is explicit.** The binary never deletes a branch and only removes a worktree or merges in response to the matching command. Text from reports, pull requests and command output is never placed in a prompt.

## Where things live

```
~/.herdr-ade/<project>/
  PROJECT.md              settings (TOML between +++ lines) and your standing instructions; yours
  MEMORY.md, memory/      project memory; the coordinator's
  TASKS.md                the task list; the coordinator's
  routines/<name>.md      routines; the coordinator's
  scratch/                the coordinator's temporary files
  threads/<id>.toml       thread record          threads/<id>.md   home copy of its report
  threads/<id>.task.md    the task as given      threads/<id>/     working folder of a tab thread
  inbox/, inbox/done/     messages with no thread or round home
  library/<id>/           home copy of files a thread produced
  .state/                 status, coordinator pane, ticker state, lock
~/.herdr-ade/.ticker.lock  .ticker.log  .trash/
~/.config/herdr-ade/config.toml             executable model recipes, dispatch placement, machines and harness repositories; any coordinator may edit it
~/.config/herdr-ade/routing.json             one index question, measured model cards, confidence floor and routing floors
~/.config/herdr-ade/approved-routines.json  written only by `routine approve`
```

Every ADE lane works from a plain git worktree at `<repo>/.worktrees/<thread-id>/`, opened as a tab in the coordinator workspace (not as a Herdr worktree workspace). `tab create` sets `HERDR_ADE_LAUNCH`. The lane is primed with `Run <prefix> skill <role>, then read tasks/<id>.md and do what it says.` The thread directory is `<worktree>/.herdr-project/<project>-<id>/`: `report.md` and `library/` (written by the agent); a tab thread with no repository gets its `brief.md` there instead. That folder, and `.worktrees/`, are added to `info/exclude`. **Git treats them as clean: `git worktree remove` without `--force` deletes the checkout**, which is why `--remove-worktree` insists on a complete copy home first. The binary never calls `herdr worktree remove` for an ADE lane.

`PROJECT.md` settings: `name` (the Herdr workspace label; a slug-like name such as `herdr-ade` is stored and shown as `Herdr Ade`, plain title case, so write `GTM AI` yourself if you want capitals; an edited name renames the workspace on the next `open`), `goal`, `repos` (`path`, optional `machine`, `box_path`, `publish_url`), `talk` (default: on for a `claude` coordinator), `max_parallel_threads` (3), `auto_resolve_days` (7), `nudge` (`false`). `coordinator_agent`, `thread_agent` and the two `*_agent_args` keys are gone; `doctor` refuses a `PROJECT.md` that still has them. `[roles.*]` is gone from both files and is refused. Kind and args come from `[recipes.<id>]` (`kind`, `provider`, `args`, `env`, `ready_timeout_ms`, `enabled`, `plain`). At dispatch, the full task and repository facts are scored by Jev; `routing.json` maps the scores to a recipe. Workflow labels such as reviewer select skill text and any explicitly configured routing floor, not a fixed model table.

The birth sentence is required: `thread start` and `thread adopt` take `--plain`. The checker refuses an empty sentence, more than one sentence, or an identifier-shaped token; a thread or round sentence drops the known-word rule, because it is a row on a screen and may name a file. The default workflow is `lane`. `--passive` on adopt sets the parent token and sends no primer.

`config.toml` also carries the harness repositories under `[harness] repos` (rows with `path` and `box_path`, the same shape a project's `repos` rows have). Every project may start a lane or open a round on a harness repository, listed in `PROJECT.md` or not; a repository that is neither listed nor a harness repository is refused. `harness install` builds each harness repository after a merge and installs it into `~/.local/bin`, then the same on a saved box.

## Commands

Every command accepts the global `--json` flag. It returns one record with an
`outcome`, the command name, the ordinary human `message`, and useful ids under
`data`. A refusal exits non-zero and includes its `reason` in that record.

| Command | What it does |
| --- | --- |
| `new <name> [--goal] [--repo PATH[@MACHINE]]...` | Create a project folder. |
| `list [--all]` | Projects with status and thread counts by group. |
| `open <project> [--reprime] [--session N \| --socket P] [--rebind]` | Workspace, coordinator tab and coordinator agent; focuses it when it already runs. |
| `context <project> [--peek]` | The digest the coordinator reads every turn. `--peek` records nothing. |
| `inbox done <project> <item>... \| --all` | Mark inbox items handled. |
| `thread start <project> --title T --plain S [--repo PATH] [--machine M] [--base BRANCH] --task-file F` | The full brief is scored before any worktree or tab exists. Its selected recipe supplies kind and args. The brief `tasks/<id>.md` is committed on the integration branch, then the worktree and tab are created. `--plain` is required; returns before the agent is up. `[dispatch].machine` supplies default box placement for repositories with a box clone. |
| `thread restart`, `thread prompt`, `thread adopt`, `thread list`, `thread show`, `thread ack` | See `--help` on each. |
| `thread resolve <project> <id> [--remove-worktree] [--skip-copy] [--discard-uncopied] [--keep-pane] [--reopen]` | Resolve after the final copy: close the pane and tab through Herdr (`--keep-pane` leaves them), and optionally remove the worktree (the branch is kept). |
| `pickup [<project>] [--all] [--start] [--dry-run]` | Re-link live threads to the coordinator pane: local lanes from the session, box lanes from the courier's box-local lists (one SSH per machine). Gone threads print start lines, or restart through their launch records with `--start` when the project's `start_threads` is `auto`. `--all` covers every active project. |
| `overview [<project>] [--wait]`, `focus [<project>]`, `unfocus` | Threads grouped by what needs you, as text and in the sidebar. |
| `plan show`, `plan set`, `plan step add\|edit\|link\|unlink\|remove\|move`, `plan sync` | The plan card: goal, end result and up to seven steps. A step is `done` only when all its bound work has landed in a merged round. |
| `decide "<line>" --class <what-you-get\|money\|undo\|routine>`, `decide list`, `decide show <id>` | The log of choices the coordinator made without asking. |
| `decide overturn <id> "<reason>"` | Overturn a choice by id, keeping the original and recording who (`USER`), when and why. The screen and context show it as overturned. |
| `ask "<question>?" --choice "<sentence>" --choice "<sentence>"` | Record a question; return one line with its id first. A normalized duplicate of another open question is refused with the existing id. |
| `ask withdraw <id> "<reason>"` | Remove an open question from the board, retaining its record and withdrawal reason, actor (`USER`) and time. Answered questions cannot be withdrawn. |
| `say --what S [--means S] [--landed-round R]` | One checked line on the board and in talk. `--landed-round` marks it as landing evidence for a merged round. |
| `talk <project> [--replay]` | The project screen, or a conversation-only text replay for copying. |
| `routine list`, `routine approve`, `safety show` | Routines and safety settings. |
| `harness install` | Build every repository in `[harness]`, install it into `~/.local/bin`, then the same on the saved box. |
| `pause`, `resume`, `archive`, `unarchive`, `delete [--force]` | Project lifecycle. `delete` moves the folder to `.trash/`. |
| `ticker start \| run \| stop \| status`, `doctor`, `skill` | Housekeeping. |

Groups, first match wins: Resolved; Working while starting; **Waiting on you** (failed, a launch stuck for 60 seconds, a pane gone with no report, or blocked for 30 seconds); **Working**; **Landing** (pull request open and approved); **Ready for review** (a report exists and either its pull request is open or you haven't acknowledged it); Idle. Threads idle for `auto_resolve_days` are resolved after a final copy home.

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

For copying, run `ha talk <project> --replay`: folded conversation only, no live overview, network calls or resending. After the coordinator installs a new binary, restart each existing talk tab with `Ctrl+C`, then `ha talk <project>` in that same shell. `ha open` does not restart a stopped command in an existing tab. Restarting the screen sends nothing.

## Plans and choices

The project screen reads two records the coordinator keeps. They are ordinary files in the project folder; reading them never creates a plan, makes a decision, resolves a lane or answers a question.

- **The plan card** is `<project>/plan.toml`, written under `<project>/.plan.lock` with a revision guard and an atomic rename. It holds the goal copied exactly from `PROJECT.md`, one of seven end-result kinds (`screen`, `command`, `background`, `document`, `picture`, `number`, `finding`) with its fixed sentence, and up to seven ordered steps. A step's `state` is a projection, never a status the coordinator can set: it is `done` only when every bound thread's carrying round has merged with that work included and every bound round has merged, `running` when some required work has started or landed, and `left` otherwise. The shared refresh runs after a thread or round membership change, at each round merge and at each checkpoint, and derives the states from the durable records. `plan sync` is the manual form.
- **The decision log** is `<project>/decisions.jsonl`, appended under `<project>/.decisions.lock`; it is history, not the conversation journal. Each line keeps the length limit but drops the known-word rule, so it may name a file, with class `what-you-get`, `money`, `undo` or `routine`, and optional retry `key`, `basis`, `replaces` and `request` links. A `replaces` record preserves the original instead of editing it; the old choice stops being current only when the replacement is valid. A `--key` retry with the same payload returns the existing record, and the same key with different content fails.

Every message Rolf sends the coordinator is a request with an id in `talk/journal.jsonl`. A talk-tab message gets its id when the screen queues it; a message Rolf types straight into the coordinator pane gets one from the prompt-submit hook (`UserPromptSubmit` for a Claude Code coordinator), which keeps his text verbatim and prints the id. `context` lists the latest ids under "Latest messages from Rolf". Harness lines (priming, nudges, `DONE`/`WAITING`/`FAILED` events) are marked before they are typed and are never recorded as Rolf's.

**Authority boundary.** `what-you-get`, `money` and `undo` are consequential: the coordinator asks before acting, and records one only with `--basis request:<id>` for an existing human message or `--basis ask:<id>@<revision>` for a current, nonzero answered ask. The plugin checks the reference exists and has that provenance; it cannot check that the permission really covers the choice, so the coordinator must. A `no` answer authorizes nothing. `routine` needs no basis. This is not an approval bypass.

**The three-ask cap.** At most three open asks may exist at once, counting a recorded ask whose publication is still pending. Creation, re-asking and answering serialize through `<project>/asks/.open.lock`, so concurrent writers cannot each claim the last slot; a fourth creation fails and records nothing. The newest open ask is re-asked as one merged question: its identifier stays and its revision advances. A project already above the cap can create nothing new until the count is within bounds; its existing asks stay visible.

**Recovery.** A plan or decision write is atomic, so a reader sees an old or a new complete record. A stale `--expect` fails and changes nothing. A decision log whose last line is cut is not read as a decision and blocks further appends until the file is repaired. A failed plan refresh is reported on its own line and never rolls back a merge; the screen reads the authoritative records and shows that the plan needs to catch up until persistence catches up.

When Rolf asks in chat to change a recorded choice, the coordinator treats it like any other message, records the replacement with its `request` link, and says in plain words what will change. Message acceptance is not completion, and a replacement's wording must not claim finished work before it exists.

## Rounds

`.state/rounds/rNN.toml` owns each round's phase, pins, review output intent, accepted verdict and merge/checkpoint transaction. Old round files migrate in place on read; the old merge sidecar is absorbed once and removed. Branches, committed briefs, verdict files and checkpoints are checked outputs of that record. `round show` displays its phase.

A round is a set of lanes that are reviewed and merged together. `hp round advance <slug>` starts the reviewer on its own, including after a REJECT once `hp round review <slug> <round>` makes the next revision. Rounds may be reviewed side by side: later lane tasks, review briefs, verdict files and HANDOFF checkpoints are bookkeeping, so they do not make an earlier verdict stale. When `hp round merge <slug> <round>` finds any other changed path after the brief commit, it makes the next `review/<round>-<n>` revision on the new base and starts a repair reviewer. That reviewer's task names the earlier candidate C and verdict commit V, so it merges C, including the earlier reviewer's fixes, over the new base instead of merging the raw lane shas. This happens even when Git reports a clean merge: textual compatibility does not establish that two independently reviewed changes work together. Merge transactions on the same integration branch take one durable turn at a time; if one is interrupted, retry that round's merge before merging another. `hp round reviewer <slug> <round>` is the manual repair command for a resolved or gone reviewer; it starts and binds a thread with the reviewer skill and reviewer model floor through the same path as `round advance`. A start that does not take says so on standard error and is retried on the next `advance` pass, up to three failures; a round is never left with a bound reviewer whose agent never came up. `hp round abandon <slug> <round> --reason "<why>"` ends a round that will not merge and records why; it refuses after a merge transaction begins.

## Task-based routing

The coordinator does not choose a model. `thread start --task-file <full brief>` has no `--role`, `--recipe` or `--model`; those arguments are refused. `--workflow` selects instruction text and may trigger a policy floor. `hp round reviewer <slug> <round>` starts a hand-run reviewer with the same reviewer skill and configured floor as one started by `round advance`. The title is a display label, never classifier state. Repository facts include HEAD, tracked paths, status and recent changes. The serialized TypeSafe request is capped at 256 KiB: the complete task brief is kept, cheaper repository evidence is removed first, and visible truncation metadata is sent and logged. A brief that cannot fit is refused. Reviewer priming tasks are capped at 128 KiB and always name the committed review-brief path and every pinned commit range; whole source text is inlined only when it fits, because the reviewer can read the named inputs from its checkout.

Install the tracked `config/routing.json` at `~/.config/herdr-ade/routing.json` before starting lanes. There is no embedded policy fallback or old-format reader. Keep executable rows in `config.toml`; remove every `[roles.*]` table. A model card names its published Coding Index, blended price per million tokens and escalation tier. Configured recipe rows replace built-in rows completely, not field by field.

The dispatch process (including a background ticker that starts reviews) must inherit `TYPESAFE_API_KEY`. The client posts to `https://api.typesafe.ai/v1/systemone` with `model: jev-latest` and one Score question: the Coding Index required by the work. Its ordered criteria have matching real-unit `index_values`. No model roster enters the prompt. Exact recipe/model ids are scrubbed; the brief is never reduced to a title or excerpt. The key travels on curl's stdin, not argv, and is removed from its child environment. A non-success HTTP response reports its status and bounded, key-redacted body; timeouts and transport failures remain separate errors.

The raw Score is interpolated between `index_values`. Dispatch chooses the lowest-price model whose published `coding_index` clears that requirement; fixed normalized cutoffs and a hand-ordered price ladder do not exist. Capability tiers must rise with Coding Index, and the strongest model must cover the top index anchor, so an incapable model is never used as a fallback. A result below `confidence_floor` chooses the cheapest capable higher tier. Explicit endpoint size refusals select the highest tier, with any higher role floor, and record `rule: "jev-size-fallback"` plus a redacted cause instead of invented scores. Local request-size guards still refuse briefs that cannot fit; authentication, transport, other HTTP, config and malformed-response failures still refuse.

Floors express non-compensating constraints. The shipped policy keeps the reviewer floor and the top-answer veto:

```json
"role_floors": { "reviewer": "pi_codex_sol_high" },
"answer_floors": [
  { "question": "required_index", "min_score": 3, "recipe": "pi_codex_sol_high" }
]
```

An answer floor compares the raw zero-based Score to `min_score`; fractional thresholds are allowed within the criterion range. Every floor target needs a measured model card and an enabled recipe. After index selection, confidence and escalation, the strongest triggered floor raises the pick only if its tier is higher. Unknown roles/questions, out-of-range thresholds, unknown recipes and disabled targets are refused. Floors do not override the fixed exclusions below.

A raised pick records `rule: "jev-scores-floor"` and a `floors` array next to it. Each entry names the recipe, tier and cause (`kind: "role"` with `role`, or `kind: "answer"` with question, threshold and observed score). All triggered floors above the original pick are recorded, including a weaker floor dominated by another; already-satisfied floors do not claim an upgrade. Jev still scores reviews; a floor is not a pin.

Four exclusions are code rules and never call Jev:

- Web **research as the product** goes to agy. State `product = "web-research"` in opening `+++` TOML task front matter. A coding task that cites URLs does not qualify.
- `requires_claude = true` uses the Claude binary. Coordinators also use the Claude binary, without Jev.
- `product = "spec"` uses Fable for specification writing. Pro through the relay remains available via a user pin.
- Rolf's hand pin is `pins[SHA256(exact task-file bytes)] = recipe-id` in `routing.json`. There is no coordinator CLI pin.

`ha failed "<failure and evidence>"` seals a lane-bound event locally or on the box. The ticker/courier consumes it once, assesses the same full task with the failure, and chooses the cheapest capable model at a strictly stronger tier. It preserves the dirty worktree, replaces only that lane's tab, increments its attempt and tells the replacement what failed. Three upgrades is the hard bound; no stronger measured model or a fixed exclusion produces a recorded refusal. `waiting` still means missing input, not a model failure. Transport/placement errors are visible and never spin through models indefinitely.

`<project>/.state/dispatch.jsonl` is the append-only dispatch ledger: rubric hash, recipe-policy hash, task hash, full score distributions/confidences, arithmetic result, upgrades and failure evidence. The original task file remains the source of brief bytes. Sealed failure events remain in `events/`; a `failure_event` marker and pending-placement state on the thread make replay safe.

### Measure and replay the policy

```sh
ha routing-eval <project>
```

The command makes no model call. It joins the current attempt's sealed completion and first dispatch pick to `.state/dispatch.jsonl` and the carrying round. Each outcome says which model was first tried, the final model, whether the lane escalated, and whether its round merged without a REJECT. The first model is observed good enough only when the lane did not escalate and its round merged without a REJECT. An unfinished round with no REJECT has no outcome yet; it is not guessed bad.

Saved first-pick assessments are replayed through the current policy. When the current policy selects the model that was first tried, the observed outcome confirms it good or bad. A different selection is `untried`, not an invented label. `confidence_clear` reports how many saved assessments meet the current floor. Old dispatch and round records still load; records made before rejection counting or the current one-question rubric remain visible but may be `not-scored`.

## Safety settings

Set per project in `~/.config/herdr-ade/config.toml`; `safety show <project>` prints the table header to use.

```toml
[safety."/Users/you/.herdr-ade/billing"]
start_threads = "propose"          # or "auto": the coordinator starts threads without asking
routine_commands = false           # true lets approved routines run shell commands

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
  "Bash(<binary> --root <root> thread restart:*)"
] } }
```

These patterns also cover the here-document form the coordinator uses to pass text on standard input (checked with Claude Code 2.1). A root with spaces is printed shell-quoted; write the pattern for that quoted form.

- **Allow `thread start` only where you've set `start_threads = "auto"`.** Left off the list, every thread start meets your agent's own permission prompt, which turns "propose first" from skill text into a real confirmation.
- **Never allow** `thread resolve` (with any flag), `thread adopt`, `delete`, `archive`, `pause`, `routine approve`, `new`, `open` or `ticker stop`.

For other agents the principle is the same: allow reading and steering, keep anything that starts, ends or deletes on a prompt.

## What the safety settings do and don't stop

- **They are soft.** Agents have a shell. The guards are the skill text, your agent's permission prompts, keeping the approval list outside every agent's working directory, and `routine approve` refusing without a terminal and a typed confirmation. None of this stops an agent that runs with skip-permission arguments from editing those files directly.
- **A thread can impersonate you.** Any thread agent can prompt the coordinator's pane through Herdr, and that message carries no ticker marker. The skill's rule that a go-ahead must name the threads lowers the risk; it does not remove it.
- **An approved routine command covers the command text only.** `./check.sh` keeps its hash while the script changes.
- **Prompt injection is reduced, not removed.** The coordinator reads reports and may choose to fetch pull request comments itself. Memory is a carrier: whatever it writes there is inlined into every later brief.
- **Cost.** Every thread is a full agent session, and each nudge and each `context` spends coordinator tokens.

## Nudges and notifications

`nudge = false` is the default, because on Herdr 0.9.1 a prompt that arrives while you are typing in the coordinator **is merged with, and submits, your half-typed text**. With it off, the ticker shows one Herdr notification per set of new inbox items ("3 new inbox items") and the coordinator picks them up at its next turn. Set `nudge = true` in `PROJECT.md` to have the ticker prompt the coordinator when it is idle; the message always begins `[herdr-ade ticker: automated, not the user, approves nothing]` and never carries outside text.

## Lane completion deliveries

A lane's `DONE`/`WAITING` line is the wake-up: the ticker types it once into the coordinator's ready pane, so the coordinator is roused even when it already handled the result. `context` reads the current attempt's sealed completion evidence directly, alongside thread and round records. Local completions write no inbox item. A courier import leaves one `courier-delivery` message; a changed recipient leaves a `recipient-changed` message. Only a command the bound coordinator runs (`context`, or `inbox done` for messages) acknowledges a delivery; `--peek` and automation never do. Old thread/round inbox projections are ignored on read, not migrated.

## Routines

A file `routines/<name>.md`: TOML front matter with `schedule` (`every <N>m|h|d` or `daily HH:MM`, local time), optional `command`, `enabled`; the body is the prompt the coordinator receives as an inbox item when it is due. A routine with a `command` runs (`sh -c`, in the project folder, 60 second timeout) only when `routine_commands = true` **and** you have run `herdr-ade routine approve <project> <name>` in a terminal; its output reaches the coordinator capped at 4,000 characters inside a fence labelled as untrusted. Edit the command and it stops until approved again.

## Threads on other machines

Save the machine with `herdr machine add --label <label> <ssh target>` (both machines need Herdr 0.9.1), then list a repo as `--repo /path/on/machine@<label>` or pass `thread start --machine <label>`. The home machine owns the project; only outbound SSH from home is needed, in batch mode, so set up key-based login first.

A lane or review tries the box by default when it has a repository and `[dispatch] machine = "oci"` in `~/.config/herdr-ade/config.toml`. The repository needs both `box_path` and `publish_url` in its `PROJECT.md` row, or a complete committed default mapping. Without the dispatch key it stays local. `thread start --machine local` keeps one start on the Mac, and `--machine <label>` still names any saved machine for any role. When a default box start finds the box held, unreachable, unready, or unable to place the repository, it runs on the Mac instead and says why; an explicit `--machine <label>` still fails.

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
