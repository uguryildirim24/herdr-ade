# Getting started

This is the setup path for macOS and Linux. Start from a source clone of `uguryildirim24/herdr-ade`. Repository access is required until publication. Anonymous clone and GitHub plugin installation were not verified during cleanup.

## 1. Install prerequisites in order

1. Install [Herdr](https://herdr.dev) 0.9.1 or newer using its supported instructions. Run `herdr status` to check the client and running server. Apply server changes through Herdr's live handoff, not by stopping your running sessions.
2. Install [Rust/Cargo](https://rustup.rs/) **1.89 or newer**, a C compiler (Xcode Command Line Tools on macOS, your distribution's compiler tools on Linux), and [Git](https://git-scm.com/downloads).
3. Install [Node.js/npm](https://nodejs.org/en/download), **Node 22.19.0 or newer** (Node 24 is used by development CI). Confirm `node --version` and `npm --version` work in the shell Herdr starts.
4. Install ADE and its pinned Pi **0.99.1** runtime below, then log into `openai-codex` on this machine. This walkthrough uses Pi for coordinator, coding lanes and reviewer, with that single provider login. Use ADE's `herdr-pi setup`, not a separate global Pi installation.

Claude and Antigravity (`agy`) are optional, not prerequisites for this path. Selecting them later requires their CLI and machine-local login; see [routing](operations.md#task-based-routing). No remote machine is needed.

For deletion, each affected machine needs `/usr/bin/trash` on macOS, or `gio trash` or `trash-put` on Linux. ADE checks these before effects and never falls back to permanent deletion. `gh` is optional for GitHub operations; remote lanes additionally need SSH and `rsync`.

## 2. Install ADE and set up Pi

From the source checkout, build and link the plugin. All three links are required by the manifest, including Rundown:

```bash
cargo build --release --locked
PLUGIN_ROOT="$(pwd -P)"
mkdir -p "$HOME/.local/bin" "$HOME/.config/herdr-ade"
ln -s "$PLUGIN_ROOT/target/release/herdr-ade" "$HOME/.local/bin/herdr-ade"
ln -s "$PLUGIN_ROOT/target/release/herdr-pi" "$HOME/.local/bin/herdr-pi"
ln -s "$PLUGIN_ROOT/target/release/herdr-rundown" "$HOME/.local/bin/herdr-rundown"
export PATH="$HOME/.local/bin:$PATH"
herdr plugin link "$PLUGIN_ROOT"
```

These commands assume a fresh link destination. Inspect an existing executable or link before replacing it. `herdr plugin link` registers the actions, Rundown pane and ticker startup, but does not build the executables. Persist PATH in the shell configuration. No `ha` alias is required for this Pi walkthrough. Generated command prefixes include the resolved binary and project root. The optional Claude handoff mod has separate [executable and default-root requirements](operations.md#threads-on-other-machines).

The hosted alternative is `herdr plugin install uguryildirim24/herdr-ade`. It requires repository access and was not exercised during cleanup. Its checkout still needs all three executable links above; use the actual plugin path reported by `herdr plugin list` for `PLUGIN_ROOT`.

### Pi configuration

Open `~/.config/herdr-ade/config.toml` with your editor (for example `nano` if `$EDITOR` is unset). For a fresh setup, use this local, single-provider configuration. The default covers coordinator and lanes; the separate reviewer uses xhigh. Disabled rows replace shipped rows completely, so their required arguments remain present. On an existing setup, edit the corresponding tables instead of duplicating them; leave `[dispatch] machine` unset for this local journey.

```toml
[routing]
default = "pi_codex_sol_high"
retries = 1

[[routing.rules]]
workflow = "reviewer"
recipe = "pi_first_review"

[recipes.pi_first_review]
kind = "pi"
provider = "openai-codex"
args = ["--provider", "openai-codex", "--model", "gpt-5.6-sol", "--thinking", "xhigh", "--no-skills"]
ready_timeout_ms = 300000

[recipes]
agy_gemini_flash = { enabled = false, kind = "agy", args = ["--dangerously-skip-permissions"] }
claude_fable_xhigh = { enabled = false, kind = "claude", args = ["--dangerously-skip-permissions", "--disallowedTools", "Agent"] }
claude_coordinator_opus = { enabled = false, kind = "claude", args = ["--dangerously-skip-permissions", "--disallowedTools", "Agent"] }
pi_opencode_deepseek = { enabled = false, kind = "pi", provider = "opencode-go", args = ["--provider", "opencode-go", "--model", "deepseek-v4.1-flash", "--thinking", "high", "--no-skills"] }
pi_opencode_muse = { enabled = false, kind = "pi", provider = "opencode-go", args = ["--provider", "opencode-go", "--model", "muse-spark-1.3-contributor", "--thinking", "high", "--no-skills"] }
```

```bash
herdr-pi setup
```

Setup installs ADE's pinned npm package and integration, then prints the wrapper link command. **Run the printed wrapper link command for `~/.local/bin/pi`** so Herdr can launch `pi`. Then use:

```bash
herdr-pi login
herdr-pi doctor
```

Choose `openai-codex` in the login flow; both enabled recipes use it. [Pi's authentication instructions](https://github.com/earendil-works/pi/tree/main/packages/coding-agent#authentication) describe the browser flow. Logins belong to each machine: never copy credentials to a box. Pi tools here run without per-command approval; see [the trust boundary](../README.md#trust-boundary) before opening a trusted repository.

## 3. Create the project and enable review once

Use a trusted existing repository at `~/dev/app` with a committed HEAD and a clean integration checkout (substitute your path throughout). From a shell inside Herdr:

```bash
herdr-ade new "Billing" --repo ~/dev/app
```

Before enabling review, open `~/.herdr-ade/billing/PROJECT.md` and inspect every generated `[[repos]]` front-matter table. `new` records the checked-out branch as the integration branch and automatically sets `push_remote` when a local repository has exactly one remote. A typical clone therefore gets `push_remote = "origin"` without a separate publication opt-in. Multiple remotes require an explicit choice; no remotes means no inferred destination.

For this local-only journey, remove both `push_remote` and `publish_url` if either is present, including any automatically inferred value. Do this before running `review` below. With neither destination configured, landing stays local and reports no remote configured. Keep or add a destination only when Rolf has authorized publication there and the machine's Git credentials work. Enabling review can otherwise push accepted work automatically, without another approval.

In that repository table, add `gates = [{ command = "git diff --check" }]`. That is this documentation result's mechanical gate; the reviewer must also check the content. For application work, declare the repository's real test/build gates there. Change `branch` only if a different integration branch is intended.

```bash
herdr-ade review billing --repo ~/dev/app
herdr-ade open billing
```

`review` enables automatic pile reviews project-wide even when the pile is empty. It is the existing opt-in, not a second approval needed later. `open` creates or focuses the coordinator and bundled Rundown tab. No hand-written plan or task brief is needed. For a named Herdr session, pass `--session <name>` to `open` and `doctor` consistently; outside Herdr, pass a session or socket if discovery cannot select one.

The full authenticated journey and fresh-install sidebar nesting were not verified during cleanup. Without `agent start --parent`, ADE warns and uses post-start parenting. Set `agent_parent_notify = false` in Herdr's `[experimental]` settings so native notices do not bypass ADE's durable notices. Do not launch a raw `herdr-rundown` pane; ADE supplies its project environment.

## 4. Give one goal; come back to the result

Type this single paragraph in the coordinator pane:

> Make this repository easier for a new contributor to run. Inspect its existing entry points and instructions, then add `docs/first-run.md` with the shortest working local setup and run commands. Verify those commands where this machine can run them, and name any missing credentials or services instead of claiming success. Keep this documentation-only; do not deploy, buy anything or change credentials. Plan and dispatch the work, use the enabled pile review, land it on the integration branch and summarize the artifact, verification and any remaining limits. Stop once this first-run guide is usable.

The prompt hook records your request; the coordinator turns it into an outcome, request-backed acceptance and linked plan/tasks, then starts lanes. It can make ordinary reversible choices within that goal without another go-ahead. Spending, irreversible effects or consequential missing intent are not silently authorized. Direct CLI dispatch still needs a real recorded request; `new`, automated notices and reports do not create authority.

The lane commits only if it changed repository files, writes its recorded report and seals with `ha done` (the generated prefix does not require an alias). No empty commit or guessed report/SHA arguments are needed. A changed seal joins the pile, not the integration branch. When this repository's lanes stop working, one reviewer checks the combined artifact and gates; MERGE lands it, pushes only a configured destination, then cleans up. Rejected work needs correction and a fresh seal. No-change reports finish without pile review; that alone does not establish that the requested guide works.

Come back to `docs/first-run.md` on the integration branch and the coordinator's summary of command evidence and limits. Rundown projects task progress; a done count is not proof of usability. Goal articulation and semantic acceptance still depend on coordinator/reviewer judgment, not an enforced acceptance state. Inspect evidence with `herdr-ade context billing --peek --full`, `herdr-ade task list billing` and `herdr-ade task show billing <listed-job-id>`. REVIEW separates merge, publication and install; this ordinary repository needs no harness install.

A browser login, unavailable service or authority outside your prompt is a real boundary: the coordinator must name the affected command/machine and missing input, not claim verified completion. Never manufacture request records or treat a green whitespace gate as proof that setup works.

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
- **Lane failed or stuck:** the coordinator uses the notice's action row. Automatic recovery retries the same recipe once here and preserves the worktree; exhaustion or unknown evidence needs diagnosis, not model switching. A stall notice is not proof of death. Corrections to an open parked lane use `thread prompt` and a fresh seal; the harness reopens it. See [recovery](operations.md#inbox-notifications).
- **Coordinator pane missing or deliberately closed:** it is not blindly recreated. Run `herdr-ade open billing` to resume; `open billing --reprime` is for a live coordinator needing its instructions again.
- **Optional Claude trust dialog:** ADE answers old/new trust dialogs only for a verified local managed worktree. The new dialog uses Down, verifies the highlighted Yes, then Enter. Coordinator/project folders, remote or unverified folders still need your trust decision in that pane; permission bypass is not folder trust.

See [Operations](operations.md) for safety, records, reviews, remote lanes and development gates.
