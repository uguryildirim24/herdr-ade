# Herdr ADE

One coordinator conversation drives parallel coding agents on your machines. Each lane has a task, a frozen brief and its own Git worktree. One reviewer checks the finished pile before it lands. The bundled Rundown tab shows the project's plan and what needs attention.

![Coordinator, parallel lanes and pile review](assets/herdr-ade-coordinator-threads.svg)

## Installation status

**The repository is still private.** Anonymous installation does not work today. Publishing this repository is the release prerequisite; there is no public bundle workaround. With repository access, follow the single [setup walkthrough](docs/getting-started.md).

The default setup needs Herdr, Rust/Cargo, Git, Node/npm, ADE's pinned Pi setup and provider logins, and the Claude CLI with its own login for the coordinator. ADE is MIT licensed; agents' service charges still apply.

## Start work

After setup:

```bash
herdr-ade new "Billing" --repo ~/dev/app
herdr-ade open billing
```

Tell the coordinator your goal in its pane. That conversation records the request that authorizes tasks; a direct `thread start` also needs a task backed by an existing recorded request. Creating a project without agent logins is not enough to dispatch work. The coordinator starts the lanes it needs autonomously, and you answer decisions in chat.

## Check your setup

```bash
herdr-ade doctor
herdr-ade ticker status
```

Use the terminal doctor for the actual checks and exit status. Herdr's doctor action runs asynchronously: its invoke reply only acknowledges launch; the checks and final result appear in the plugin log and notification. If the ticker is absent, run `herdr-ade ticker start`, then check status again. See [setup troubleshooting](docs/getting-started.md#check-your-setup).

## When not to use it

Do not use ADE for untrusted repositories, on a machine whose files or credentials agents must not reach, or when you need approval of every shell command. It needs an available machine and signed-in agents; it is not a hosted service that runs with all your machines off.

## Trust boundary

**ADE is not a sandbox.** Shipped recipes bypass agent permission prompts, and ADE requires their declared bypass flags (including Claude's `--dangerously-skip-permissions`). An allow-list is not a destructive-command safety boundary here. Agents run with your account's filesystem, network and credential access; their selected providers receive the context those agents send.

Worktree isolation is not security isolation. ADE verifies deletion scope and Git publication, does not copy provider credentials between machines, and keeps seal, merge, push, install and delivery as separate facts. Missing evidence is not promoted to success. See the canonical [safety and cleanup reference](docs/operations.md#safety-and-cleanup).

## Reference

- [Setup](docs/getting-started.md): prerequisites, installation, first project and troubleshooting.
- [Operations and development](docs/operations.md): records, routing, review, machines, safety and gates.
- `herdr-ade --help` and each subcommand's `--help`: current command syntax.

Projects live under `~/.herdr-ade/` by default. `PROJECT.md` is hand-edited settings; `.state/page.md` is the generated view. Historical records remain readable. No remote machine is enabled by default.
