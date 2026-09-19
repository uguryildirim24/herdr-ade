# Operations and development

How Herdr Projects works, what it writes where, what its safety settings do and don't stop, and how to run threads on other machines.

## How it works

- **The coordinator is an ordinary agent** in a Herdr pane that follows a skill (`herdr-ade skill` prints it). Plugin code does not route messages, plan work or decide anything.
- **The binary does mechanics.** Starting or restarting a thread, copying reports, marking inbox items handled: each is one deterministic subcommand. It talks to Herdr through Herdr's CLI. The one exception is `focus`/`unfocus`: Herdr 0.9.1 has no CLI command for `agent.view.set`, so those two send one JSON line to the project's socket.
- **Files are the record, prompts are nudges.** Threads write a report file, the ticker writes events to an inbox folder, and the coordinator re-reads state with `context` at the start of every turn. A missed prompt loses nothing.
- **One ticker per projects root** checks every 15 seconds: thread state and groups, pending prompts, changed reports, pull requests (every two minutes), routines, auto-resolve. Remote machines are polled once a minute.
- **Tools are found even under a bare `PATH`.** A Herdr server started outside a login shell gives its plugins a minimal `PATH`; the binary appends `/opt/homebrew/bin`, `/usr/local/bin`, `~/.local/bin` and `~/.cargo/bin` to its own, so the ticker finds `gh`, `rsync` and friends. `ticker status` and `doctor` show what resolved.
- **Nothing destructive is automatic.** The binary never removes a worktree, deletes a branch, merges or pushes on its own. Text from reports, pull requests and command output is never placed in a prompt.

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
  inbox/, inbox/done/     events for the coordinator
  library/<id>/           home copy of files a thread produced
  .state/                 status, coordinator pane, ticker state, lock
