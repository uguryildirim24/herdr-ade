# Herdr ADE

Herdr ADE is a Rust plugin for Herdr that coordinates parallel coding agents in Git worktrees and reviews their combined changes.

## Why it exists

Parallel agents need separate checkouts, a shared task record and a clear path back to the integration branch. ADE provides that workflow. A coordinator records requests and assigns lanes. Each lane receives a frozen brief. One pile reviewer checks the combined changes before landing. The bundled Rundown pane shows the plan and work needing attention.

![Coordinator, parallel lanes and pile review](assets/herdr-ade-coordinator-threads.svg)

This diagram describes the workflow, not measured performance. The repository contains implementation, fixtures and reproduction tools, not a benchmark dataset or research results.

## Build from a clean clone

Requirements: macOS or Linux, Git, Rust/Cargo **1.89 or newer**, and a C compiler (Xcode Command Line Tools on macOS, distribution compiler tools on Linux). This source build does not need Herdr or a provider login.

Once repository access is available:

```bash
git clone https://github.com/uguryildirim24/herdr-ade.git
cd herdr-ade
cargo build --release --locked
./target/release/herdr-ade --help
./target/release/herdr-pi --help
./target/release/herdr-rundown --version
```

The three executables are under `target/release/`. These commands inspect the CLI without starting agents. Anonymous clone and GitHub plugin installation depend on publication and were not checked during this cleanup.

## Run a project

Use a trusted repository with a committed HEAD. Runtime setup requires **Herdr 0.9.1 or newer**, **Node 22.19.0 or newer**, npm and a machine-local provider login. ADE installs exactly **`@earendil-works/pi-coding-agent@0.99.1`** through `herdr-pi setup`; a global Pi installation is not the documented runtime.

From the source checkout, register the plugin and link all three executables:

```bash
PLUGIN_ROOT="$(pwd -P)"
mkdir -p "$HOME/.local/bin" "$HOME/.config/herdr-ade"
ln -s "$PLUGIN_ROOT/target/release/herdr-ade" "$HOME/.local/bin/herdr-ade"
ln -s "$PLUGIN_ROOT/target/release/herdr-pi" "$HOME/.local/bin/herdr-pi"
ln -s "$PLUGIN_ROOT/target/release/herdr-rundown" "$HOME/.local/bin/herdr-rundown"
export PATH="$HOME/.local/bin:$PATH"
herdr plugin link "$PLUGIN_ROOT"
```

These link commands assume a fresh destination. If a name already exists, inspect it before replacing anything. Persist PATH in the shell configuration. Linking alone does not build ADE.

