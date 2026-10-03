# Getting started

This is the one setup path for macOS and Linux. **The repository is private:** you need GitHub access to `uguryildirim24/herdr-ade`. Anonymous clone and plugin install currently stop at GitHub authentication. Publication is an external release prerequisite, not an installer feature.

## 1. Install prerequisites in order

1. Install [Herdr](https://herdr.dev) 0.9.1 or newer using its supported instructions. Run `herdr status` to check the client and running server. Apply server changes through Herdr's live handoff, not by stopping your running sessions.
2. Install [Rust/Cargo](https://rustup.rs/) **1.89 or newer**, a C compiler (Xcode Command Line Tools on macOS, your distribution's compiler tools on Linux), and [Git](https://git-scm.com/downloads).
3. Install [Node.js/npm](https://nodejs.org/en/download) (Node 24 is used by development CI). Confirm `node --version` and `npm --version` work in the shell Herdr starts.
4. Install ADE and its pinned Pi runtime as described below. Pi lanes need their own provider logins. [Pi's instructions](https://github.com/earendil-works/pi/tree/main/packages/coding-agent#authentication) describe authentication; use ADE's `herdr-pi setup`, not a separate global Pi installation.
5. Install the [Claude CLI](https://code.claude.com/docs/en/setup) and complete its supported login flow on this machine. Confirm `claude --version` works. The starting coordinator recipe uses Claude, even though the default work lane uses Pi.

The full starting routing example also selects the Antigravity (`agy`) adapter for web research. Configure only recipes you have installed and signed into; disable unused recipes and remove their routing rules rather than expecting a missing CLI to work. The recipe catalogue is shown by `herdr-ade context <project>`.

For deletion, each affected machine needs `/usr/bin/trash` on macOS, or `gio trash` or `trash-put` on Linux. ADE checks these before effects and never falls back to permanent deletion. `gh` is optional for GitHub operations; remote lanes additionally need SSH and `rsync`.

## 2. Install ADE and set up Pi

With repository access:

```bash
herdr plugin install uguryildirim24/herdr-ade
```

Herdr clones and runs the locked release build, then registers ADE's actions, bundled Rundown pane and ticker startup. For an unattended install, Herdr's existing `--yes` option accepts its install confirmation; it does not grant GitHub access.

Find the plugin directory with `herdr plugin list`, substitute its actual path below, and make sure `~/.local/bin` is on your shell's `PATH`:

```bash
PLUGIN_ROOT=/path/to/herdr-ade
mkdir -p ~/.local/bin ~/.config/herdr-ade
ln -s "$PLUGIN_ROOT/target/release/herdr-ade" ~/.local/bin/herdr-ade
ln -s "$PLUGIN_ROOT/target/release/herdr-pi" ~/.local/bin/herdr-pi
export PATH="$HOME/.local/bin:$PATH"
```

Persist the PATH change in your shell configuration. No `ha` alias is required. Generated command prefixes include the resolved binary and project root.

Open `~/.config/herdr-ade/config.toml` with your editor (for example `nano` if `$EDITOR` is unset). Add the [starting routing table](operations.md#task-based-routing). Recipes, routing and machine placement share that file; the built-in recipes supply executable arguments. The example routes coordinator work to Claude and ordinary lanes to Pi.

```bash
herdr-pi setup
```

Setup installs ADE's pinned npm package and integration, then prints the wrapper link command. **Run that printed `ln -s … ~/.local/bin/pi` command** so Herdr can launch `pi`. Then use:

```bash
herdr-pi login
herdr-pi doctor
```

Follow the login instructions for each provider used by your enabled recipes. Logins belong to each machine: never copy credentials to a box. Complete the Claude installation/login from step 1 before opening the coordinator. See [the trust boundary](../README.md#trust-boundary): shipped recipes bypass permissions; ADE refuses recipes missing their required bypass flags.

## 3. Create a project, plan and coordinator

From inside Herdr, use **Projects: new project**, or run:

```bash
herdr-ade new "Billing" --repo ~/dev/app
herdr-ade plan set billing --does "A working billing page"
herdr-ade plan step add billing "Build the page"
herdr-ade plan step add billing "Review and ship"
herdr-ade open billing
```

The repository must already exist. `new` creates records under `~/.herdr-ade`; `open` creates or focuses the project workspace and coordinator. If using a named session, pass `--session <name>` to `open` and `doctor` consistently. A shell outside Herdr needs an explicit session or socket when discovery cannot select one.

**Rundown is included, not a separate plugin.** Opening the project ensures a project-bound Rundown tab; the ticker also maintains it for a recorded workspace. Do not launch a raw `herdr-rundown` pane: it needs the project environment supplied by ADE. Steps gain their done state from linked stable tasks, not from reading the plan.

Herdr 0.9.1 was verified for project, plan and Rundown setup. Public Herdr without `agent start --parent` reports a compatibility warning and uses post-start parenting; sidebar nesting itself was not verified in the fresh-install run. ADE's parenting path requires `agent_parent_notify = false` in Herdr's `[experimental]` settings so Herdr does not bypass ADE's durable notices.

Edit `PROJECT.md` front matter for repositories and project settings. ADE never replaces it; `.state/page.md` is the generated view. See [Operations](operations.md#where-things-live).

## 4. Start work

Tell the signed-in coordinator your goal in its pane. The prompt-submit hook records a request id, and the coordinator uses it to create stable tasks and dispatch lanes. Every lane gets a pinned brief, its own branch/worktree and the applicable instructions.

Direct CLI dispatch does not bypass request authority. After a real coordinator conversation, find its recorded request in `herdr-ade context billing`, then use that actual id:

```bash
herdr-ade task add billing --title "Build billing page" --request <recorded-request-id> --acceptance "The billing page works" --repo ~/dev/app
herdr-ade thread start billing --job <returned-job-id> --task-file /path/to/brief.md
```

Without a signed-in coordinator and a captured request, you can create records and view the plan/Rundown, but cannot authorize tasks or lanes. Do not synthesize request state. Finished changed lanes go through one pile review; reports and cleanup follow the [operations reference](operations.md#safety-and-cleanup).

## Check your setup

```bash
herdr-ade doctor
herdr-ade ticker start
herdr-ade ticker status
```

Doctor prints the checks and exits nonzero for failures. Startup confirms a running ticker lock, even before the first project; it reports child initialization failures instead of claiming success. The ticker stays available when sessions are temporarily unreachable.

- **Missing CLI:** install the executable named in the pane's `command not found` error and check its PATH on that machine. `open` surfaces a visible shell launch failure instead of waiting out the readiness window. Fix it, then rerun `open`.
- **Missing login:** use that CLI/provider's supported login on the same machine, then rerun `doctor`. A provider readiness failure is not fixed by creating another project.
- **Missing/unreachable session:** run `open` in the right Herdr session or pass `--session`/`--socket`. `--rebind` is only for a recorded session whose socket is gone; it is not login recovery.
- **Ticker absent:** run `ticker start`, then `ticker status`. Check `.ticker.log` under the projects root for a reported startup error. Merely invoking a plugin action is not evidence that a process stayed running.
- **Lane failed or stuck:** inspect `thread show`, then `thread retry --reason "<why>"`, `thread cancel`, or `thread rebind` for an already-live verified process. These are not provider-login commands.
- **Coordinator needs instructions again:** `open <project> --reprime`.

See [Operations](operations.md) for safety, records, reviews, remote lanes and development gates.
