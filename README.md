# Herdr ADE

One coordinator conversation drives parallel coding agents on your machines. Each lane has a task, a frozen brief and its own Git worktree. One reviewer checks the finished pile before it lands. The bundled Rundown tab shows the project's plan and what needs attention.

![Coordinator, parallel lanes and pile review](assets/herdr-ade-coordinator-threads.svg)

## Installation status

**The repository is still private.** Anonymous installation does not work today. Publishing this repository is the release prerequisite; there is no public bundle workaround. With repository access, follow the single [setup walkthrough](docs/getting-started.md).

The walkthrough needs Herdr, Rust/Cargo, Git, Node/npm and ADE's pinned Pi runtime with one machine-local provider login. It uses Pi for coordinator, lanes and reviewer; Claude and agy are optional recipe choices with separate logins. ADE is MIT licensed; agents' service charges still apply.

## Start work

Follow [Getting started](docs/getting-started.md) through its first accepted artifact: one-time setup, project creation, review enablement, then one paragraph in the coordinator pane. The coordinator records your request, develops the goal into tasks, dispatches lanes and sends changed work through one pile reviewer to the integration branch.

That prompt authorizes ordinary reversible work within the goal, not unrelated spending or deployment. Review enablement is an explicit command in setup, not an unmentioned second approval after work finishes. Missing login or consequential authority is a real stop; lane completion alone is not acceptance.

## Check your setup

```bash
herdr-ade doctor
herdr-ade ticker status
```

Use the terminal doctor for the actual checks and exit status. Herdr's doctor action runs asynchronously: its invoke reply only acknowledges launch; the checks and final result appear in the plugin log and notification. If the ticker is absent, run `herdr-ade ticker start`, then check status again. See [setup troubleshooting](docs/getting-started.md#check-your-setup).

## When not to use it

Do not use ADE for untrusted repositories, on a machine whose files or credentials agents must not reach, or when you need approval of every shell command. It needs an available machine and signed-in agents; it is not a hosted service that runs with all your machines off.

## Trust boundary

**ADE is not a sandbox.** The walkthrough's Pi agents execute tools without per-command approval. Claude and agy recipes require `--dangerously-skip-permissions`; Cursor requires `--force`. An allow-list is not a destructive-command safety boundary here. Agents run with your account's filesystem, network and credential access; their selected providers receive the context those agents send.

Worktree isolation is not security isolation. ADE verifies deletion scope and Git publication, does not copy provider credentials between machines, and keeps seal, merge, push, install and delivery as separate facts. Missing evidence is not promoted to success. See the canonical [safety and cleanup reference](docs/operations.md#safety-and-cleanup).

## Reference

- [Setup](docs/getting-started.md): prerequisites, installation, first project and troubleshooting.
- [Operations and development](docs/operations.md): records, routing, review, machines, safety and gates.
- `herdr-ade --help` and each subcommand's `--help`: current command syntax.

Projects live under `~/.herdr-ade/` by default. `PROJECT.md` is hand-edited settings; `.state/page.md` is the generated view. Historical records remain readable. No remote machine is enabled by default.
