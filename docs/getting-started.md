# Getting started: open your first project

Install Herdr ADE, configure task routing, then open a project for its coordinator.

## 1. Prerequisites

- macOS or Linux with [Herdr](https://herdr.dev) 0.9.1 or newer. Check both the client and running server with `herdr status`; restart a stale server after an update.
- Rust 1.89 or newer, Cargo, a C compiler, and Git.
- Access to `uguryildirim24/herdr-ade` and an agent CLI used by one of your configured recipes.
- A TypeSafe System One key in `TYPESAFE_API_KEY`. Task dispatch uses Jev to score the full brief. The Herdr server and background ticker must inherit this variable.
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
cp <plugin-root>/config/routing.json ~/.config/herdr-ade/routing.json
herdr-ade doctor
```

`routing.json` is required; there is no embedded fallback. It contains the scoring questions, model cards, cutoffs, and routes. Executable recipes and machine placement belong in `~/.config/herdr-ade/config.toml`.

If you use pi recipes, run `herdr-pi setup`, complete each required login with `herdr-pi login`, then run `herdr-pi doctor`. A login is local to that machine; do not copy its credential store to another machine.

## 3. Create and open a project

Use **Projects: new project** in Herdr, or run:

```bash
herdr-ade new "Billing" --goal "Ship the new billing page" --repo ~/dev/app
herdr-ade open billing
```

The default project root is `~/.herdr-ade`. `new` creates the project record; `open` creates or focuses its Herdr workspace and coordinator. The coordinator reads its skill and current digest before answering.

Edit the new `PROJECT.md` for standing instructions, repositories, and project settings. Model choice does not live there: Jev scores each complete task brief and `routing.json` selects an executable recipe.

## 4. Start work

Tell the coordinator what you want. In the default `propose` mode it describes the lanes it would start and waits for your go-ahead. Each repository lane gets:

- a committed task file under `tasks/`;
- a branch and worktree under `<repo>/.worktrees/`;
- a tab under the coordinator workspace; and
- the project's instructions and bounded memory.

A lane reports through its file under `.herdr-project/`. The harness copies reports to the project record and groups each lane by what needs attention. Resolve a finished lane to close its tab and release the agent process:

```bash
herdr-ade thread resolve billing t-0001
```

Use `--remove-worktree` only when you also want the copied worktree removed. The branch is retained, and removal is never forced.

## Check your setup

```bash
herdr-ade doctor
herdr-ade ticker status
```

- **The project session is unreachable:** run `open` inside Herdr or pass the right `--session` or `--socket`. Use `--rebind` only when the recorded session is gone.
- **A lane did not start:** inspect `thread show`, then use `thread restart` for a failed start or gone pane. Placement and provider readiness failures are reported before a lane opens.
- **A provider is not ready:** use the provider's login flow on the same machine and rerun `doctor`.
- **The coordinator needs its instructions again:** run `open <project> --reprime`.
- **A worktree cannot be removed:** the final copy may be incomplete or Git may consider the worktree dirty. The refusal names the safe next action; removal is not forced.

See [Operations](operations.md) for records, routing, rounds, safety settings, remote lanes, and the complete command surface.