~/.herdr-ade/.ticker.lock  .ticker.log  .trash/
~/.config/herdr-ade/config.toml             yours, edited by hand; holds the roles table
~/.config/herdr-ade/approved-routines.json  written only by `routine approve`
```

Every ADE lane works from a plain git worktree at `<repo>/.worktrees/<thread-id>/`, opened as a tab in the coordinator workspace (not as a Herdr worktree workspace). `tab create` sets `HERDR_ADE_LAUNCH`. The lane is primed with `Run <prefix> skill <role>, then read tasks/<id>.md and do what it says.` The thread directory is `<worktree>/.herdr-project/<project>-<id>/`: `report.md` and `library/` (written by the agent); a tab thread with no repository gets its `brief.md` there instead. That folder, and `.worktrees/`, are added to `info/exclude`. **Git treats them as clean: `git worktree remove` without `--force` deletes the checkout**, which is why `--remove-worktree` insists on a complete copy home first. The binary never calls `herdr worktree remove` for an ADE lane.

`PROJECT.md` settings: `name` (the Herdr workspace label; a slug-like name such as `herdr-ade` is stored and shown as `Herdr Ade`, plain title case, so write `GTM AI` yourself if you want capitals; an edited name renames the workspace on the next `open`), `goal`, `repos` (`path`, optional `machine`), `talk` (default: on for a `claude` coordinator), `max_parallel_threads` (3), `auto_resolve_days` (7), `nudge` (`false`). `coordinator_agent`, `thread_agent` and the two `*_agent_args` keys are gone; `doctor` refuses a `PROJECT.md` that still has them. Kind and args come from the roles table in `~/.config/herdr-ade/config.toml` (`[roles.<name>]` with `kind`, `args`, `env`, `ready_timeout_ms`). Day-one roles: `coordinator`, `lane`, `reviewer`, `critic`, `drafter`, `pro`. A project may override `kind` and `args` per role; arrays replace, they do not merge. A kind change without `args` is refused.

The birth sentence is required: `thread start` and `thread adopt` take `--plain`. The checker refuses an empty sentence, more than one sentence, or a sentence that fails R1–R5. Default role is `lane`. `--passive` on adopt sets the parent token and sends no primer.

## Commands

| Command | What it does |
| --- | --- |
| `new <name> [--goal] [--repo PATH[@MACHINE]]...` | Create a project folder. |
| `list [--all]` | Projects with status and thread counts by group. |
| `open <project> [--reprime] [--session N \| --socket P] [--rebind]` | Workspace, coordinator tab and coordinator agent; focuses it when it already runs. |
| `context <project> [--peek]` | The digest the coordinator reads every turn. `--peek` records nothing. |
| `inbox done <project> <item>... \| --all` | Mark inbox items handled. |
| `thread start <project> --title T --plain S [--role R] [--repo PATH] [--machine M] [--base BRANCH] --task-file F` | New thread: the brief `tasks/<id>.md` is committed on the integration branch (`--base`, else the checked-out branch), then a git worktree from that commit and a tab under the coordinator, with `--parent` on launch. The kind and args come from the role. `--plain` is required. Returns before the agent is up. Remote starts are refused. |
| `thread restart`, `thread prompt`, `thread adopt`, `thread list`, `thread show`, `thread ack` | See `--help` on each. |
| `thread resolve <project> <id> [--remove-worktree] [--skip-copy] [--discard-uncopied] [--keep-pane] [--reopen]` | Resolve after the final copy: close the pane and tab through Herdr (`--keep-pane` leaves them), and optionally remove the worktree (the branch is kept). |
| `overview [<project>] [--wait]`, `focus [<project>]`, `unfocus` | Threads grouped by what needs you, as text and in the sidebar. |
| `plan show [--json]`, `plan set`, `plan step add\|edit\|link\|unlink\|remove\|move`, `plan sync` | The plan card: goal, end result and up to seven steps. A step is `done` only when all its bound work has landed in a merged round. |
| `decide "<line>" --class <what-you-get\|money\|undo\|routine>`, `decide list [--json]`, `decide show <id>` | The log of choices the coordinator made without asking. |
| `say --what S [--means S] [--landed-round R]` | One checked line on the board and in talk. `--landed-round` marks it as landing evidence for a merged round. |
| `routine list`, `routine approve`, `safety show` | Routines and safety settings. |
| `pause`, `resume`, `archive`, `unarchive`, `delete [--force]` | Project lifecycle. `delete` moves the folder to `.trash/`. |
| `ticker start \| run \| stop \| status`, `doctor`, `skill` | Housekeeping. |

Groups, first match wins: Resolved; Working while starting; **Waiting on you** (failed, a launch stuck for 60 seconds, a pane gone with no report, or blocked for 30 seconds); **Working**; **Landing** (pull request open and approved); **Ready for review** (a report exists and either its pull request is open or you haven't acknowledged it); Idle. Threads idle for `auto_resolve_days` are resolved after a final copy home.

`focus` replaces any sidebar view another tool has set, and `unfocus` clears whatever view is set, because Herdr holds a single one. `focus` covers local threads only.

## Plans and choices

The project screen reads two records the coordinator keeps. They are ordinary files in the project folder; reading them never creates a plan, makes a decision, resolves a lane or answers a question.

- **The plan card** is `<project>/plan.toml`, written under `<project>/.plan.lock` with a revision guard and an atomic rename. It holds the goal copied exactly from `PROJECT.md`, one of seven end-result kinds (`screen`, `command`, `background`, `document`, `picture`, `number`, `finding`) with its fixed sentence, and up to seven ordered steps. A step's `state` is a projection, never a status the coordinator can set: it is `done` only when every bound thread's carrying round has merged with that work included and every bound round has merged, `running` when some required work has started or landed, and `left` otherwise. The shared refresh runs after a thread or round membership change, at each round merge and at each checkpoint, and derives the states from the durable records. `plan sync` is the manual form.
- **The decision log** is `<project>/decisions.jsonl`, appended under `<project>/.decisions.lock`; it is history, not the conversation journal. Each line is one checked plain sentence with class `what-you-get`, `money`, `undo` or `routine`, and optional retry `key`, `basis`, `replaces` and `request` links. A `replaces` record preserves the original instead of editing it; the old choice stops being current only when the replacement is valid. A `--key` retry with the same payload returns the existing record, and the same key with different content fails.

**Authority boundary.** `what-you-get`, `money` and `undo` are consequential: the coordinator asks before acting, and records one only with `--basis request:<id>` for an existing human message or `--basis ask:<id>@<revision>` for a current, nonzero answered ask. The plugin checks the reference exists and has that provenance; it cannot check that the permission really covers the choice, so the coordinator must. A `no` answer authorizes nothing. `routine` needs no basis. This is not an approval bypass.

**The three-ask cap.** At most three open asks may exist at once, counting a recorded ask whose publication is still pending. Creation, re-asking and answering serialize through `<project>/asks/.open.lock`, so concurrent writers cannot each claim the last slot; a fourth creation fails and records nothing. The newest open ask is re-asked as one merged question: its identifier stays and its revision advances. A project already above the cap can create nothing new until the count is within bounds; its existing asks stay visible.

**Recovery.** A plan or decision write is atomic, so a reader sees an old or a new complete record. A stale `--expect` fails and changes nothing. A decision log whose last line is cut is not read as a decision and blocks further appends until the file is repaired. A failed plan refresh is reported on its own line and never rolls back a merge; the screen reads the authoritative records and shows that the plan needs to catch up until persistence catches up.

When Rolf asks in chat to change a recorded choice, the coordinator treats it like any other message, records the replacement with its `request` link, and says in plain words what will change. Message acceptance is not completion, and a replacement's wording must not claim finished work before it exists.

## Safety settings

Set per project in `~/.config/herdr-ade/config.toml`; `safety show <project>` prints the table header to use.

```toml
[safety."/Users/you/.herdr-ade/billing"]
start_threads = "propose"          # or "auto": the coordinator starts threads without asking
routine_commands = false           # true lets approved routines run shell commands

[roles.lane]
kind = "claude"
args = ["--dangerously-skip-permissions"]
ready_timeout_ms = 20000
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