Continue with [Getting started, Pi configuration](docs/getting-started.md#pi-configuration). It supplies the local routing configuration, `herdr-pi setup`, wrapper link, login, project creation and review opt-in. Do not skip that configuration: the shipped recipes include optional providers with separate prerequisites.

Check the configured runtime with:

```bash
herdr-pi doctor
herdr-ade doctor
herdr-ade ticker status
```

A nonzero doctor result means setup is incomplete. If the ticker is absent, run `herdr-ade ticker start` and check status again. The plugin doctor action runs asynchronously; its launch acknowledgement is not the result of the checks.

## Files and data

| Path | Purpose |
| --- | --- |
| `src/` | Coordination, records, review, routing, machines and ticker |
| `src/bin/` | Pinned Pi wrapper and Rundown executable |
| `skill/` | Coordinator, lane, reviewer and Pi instructions |
| `extensions/` | Pi guard extension |
| `mods/coordinator-handoff/` | Claude handoff hooks and existing tests |
| `assets/` | Default configuration, execution tools and workflow diagram |
| `tests/` | Existing CLI and integration fixtures |
| `docs/` | Setup and operations reference |
| `tools/wall/` | Host-specific Linux fault tools and active regressions |

No datasets or model weights are required. Cargo fetches locked dependencies during the build. `herdr-pi setup` fetches the pinned npm package into `~/.herdr-ade/pi/npm/` by default. Providers run through their own authenticated services and may charge for inference.

Projects and agent records live under `~/.herdr-ade/`. Recipes and machine paths live in `~/.config/herdr-ade/config.toml`. Set `HERDR_ADE_ROOT` and `XDG_CONFIG_HOME` to separate local directories when developing. `PLUGIN_ROOT` above is the absolute checkout path; `/path/to/herdr-ade` and `~/dev/app` in the guide are placeholders to replace locally. Keep logins, SSH keys, env files, session logs and exported evidence outside the source tree. Build output, caches and common local data/secret paths are ignored. Tracked fixtures exercise record and terminal formats; they are not an authenticated run. Inspect logs before sharing them.

## Trust boundary

**ADE as a whole is not a sandbox.** Coordinator and reviewer agents, Pi on macOS and advisory backends retain host access. New Pi lanes on Linux can use the bubblewrap tool boundary described in [Operations](docs/operations.md#safety-and-cleanup); worktrees alone are not security isolation.

Pi tools execute without per-command approval. Claude and agy recipes require `--dangerously-skip-permissions`; Cursor requires `--force`. Agents can reach the launching account's files, network and credentials where no tool boundary applies. Selected providers receive the context sent to them. Do not use ADE for untrusted repositories or when every shell command needs individual approval.

Deletion checks resource scope and requires a platform trash tool. Remote machines are not enabled by default. Seal, merge, push, install and delivery are separate recorded facts. Enabling review authorizes automatic pile review and can publish accepted work. `new` automatically records a local repository's sole remote as `push_remote`, including `origin` in a typical clone. For local-only landing, inspect the generated repository settings and remove both `push_remote` and `publish_url` before enabling review. Read the [setup guide](docs/getting-started.md) before starting agents.

## Verification and limits

Source builds, ADE/Pi help, Rundown version, isolated plugin registration, formatting, Clippy, existing test compilation and the Node guard suite are the local checks for this cleanup. The detailed commands and evidence limits are in [Operations](docs/operations.md#development). Test compilation is not a passing full Rust test run. The existing Linux wall controller suite has two host-specific failures on macOS, involving GNU `mv -T` and setgid directory permissions; no portability fallback was added.

Authenticated coordinator-to-lane-to-review operation, fresh sidebar nesting, remote operation and privileged Linux wall campaigns were not exercised during cleanup. Historical reports and reproducers remain in `report.md` and `tools/wall/findings/`. They describe the named builds, not current results. Their external evidence archives are not included in this checkout. Scripted wall agents are fault fixtures, not proof of real-model acceptance. A done count does not establish semantic correctness. The first accepted artifact still needs an isolated, authenticated walkthrough before publication.

Only the current source tree was scanned for publication safety. Git history and other branches need separate review. Runtime code matches HEAD. Only existing test fixtures have privacy redactions. The live ask workflow is retired, but historical ask records and answered-ask authority references remain readable. The existing macOS ticker service label is unchanged.

## How this was built

ADE builds on Elias Stravik's Herdr Projects (`eliasstravik/herdr-projects`), not a project written from scratch by Rolf. The initial commit and its README identify that source. [Provenance and contributions](docs/provenance.md) separates the upstream foundation from later ADE development, with local commit references.

AI coding agents did much of the later implementation under Rolf's direction. Rolf set the workflow requirements and directed this public-release cleanup, including scope review, privacy checks and honest setup documentation. Agents ran mechanical checks and reported what they could not verify. These checks are not evidence that Rolf personally wrote or checked each implementation. Rolf's final attribution and diff review, and a live authenticated run, remain release requirements. No independent code audit or research validation is claimed.

## License

ADE and its Herdr Projects foundation are MIT licensed. [LICENSE](LICENSE) preserves Elias Stravik's original copyright notice unchanged from the initial commit. Later ADE development does not replace that attribution. See [the source and contribution record](docs/provenance.md). Herdr is a separate prerequisite, not vendored here; this plugin does not replace Herdr's license.
