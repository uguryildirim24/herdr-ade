# Getting started: open your first project

Install Herdr ADE, configure task routing, then open a project for its coordinator.

## 1. Prerequisites

- macOS or Linux with [Herdr](https://herdr.dev) 0.9.1 or newer. Check both the client and running server with `herdr status`; restart a stale server after an update.
- Rust 1.89 or newer, Cargo, a C compiler, and Git.
- Access to `uguryildirim24/herdr-ade` and an agent CLI used by one of your configured recipes.
- Optional: `gh` for pull-request follow-up, and SSH plus `rsync` for lanes on another machine.

## 2. Install the plugin

```bash
herdr plugin install uguryildirim24/herdr-ade
```

Herdr clones the repository, runs its locked release build, and registers the plugin's actions, panes, startup command, and pi helper.

For terminal use, link the built binary shown by `herdr plugin list`:

```bash
ln -s <plugin-root>/target/release/herdr-ade ~/.local/bin/herdr-ade
mkdir -p ~/.config/herdr-ade
$EDITOR ~/.config/herdr-ade/config.toml
herdr-ade doctor
```

Add the starting routing table shown in [Task-based routing](operations.md#task-based-routing) to `config.toml`. It defaults to `pi_codex_sol_high`, routes coordinator, spec, web-research and Claude-required work with ordered rules, and allows one retry with no fallback. Executable recipes and machine placement live in the same file. `herdr-ade context <project>` lists every recipe, what it is for, its capabilities and the exact command or rule that reaches it.

If you use pi recipes, run `herdr-pi setup`, complete each required login with `herdr-pi login`, then run `herdr-pi doctor`. A login is local to that machine; do not copy its credential store to another machine.

## 3. Create and open a project

Use **Projects: new project** in Herdr, or run:

```bash
herdr-ade new "Billing" --goal "Ship the new billing page" --repo ~/dev/app
herdr-ade open billing
```

The default project root is `~/.herdr-ade`. `new` creates the project record; `open` creates or focuses its Herdr workspace and coordinator. The coordinator reads its skill and current digest before answering.

Edit only the front matter of the new `PROJECT.md` for repositories and project settings. Its body is the binary-written current page. Add facts and standing instructions with `herdr-ade note add <project> "<text>" --kind memory|instruction --request <request-id> [--task <job>]` so each fact has one provenanced home and can be explicitly replaced. Recipe selection lives in the editable `[routing]` table in `config.toml`.

## 4. Start work

Tell the coordinator what you want. In the default `auto` mode it starts the lanes the work needs and tells you what it started. Set a project's `start_threads` safety row to `propose` if you want it to describe lanes and wait for your go-ahead. Each repository lane gets:

- a committed task file under `tasks/`;
- a branch and worktree under `<repo>/.worktrees/`;
- a tab under the coordinator workspace; and
- the project's instructions and bounded memory.

A lane reports through its file under `.herdr-project/`. The harness copies reports to the project record and groups each lane by what needs attention. When a round merges or is cancelled, the harness closes every member lane and reviewer. A report-only lane closes as soon as its unchanged commit and report arrive. A changed lane stays visible until you put it in a round. `thread resolve` remains available for an exceptional manual close.

A finished lane's worktree is removed automatically once its commits have landed or its round has closed, but only when it has no uncommitted changes or ignored data. Uncommitted changes refuse resolution. Ignored data resolves the lane but keeps the worktree with a reason naming its folders and sizes. List rebuildable ignored paths such as `target` and `node_modules` under global `[worktrees].disposable` in `config.toml`, or add `disposable` to one repository row in `PROJECT.md` (and to a harness repository row for harness-only output). Repository lists affect only that repository. `*` matches within one path part, so `runs/pytest-*` leaves other `runs/` output alone; with no matching list, all ignored files are kept. Nested worktrees are always kept. The branch is retained and removal is never forced.

## Check your setup

```bash
herdr-ade doctor
herdr-ade ticker status
```

- **The project session is unreachable:** run `open` inside Herdr or pass the right `--session` or `--socket`. Use `--rebind` only when the recorded session is gone.
- **A lane failed, blocked, or got stuck:** inspect `thread show`, then use `thread retry --reason "<why>"`. It replaces the process through bounded routing. Use `thread cancel` to stop it or `thread rebind` when its verified process is already live elsewhere.
- **A provider is not ready:** use the provider's login flow on the same machine and rerun `doctor`.
- **The coordinator needs its instructions again:** run `open <project> --reprime`.
- **A worktree cannot be removed:** the final copy may be incomplete, Git may consider the worktree dirty, or ignored data may be present. Changes refuse resolution. Ignored data still lets the lane resolve and `doctor` lists what was kept; only configured rebuildable paths are discarded. Removal is not forced.

See [Operations](operations.md) for records, routing, rounds, safety settings, remote lanes, and the complete command surface.