- **They are soft.** Agents have a shell. The guards are the skill text, your agent's permission prompts, keeping `config.toml` and approvals outside every agent's working directory, and `routine approve` refusing without a terminal and a typed confirmation. None of this stops an agent that runs with skip-permission arguments from editing those files directly.
- **A thread can impersonate you.** Any thread agent can prompt the coordinator's pane through Herdr, and that message carries no ticker marker. The skill's rule that a go-ahead must name the threads lowers the risk; it does not remove it.
- **An approved routine command covers the command text only.** `./check.sh` keeps its hash while the script changes.
- **Prompt injection is reduced, not removed.** The coordinator reads reports and may choose to fetch pull request comments itself. Memory is a carrier: whatever it writes there is inlined into every later brief.
- **Agent variety.** The skill and briefs are agent-neutral, but only Claude Code has been exercised.
- **Cost.** Every thread is a full agent session, and each nudge and each `context` spends coordinator tokens.

## Nudges and notifications

`nudge = false` is the default, because on Herdr 0.9.1 a prompt that arrives while you are typing in the coordinator **is merged with, and submits, your half-typed text**. With it off, the ticker shows one Herdr notification per set of new inbox items ("3 new inbox items") and the coordinator picks them up at its next turn. Set `nudge = true` in `PROJECT.md` to have the ticker prompt the coordinator when it is idle; the message always begins `[herdr-ade ticker: automated, not the user, approves nothing]` and never carries outside text.

## Routines

A file `routines/<name>.md`: TOML front matter with `schedule` (`every <N>m|h|d` or `daily HH:MM`, local time), optional `command`, `enabled`; the body is the prompt the coordinator receives as an inbox item when it is due. A routine with a `command` runs (`sh -c`, in the project folder, 60 second timeout) only when `routine_commands = true` **and** you have run `herdr-ade routine approve <project> <name>` in a terminal; its output reaches the coordinator capped at 4,000 characters inside a fence labelled as untrusted. Edit the command and it stops until approved again.

## Threads on other machines

Save the machine with `herdr machine add --label <label> <ssh target>` (both machines need Herdr 0.9.1), then list a repo as `--repo /path/on/machine@<label>` or pass `thread start --machine <label>`. The home machine owns the project; only outbound SSH from home is needed, in batch mode, so set up key-based login first.

- The worktree, the brief and the report live on the remote machine. The home ticker polls it once a minute and copies a changed report with `scp` and the thread's `library/` with `rsync -rt` (symbolic links are never followed or copied; a library over 50 MB is not copied and the inbox item says so).
- The box holds two plugin binaries: `/home/ubuntu/.local/bin/herdr-ade` (lane start and `ha`) and `/home/ubuntu/.local/bin/herdr-pi` (pi `setup`, `login`, `doctor` and `check`); no pi verb runs through `herdr-ade`.
- Every command the home machine runs on a box goes over SSH with the fixed box `PATH` `/home/ubuntu/.local/bin:/home/ubuntu/.cargo/bin:/usr/local/bin:/usr/bin:/bin` in front, so a fresh box needs no login-shell `PATH` edits for the plugin.
- A machine that doesn't answer is left alone: no state is read, threads keep their last group, and it is skipped for about two minutes. After ten minutes you get one `outage` inbox item, and one more when it is back.
- A blocked remote thread needs you in its pane on that machine: select the machine in Herdr's sidebar, or run `herdr --remote <ssh target>`.
- `focus` does not cover remote threads: their sidebar tokens are set on the remote Herdr server. They appear in `overview`, `thread list` and inbox items.
- Tasks with no repository always run locally, as tabs.

## Laptop-closed operation

No plugin code is involved: install Herdr and this plugin on an always-on machine, keep the projects root there, open the project there, and attach from your laptop with `herdr --remote <ssh target>` (add `--session <name>` for a named session). The ticker runs on that machine. If Herdr asks whether to restart a remote server "that may not survive SSH connection loss", answering `n` keeps its panes. Checked on a Linux (aarch64) machine from a Mac.

## Development

```bash
cargo test                       # unit tests and scenarios against a scripted fake runner
scripts/dev-server               # a throwaway `hp-dev` Herdr session with a scratch root
scripts/dev-hp <subcommand>      # the binary against <repo>/.dev-root; pass --session hp-dev to open/doctor
scripts/dev-herdr <args>         # herdr against that session
```

Never develop against your default session or `~/.herdr-ade`. Use `HERDR_ADE_ROOT` and `XDG_CONFIG_HOME` under `/var/tmp`. [`herdr-notes.md`](herdr-notes.md) records what was verified about Herdr stage by stage, and [`manual-test.md`](manual-test.md) lists the acceptance checks, including the visual ones only a person can confirm. [`going-public.md`](going-public.md) is the checklist for the public release.

`scripts/migration/swap-binary.sh` is the state-dependent install of `~/.local/bin/herdr`. Tests must set `HERDR_ADE_SWAP_DIR` so they never write `~/.local/bin`. Run `scripts/migration/swap-binary.test.sh` on this Mac.
