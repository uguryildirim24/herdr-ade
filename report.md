# Stranger setup on oci — t-0711

## Result

The public Herdr installer works on this box. ADE builds and its project, plan, and Rundown work without signing in. A stranger cannot yet complete the advertised setup:

1. **Publishing blocker #1: the GitHub repository is private.** Both anonymous clone and plugin install failed. The authorized bundle let this investigation continue.
2. The starting configuration needs more prerequisites than the setup page explains. Pi needed an unlisted Node/npm installation; the coordinator needed a separate Claude CLI.
3. `open` stayed silent through a 240-second observation window while its pane already said `claude: command not found`.
4. The one lane-start attempt stopped at missing request authority, **before** provider readiness. I did not fabricate request records or bypass that requirement.

A short follow-up installed Claude locally too. With the required Haiku probe model, `ha open` then succeeded in 4.260 seconds, but Claude was at first-run onboarding and `claude auth status` still said `loggedIn: false`. That is the final human-login boundary, not just a missing-executable stop. The follow-up also proved that ADE **requires** the permission-bypass flag in this recipe: removing it was refused.

**U4 is present:** New project asks for a repository, and the main plugin supplies a working Rundown showing the plan. This is not a request to rebuild either feature.

Cleanup is complete. No source code changed. This report is the only repository deliverable.

## Scope and evidence

- Date: 2026-10-03 UTC. Initial inspection: 05:30:17; first cleanup: 05:45:20. A focused Claude-prerequisite follow-up ran 05:50:54–05:54:05, ending with a second complete cleanup. About 24 minutes total elapsed, including gates and intervening report drafting.
- Machine: oci, Linux aarch64; fresh account `herdr-stranger`, UID/GID 1002, home `/home/herdr-stranger`.
- Runtime-tested commit: `649b5ae3fed451e8d02f11cd4efe5945955934c7`. The worktree HEAD, main, and bundle checkout matched in the first pass. Main advanced concurrently before the follow-up; that checkout was explicitly returned to this pinned commit and rebuilt before any follow-up ADE runtime command.
- Setup sources: the complete `README.md`, `docs/getting-started.md`, and `docs/operations.md`; herdr.dev and its linked agent guide, installation page, and CLI reference; command help and command output. No private project memory, other project content, or application source was used to find the setup path. Compiler diagnostics were read for the requested gates, after setup.
- The named public Herdr session was **`scratch-t-0711`**, entirely under the throwaway account. Public installation produced **Herdr 0.9.3**, not the real account's installed 0.9.1 fork. I did not replace or update the real installation.
- Every setup command ran through the clean-environment account wrapper below. No API keys, inherited agent environment, SSH agent, or real-account credential stores were passed in.
- Rustup and Node installation commands were prerequisite workarounds: ADE lists Rust without installation instructions and does not list Node/npm as setup prerequisites. They were user-local, not system installs.
- No provider login was attempted and no test model request was made. The probe brief explicitly required `--model claude-haiku-4-5-20251001` if a model process were started. The unavailable default Claude command never launched; the lane start failed before launching an agent. The follow-up's Haiku-configured Claude process reached only first-run onboarding, with no sign-in.

Common account wrapper (the setup commands below were its stdin):

```bash
sudo -iu herdr-stranger /usr/bin/env -i \
  HOME=/home/herdr-stranger USER=herdr-stranger LOGNAME=herdr-stranger \
  SHELL=/bin/bash \
  PATH=/home/herdr-stranger/.local/bin:/home/herdr-stranger/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
  TERM=xterm-256color /bin/bash
```

Each `run` printed its command, exit status, and wall duration using Bash `EPOCHREALTIME`. Output below is trimmed; shell quoting is restored where needed to preserve arguments. Read-only outer inspection commands were not individually timed, and are identified as such rather than assigned invented timings. The temporary transcript and downloads were deleted with the account.

## Ranked stumbles and deletion/rewrite fixes

| Rank | What stopped or misled the stranger | Evidence | Existing-work fix; mapping |
|---|---|---|---|
| 1 — publishing stop | No anonymous repository access | Clone exit 128; plugin install with `--yes` exit 1, both asking for GitHub credentials | **P3-S5:** rewrite the installation status honestly until publication; delete the implication that an arbitrary stranger can install it today. Publishing the existing repository is the external release prerequisite, not a new feature. Do not retain the bundle workaround as the public setup path. |
| 2 — setup stop | The starting routing example needs both Pi and Claude, not merely “an agent CLI”; Node/npm are missing from prerequisites | Pi setup: `could not run npm`; after fixing Pi, `open` launches unavailable `claude`. Doctor validates six recipes but does not identify this missing coordinator executable | **P3-S5:** rewrite one ordered prerequisites/setup section around the actual default coordinator and lane recipes. Include Node/npm, Claude installation/login, Pi setup, and the wrapper link already printed by setup. Link supported installation instructions rather than assuming an existing personal machine. Do not add another installer or recipe family. |
| 3 — diagnosis stop | `open` gives no timely explanation of an already visible missing executable | Silent for 240 seconds; pane immediately contains `claude: command not found`. Doctor later says the coordinator does not resolve and suggests ticker/open | **P3-S5/code rewrite:** rewrite the existing start/readiness result propagation to surface this command failure instead of waiting for agent readiness and then describing only an unresolved pane. Make the troubleshooting paragraph distinguish missing CLI, missing login, and missing session. No new watchdog or recovery flag. |
| 4 — incorrect safety promise | README and Operations promise normal permission prompts for destructive commands, while the shipped coordinator launch bypasses them and ADE refuses a recipe without that flag | Actual launch contains `--dangerously-skip-permissions`; follow-up without it fails `recipe_permission_missing` | **P3-D10 + P3-S5:** delete/rewrite the allow-list promise to describe the shipped recipe truthfully. This proves the required contradictory flag, not the behavior of a signed-in destructive command. No provider or destructive-operation test was run. |
| 5 — broken next action | Setup installs `herdr-ade` and `herdr-pi`, but diagnostics tell the stranger to run nonexistent `ha` | `ha doctor`: exit 127; `herdr-ade doctor` ends with `next: ha doctor --timings` before any `ha` was installed | **P3-S5; related to P3-D9's path cleanup:** rewrite generated next commands to use the already resolved executable prefix. Do not make a third alias a hidden prerequisite. My temporary symlink was a probe workaround, not a proposed feature. |
| 6 — misleading health signal | README's plugin doctor action returns a “started” JSON record rather than the checks; its later log reports success despite failed checks | Invoke exit 0; plugin log exit 0/`succeeded`, stdout contains multiple `[FAIL]` rows | **P3-S5:** rewrite the README verification command to use the documented terminal `herdr-ade doctor`, whose nonzero result and diagnostics were usable. Rewrite the existing action's result propagation if retained; do not add another health command. |
| 7 — unattended operation gap | Startup reports success but no ticker remains; creating a project did not remedy it in this run | Startup log exit 0; repeated `ticker status` says not running, including after `new`; explicit `ha ticker start` succeeds and stays running | **P3-S5/code rewrite:** rewrite the existing startup result/error reporting so it does not equate successful invocation with a running ticker. Rewrite “Check your setup” with the existing `ticker start`/`status` recovery. Root cause was not diagnosed from source; this is the observed local-link/headless path, not proof about a future GitHub-managed install. |
| 8 — no-login/CLI stopping point unexplained | A fresh account has no coordinator-captured request id with which to start a lane | `request_authority: no request ... in project billing`; suggested task command still needs an existing id | **P3-S5:** rewrite “Start work” and the direct-CLI examples to show that a real coordinator conversation must first supply a recorded request. Explain this no-login stopping point. Preserve request authority; do not add a bypass or manually synthesize state. The error is accurate but is not provider-login guidance. |
| 9 — discovery friction, not missing U4 | Docs mention bundled Rundown but do not explain the working automatic path. Raw pane launch is insufficient | Raw pane exits; binary says `HERDR_RUNDOWN_PROJECT is not set`. Ticker subsequently opens a working project-bound Rundown | **U4 + P3-S5:** rewrite the setup walkthrough to say how the existing project-bound Rundown appears, and show one plan example. Delete any separate-install implication. No second plugin or new pane mechanism. |
| 10 — lower-priority portability/documentation | Personal-machine language leaks into public help; public Herdr lacks the fork's parent flag | Doctor: `CLI has no --parent; post-start fallback only`; plugin doctor says “this Mac workspaces” on Linux; docs address Rolf throughout | **P3-S5:** rewrite the public compatibility/setup language around what was actually verified, and generic local-machine wording. The parent warning did not block project/plan/Rundown; nesting was not verified. **P3-D9:** coordinator compaction/handoff was not exercised, so this run does not confirm or clear its hard-coded-root defect. |
| 11 — development gate, not install stop | Clippy fails at the documented Rust minimum on the unchanged snapshot | Four `nonminimal_bool` diagnostics; locked release build and tests pass | **P3-S5 operations/development reference:** keep toolchain/gate instructions accurate; simplify the four existing expressions rather than suppressing the lint or adding compatibility machinery. Outside this report-only lane's edit scope. |

Additional friction, not independent release blockers: `$EDITOR` was unset; the guide assumes shell/editor setup. The bundle's missing HEAD, noninteractive `--yes`, explicit named-session selection, and raw plugin-pane argument experiments are documented below as test-path adaptations, not attributed to ADE as fresh defects.

## Ordered step log

### 0. Establish isolation and simulate the unpublished repository

The task prescribed account creation and a bundle from main. As `ubuntu`:

```bash
sudo useradd -m -s /bin/bash herdr-stranger
git -C /home/ubuntu/projects/herdr-ade bundle create /tmp/herdr-ade.bundle main
chmod 644 /tmp/herdr-ade.bundle
```

- `useradd`: 0.108 s; account row `herdr-stranger:x:1002:1002::/home/herdr-stranger:/bin/bash`.
- Bundle: 0.538 s; 6.7 MB. No credentials or real installed binaries were copied.
- Outer baseline, untimed: `pwd`, `git status --short`, `git rev-parse HEAD`, `git branch --show-current`, `git -C /home/ubuntu/projects/herdr-ade rev-parse main`, `command -v herdr`, `command -v ha`, `id`, `getent passwd herdr-stranger`, `date -u +'%Y-%m-%dT%H:%M:%SZ'`. Worktree clean; no pre-existing throwaway account.
- Read-only real-account baseline: `herdr --help`, `herdr status`, `herdr session list`, `herdr workspace list`, `herdr tab list`, and `ps -u ubuntu -o pid,lstart,comm | grep -E 'herdr|PID'`. Comparison is in Cleanup.

As the stranger, initial `date`/`id`: 0.001 s each, UID 1002 and timestamp 05:30:48. The following prerequisite inventory took 0.007 s (final exit 127 because npm was absent):

```bash
printf 'HOME=%s\nPATH=%s\n' "$HOME" "$PATH"
for tool in herdr rustc cargo cc git curl gh ssh rsync node npm claude pi; do
  command -v "$tool" || true
done
uname -sm
git --version
cc --version | head -n 1
rustc --version
cargo --version
node --version
npm --version
```

Available system prerequisites: Git 2.43.0, GCC 13.3.0, curl, gh, SSH, rsync. No Herdr, Rust/Cargo, Node/npm, Claude, or Pi on this account's PATH. Nothing was installed system-wide.

| Command | Trimmed result | Seconds / exit |
|---|---|---|
| `env GIT_TERMINAL_PROMPT=0 git clone https://github.com/uguryildirim24/herdr-ade.git "$HOME/herdr-ade-public"` | `fatal: could not read Username for 'https://github.com': terminal prompts disabled` | 0.070 / 128 |
| `git clone /tmp/herdr-ade.bundle "$HOME/herdr-ade"` | `warning: remote HEAD refers to nonexistent ref, unable to checkout` | 0.445 / 0 |
| `git -C "$HOME/herdr-ade" status --short --branch` | `No commits yet on master...origin/master [gone]` | 0.002 / 0 |
| `git -C "$HOME/herdr-ade" rev-parse HEAD main` | ambiguous `HEAD` | 0.001 / 128 |
| `git -C "$HOME/herdr-ade" checkout main` | switched to main, tracking origin/main | 0.023 / 0 |
| `git -C "$HOME/herdr-ade" rev-parse HEAD` | `649b5ae3fed451e8d02f11cd4efe5945955934c7` | 0.002 / 0 |

The checkout correction is a **bundle-only workaround**, not a finding against public clone. `chmod o+x "$HOME"` took 0.001 s so the lane's file-reading tool could read public downloaded docs/logs after an initial permission-denied read. No credential file was read or created for login.

### 1. Follow the linked Herdr installation path

ADE requires Herdr 0.9.1+. The linked herdr.dev page and agent guide say:

```bash
curl -fsSL https://herdr.dev/install.sh | sh
herdr
```

Public documentation fetches, all successful:

| Exact command | Seconds |
|---|---:|
| `curl -fLsS https://herdr.dev -o "$HOME/herdr.dev.html"` | 0.067 |
| `curl -fLsS https://herdr.dev/agent-guide.md -o "$HOME/herdr-agent-guide.md"` | 0.058 |
| `curl -fLsS https://herdr.dev/docs/cli-reference/ -o "$HOME/herdr-cli.html"` | 0.072 |
| `curl -fLsS https://herdr.dev/docs/install/ -o "$HOME/herdr-install.html"` | 0.079 |

The actual install ran as `bash -c 'curl -fsSL https://herdr.dev/install.sh | sh'`: **0.379 s, exit 0**. Output:

```text
detected linux/aarch64
downloading v0.9.3...
installed herdr to /home/herdr-stranger/.local/bin/herdr
ready. run 'herdr' to get started.
```

`herdr --version`: 0.005 s, `herdr 0.9.3`. `herdr status`: 0.004 s, client 0.9.3, server not running at the **stranger's** default socket. `herdr plugin --help`: 0.005 s; exposes `install`, `link`, `list`, actions and panes.

I used the public CLI reference's headless `server` command later, with the required isolated session, instead of attaching an interactive onboarding client. Full mouse/onboarding behavior was therefore not tested.

### 2. Satisfy Rust prerequisite; attempt plugin install literally

Getting started requires Rust 1.89+, Cargo, a C compiler and Git. The compiler/Git were present. The user-local Rust workaround was:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
  sh -s -- -y --profile minimal --default-toolchain 1.89.0
```

7.240 s, exit 0. `rustc --version`: 0.033 s, 1.89.0. `cargo --version`: 0.062 s, 1.89.0. The exact minimum built ADE successfully below.

The docs say `herdr plugin install uguryildirim24/herdr-ade`, with a locked release build. All plugin operations were scoped to `scratch-t-0711`:

| Exact command | Trimmed result | Seconds / exit |
|---|---|---|
| `env GIT_TERMINAL_PROMPT=0 herdr --session scratch-t-0711 plugin install uguryildirim24/herdr-ade` | `remote plugin install requires --yes when stdin is not interactive` | 0.002 / 2 |
| `herdr --session scratch-t-0711 plugin link "$HOME/herdr-ade"` | linked ADE, enabled; shows release-build command and bundled `rundown` pane | 0.010 / 0 |
| `herdr --session scratch-t-0711 plugin list` | one enabled local plugin at `/home/herdr-stranger/herdr-ade` | 0.005 / 0 |
| `herdr --session scratch-t-0711 plugin unlink herdr-ade` | `server_not_running`, names stranger's scratch socket | 0.001 / 1 |
| `env GIT_TERMINAL_PROMPT=0 herdr --session scratch-t-0711 plugin install uguryildirim24/herdr-ade --yes` | Git exit 128: cannot read GitHub username | 0.082 / 1 |
| `herdr --session scratch-t-0711 plugin link "$HOME/herdr-ade"` | linked local plugin again | 0.002 / 0 |
| `cd "$HOME/herdr-ade" && cargo build --release --locked` | `Finished release profile [optimized] ... in 46.88s` | 46.936 / 0 |

**Substitution:** public Herdr offers `plugin link <local checkout>`, not `plugin install <local path>`. Linking did not build the binary, so I ran the exact build command exposed in its output. No manifest/source reading was necessary. The attempted unlink before the authenticated-install retry could not run until a server existed; the remote retry still reached and demonstrated the private-repository failure.

### 3. Terminal links and the documented routing configuration

Getting started says to find `PLUGIN_ROOT`, link both executables, and edit config. I ran:

```bash
PLUGIN_ROOT="$HOME/herdr-ade"
mkdir -p ~/.local/bin ~/.config/herdr-ade
ln -s "$PLUGIN_ROOT/target/release/herdr-ade" ~/.local/bin/herdr-ade
ln -s "$PLUGIN_ROOT/target/release/herdr-pi" ~/.local/bin/herdr-pi
printf 'EDITOR=%s\n' "${EDITOR-unset}"
```

0.004 s, exit 0, `EDITOR=unset`. Instead of guessing an editor, I wrote exactly the documented starting block to `~/.config/herdr-ade/config.toml` with `printf '%s\n'` (less than 0.001 s):

```toml
[routing]
default = "pi_codex_sol_high"
retries = 1

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

No personal recipe or machine configuration was copied. No remote machine was enabled. Doctor accepted this table and the six shipped recipes.

### 4. Pi setup exposes an undocumented prerequisite

The guide says `herdr-pi setup`, required logins, then `herdr-pi doctor`. Logins were intentionally skipped.

| Command | Trimmed result | Seconds / exit |
|---|---|---|
| `herdr-pi setup` | `could not run npm: No such file or directory` | 0.005 / 1 |
| `herdr-pi doctor` | Node/npm, wrapper, settings, integration missing; interactive-shell job-control noise | 1.068 / 1 |
| `ha doctor` | `ha: command not found` | 0.001 / 127 |
| `herdr-ade doctor` | recipes valid; no ticker; Pi prerequisite failures; ends `next: ha doctor --timings` | 0.558 / 1 |

The missing Node/npm are not merely missing logins. Doctor's shell suggests `apt install nodejs`/`npm`; I did **not** run apt. The user-local workaround used the Node 24 line mentioned in Operations' development section, not a claim that 24 is the proven minimum:

```bash
set -e
cd "$HOME"
curl -fLsS https://nodejs.org/dist/latest-v24.x/SHASUMS256.txt -o node-shasums.txt
node_file=$(awk '/ linux-arm64.tar.xz$/ {print $2}' node-shasums.txt)
if [ -z "$node_file" ]; then
  node_file=$(awk '/node-v24.*-linux-arm64.tar.xz$/ {print $2}' node-shasums.txt)
fi
printf 'Selected %s\n' "$node_file"
curl -fLsS "https://nodejs.org/dist/latest-v24.x/$node_file" -o "$node_file"
grep " $node_file\$" node-shasums.txt | sha256sum --check
mkdir -p "$HOME/.local/node24"
tar -xJf "$node_file" --strip-components=1 -C "$HOME/.local/node24"
for tool in node npm npx; do
  ln -s "$HOME/.local/node24/bin/$tool" "$HOME/.local/bin/$tool"
done
```

2.300 s, exit 0; selected `node-v24.21.0-linux-arm64.tar.xz`, checksum OK. `node --version`: 0.042 s, v24.21.0. `npm --version`: 0.172 s, 11.19.0.

| Command | Trimmed result | Seconds / exit |
|---|---|---|
| `herdr-pi setup` | installs `@earendil-works/pi-coding-agent@0.99.1`; writes only stranger's Pi folder, guard and Herdr integration | 9.158 / 0 |
| `herdr-pi doctor` | wrapper not on PATH; `credentials_not_configured` | 1.190 / 1 |
| `herdr-pi login --help` | one provider: `openai-codex`, `opencode-go`, `kimi-coding` | 0.001 / 0 |
| `ln -s "$HOME/.herdr-ade/pi/bin/pi" "$HOME/.local/bin/pi"` | follows the exact extra line printed by setup | 0.001 / 0 |
| `herdr-pi doctor` | all installation checks OK; only `openai-codex/gpt-5.6-sol` sign-in fails | 1.266 / 1 |

The last diagnostic is useful:

```text
[FAIL] provider openai-codex/gpt-5.6-sol login: missing sign-in:
credentials_not_configured (run `herdr-pi login openai-codex`)
```

To exercise the brief's `ha` commands, I then used the explicit, undocumented probe alias:

```bash
ln -s "$HOME/.local/bin/herdr-ade" "$HOME/.local/bin/ha"
```

0.001 s, exit 0. It points to the same newly built binary, not to ubuntu's `ha`.

### 5. Start the isolated server; check the README doctor action

This occurred after the first failed Pi setup and before the Node workaround above:

```bash
nohup herdr --session scratch-t-0711 server > "$HOME/herdr-server.log" 2>&1 < /dev/null &
echo "throwaway server pid=$!"
sleep 1
```

1.002 s including sleep; PID 1178435. `herdr --session scratch-t-0711 status`: 0.103 s, client and server 0.9.3, compatible, socket `/home/herdr-stranger/.config/herdr/sessions/scratch-t-0711/herdr.sock`.

| Command | Trimmed result | Seconds / exit |
|---|---|---|
| `herdr-ade ticker status` | not running | 0.002 / 0 |
| `herdr --session scratch-t-0711 plugin action list --plugin herdr-ade` | seven ADE actions including new/open/doctor | 0.003 / 0 |
| `herdr --session scratch-t-0711 plugin action invoke doctor --plugin herdr-ade` | JSON `plugin_action_invoked`, log status `running`; no checks in the response | 0.002 / 0 |
| `herdr --session scratch-t-0711 plugin log list --plugin herdr-ade --limit 5` | startup: exit 0, succeeded; doctor: exit 0, succeeded, but stdout contains Pi `[FAIL]` checks and ticker not running | 0.002 / 0 |

The log command was repeated later with the same two entries (0.002 s). The command is discoverable in the linked public CLI reference, but the README's check command alone does not tell a stranger that its response is only an invocation receipt.

### 6. Create the scratch repository and project; try `open`

The guide's example is `new "Billing" --repo ~/dev/app`, then `open billing`. I first supplied an actual empty Git repository with a commit and **repository-local** identity:

```bash
mkdir -p "$HOME/dev/app"
git -C "$HOME/dev/app" init -b main
git -C "$HOME/dev/app" config user.name 'Stranger Probe'
git -C "$HOME/dev/app" config user.email 'stranger@example.invalid'
printf '# Empty scratch repository\n' > "$HOME/dev/app/README.md"
git -C "$HOME/dev/app" add README.md
git -C "$HOME/dev/app" commit -m 'Create scratch repository'
```

0.179 s, exit 0; scratch commit `1f526e4`. No global Git identity or GitHub remote was configured.

| Command | Trimmed result | Seconds / exit |
|---|---|---|
| `ha new Billing --repo "$HOME/dev/app"` | created `/home/herdr-stranger/.herdr-ade/billing`; next `ha open billing` | 0.840 / 0 |
| `ha open billing` | stranger's default session not reachable; run Herdr | 0.044 / 1 |
| `ha open billing --session scratch-t-0711` | no terminal response before outer 240-second command timeout | 240-second observation cutoff; no captured command exit |

Passing the named session is the guide's documented recovery and the lane's required isolation, not a product defect. During the silent second open:

- `herdr --session scratch-t-0711 workspace list`: 0.002 s, Billing `w1` exists.
- `herdr --session scratch-t-0711 tab list`: 0.002 s, coordinator `w1:t1` exists.
- `herdr --session scratch-t-0711 agent list`: 0.002 s, `hp-billing-coordinator`, `launch_pending:true`, `unknown`.
- An outer `ps -u herdr-stranger -o pid,ppid,etimes,comm` showed the original `ha` child still alive at about 249 seconds, even though the tool invocation had timed out. It was gone at the subsequent 311-second process snapshot. I do not infer a successful open or an exact internal timeout from that.
- `herdr --session scratch-t-0711 pane read w1:p1`: 0.002 s, displayed:

```text
claude --model claude-opus-5 --dangerously-skip-permissions --effort high --disallowedTools Agent
claude: command not found
```

`ha context billing --peek --full`: 0.065 s, shows all six recipes and `/home/herdr-stranger/dev/app: main, clean, no tracking remote`. This establishes that the starter routing block was sufficient to load recipes, not sufficient to install the coordinator CLI.

Later `ha doctor`: 0.729 s, exit 1. Pi installation healthy; explicit Pi login instruction; project workspace/pane exist but coordinator does not resolve. It still did not name the missing Claude executable in its summary. Its default-session warning was expected because only a named session was running.

The first pass stopped here and was cleaned up. Before sealing, I revisited the missing Claude executable in a focused, fully isolated follow-up below, so that an installable prerequisite would not be mistaken for the final human boundary. A logged-in coordinator, compaction, and a working agent lane remain outside this run's evidence.

### 7. Plan and Rundown without an agent

Operations documents plan commands. No provider is required for them:

| Exact command | Output | Seconds / exit |
|---|---|---|
| `ha plan show billing` | no plan, revision 0 | 0.002 / 0 |
| `ha plan set billing --does 'A scratch project with a visible plan, without logging in'` | revision 1 set | 0.016 / 0 |
| `ha plan step add billing 'Verify the scratch project opens'` | revision 2, added s-1 | 0.013 / 0 |
| `ha plan show billing` | revision 2; s-1 is left | 0.002 / 0 |

The missing goal is expected: I did not supply `--goal`. There was no task completion to project as done.

To discover Rundown using the public Herdr pane interface:

```bash
herdr --session scratch-t-0711 plugin pane open \
  --plugin herdr-ade --entrypoint rundown --placement tab --workspace w1 \
  --cwd "$HOME/.herdr-ade/billing"
```

0.005 s, exit 0, reports `w1:p2`/`w1:t2`; the pane immediately disappears. `tab list` and `pane list` (0.002 s each) showed only the coordinator. Direct help attempt:

```bash
timeout 5 "$HOME/herdr-ade/target/release/herdr-rundown" --help
```

0.002 s, exit 1, `herdr-rundown: HERDR_RUNDOWN_PROJECT is not set`.

Ticker observations and recovery:

| Command | Output | Seconds / exit |
|---|---|---|
| `ha ticker status` after project creation | not running | 0.002 / 0 |
| `ha ticker start` | empty stdout | 0.003 / 0 |
| `ha ticker status` | running, PID 1321898, correct binary/root, started 05:39:32Z | 0.003 / 0 |

A later `ps -u herdr-stranger -o pid,ppid,etimes,args` (0.013 s) showed a running Rundown process, and `tab list`/`pane list` (0.002 s each) showed automatically created **`w1:p3` / `w1:t3`**. I also tried the raw pane command again with `--env HERDR_RUNDOWN_PROJECT=billing` (0.005 s). That guessed slug was insufficient and `w1:p4` disappeared; it did not create the working pane. No other environment key or hidden root was invented.

`herdr --session scratch-t-0711 pane read w1:p3` (0.002 s) showed:

```text
Billing
A scratch project with a visible plan, without logging in
0 of 1
Verify the scratch project opens
```

This is the working **U4 main-plugin Rundown**, opened by the project machinery after ticker start. It needed no separate plugin or binary link in `~/.local/bin`.

`ha plan sync billing`: 0.007 s, revision 2, no state changed. Repeated `ha plan show billing` calls (0.002 s each) preserved 0 completed of 1 throughout. At plugin build time the throwaway root had zero projects; no pre-existing plan count could drop. No harness install or plan operation targeted any real project.

### 8. One direct lane-start attempt

Operations says start can create a stable task when title, request and acceptance are supplied. `ha task add --help` (0.002 s) confirms that request is an existing request id. I used the actual request id from this lane's brief, not an invented successful local receipt, to expose the fresh-account boundary:

```bash
printf '%s\n' \
  'No-login setup probe. Do not modify any files or contact any provider.' \
  'A model process, if any is started, must use --model claude-haiku-4-5-20251001.' \
  > "$HOME/lane.md"

timeout 45 ha thread start billing \
  --title 'No-login setup probe' --repo "$HOME/dev/app" \
  --request q-1791005100705-79763-0 \
  --acceptance 'Explain the missing login without changing files' \
  --task-file "$HOME/lane.md"
```

File write: 0.001 s. Start: **0.003 s, exit 1**, not the timeout:

```text
herdr-ade: request_authority: no request `q-1791005100705-79763-0` in project `billing`
next: ha task add billing --title "No-login setup probe" --request <existing-request-id> --acceptance "<condition>"
```

`ha thread list billing` before and after: 0.043 s each, empty. No worktree or agent lane was created. This is the **only** lane-start attempt. It cannot be cited as a provider-readiness failure: request authority prevented reaching that check. The separate Pi doctor output is the evidence for a useful provider-login message. I did not write `.state/requests` or make the gate disappear to manufacture the expected failure.

### 9. Check U4's New project repository prompt

The main plugin's `new` pane was present in the initial link output. Public Herdr documents pane placement overrides, so a tab made the text inspectable without an attached graphical client.

| Command | Trimmed output | Seconds / exit |
|---|---|---|
| `herdr --session scratch-t-0711 plugin pane open --plugin herdr-ade --entrypoint new --placement popup --workspace w1` | overlay/popup targets active pane; supplied workspace invalid | 0.002 / 1 |
| `herdr --session scratch-t-0711 plugin pane open --plugin herdr-ade --entrypoint new --placement tab --workspace w1` | created w1:p5 | 0.005 / 0 |
| `herdr --session scratch-t-0711 pane read w1:p5` | `New project`, `Name:` | 0.002 / 0 |
| `herdr --session scratch-t-0711 pane close w1:p5` | OK | 0.007 / 0 |
| `herdr --session scratch-t-0711 plugin pane open --plugin herdr-ade --entrypoint new --placement tab --workspace w1` | created w1:p6 | 0.005 / 0 |
| `herdr --session scratch-t-0711 pane run w1:p6 'UI-only probe'` | submitted the name, no agent involved | 0.002 / 0 |
| `herdr --session scratch-t-0711 pane read w1:p6` | `Name: UI-only probe` followed by **`Repository:`** | 0.002 / 0 |
| `herdr --session scratch-t-0711 pane close w1:p6` | OK; no second project submitted | 0.007 / 0 |

The popup argument refusal was my headless-probe adaptation, not an ADE new-project defect. The repository prompt and successful CLI creation directly support U4.

### 10. Auxiliary help/inspection used, not hidden knowledge

These were read-only discovery steps inside the throwaway account:

| Command | Seconds / result |
|---|---|
| `herdr-ade --help` | 0.002, commands and global root/JSON options |
| `ha plan --help` | 0.002, show/set/step/sync |
| `ha thread start --help` | 0.002, task-file, job/request/acceptance, recipe; no model flag |
| `herdr --session scratch-t-0711 pane --help` | 0.002, read/run/close/list controls |
| `herdr --session scratch-t-0711 pane send-text --help` | 0.002, points to `pane run` for text plus Enter |

The public HTML was searched with `rg` for install, plugin link, named sessions, pane commands, and send-text syntax. The downloaded agent guide and relevant public CLI sections were read with the file-reading tool. The account's transcript and gate logs were inspected; no real-account log or credential store was read.

### 11. Follow-up: install Claude and reach the actual human boundary

I recreated only the same throwaway account/session after the first cleanup. This is a continuation of the investigation, not a second lane-start attempt. No new `thread start` was run.

Outer bootstrap repeated exactly `sudo useradd -m -s /bin/bash herdr-stranger` (0.075 s), the prescribed `git -C /home/ubuntu/projects/herdr-ade bundle create /tmp/herdr-ade.bundle main` (0.532 s), and `chmod 644 /tmp/herdr-ade.bundle`. UID was again 1002. **Main had advanced to `f99739efdf37bbf35358ec48847d9d1a69f7dfee` concurrently.** The first replay build used that tip; after noticing the changed SHA I checked out the original pinned commit and rebuilt before linking/starting ADE. No runtime result below is attributed to the advanced tip.

Same clean-environment wrapper as above:

| Command | Trimmed output | Seconds / exit |
|---|---|---|
| `id`; `date -u +%Y-%m-%dT%H:%M:%SZ`; `chmod o+x "$HOME"` | UID 1002; 05:50:54Z; traversal for report-tool reads | 0.001 each / 0 |
| `git clone --branch main /tmp/herdr-ade.bundle "$HOME/herdr-ade"` | checkout succeeds, no bundle-HEAD warning | 0.461 / 0 |
| `curl -fsSL https://herdr.dev/install.sh \| sh` | Herdr 0.9.3 in stranger's local bin | 0.402 / 0 |
| Rustup command from step 2, unchanged | Rust/Cargo 1.89.0 installed locally | 6.975 / 0 |
| `cd "$HOME/herdr-ade" && cargo build --release --locked` | release build of the moved main; no runtime used from it | 43.688 / 0 |
| Node procedure from step 4, using `node_file=$(awk '/node-v24.*-linux-arm64.tar.xz$/ {print $2}' node-shasums.txt)` directly | same v24.21.0 archive, checksum OK, same local links | 2.289 / 0 |
| `npm install --global --prefix "$HOME/.local" @anthropic-ai/claude-code@2.1.287` | added 2 packages; npm warns postinstall script not allowed, but binary works without enabling it | 3.606 / 0 |
| `claude --version` | `2.1.287 (Claude Code)` | 0.075 / 0 |
| `claude auth status` | `loggedIn: false`, `authMethod: none`, config under stranger's home | 0.307 / 1 |
| `git -C "$HOME/herdr-ade" checkout --detach 649b5ae3fed451e8d02f11cd4efe5945955934c7` | pinned HEAD restored | 0.011 / 0 |
| `cd "$HOME/herdr-ade" && cargo build --release --locked` | pinned release rebuilt in 38.22 s | 38.245 / 0 |

The two build outputs were redirected to `$HOME/followup-build.log` and `$HOME/pinned-build.log`, with the last three lines printed and the status propagated. The Claude package/version came from the existing Operations development section; only `--prefix "$HOME/.local"` was added to keep the global npm install account-local. `claude auth status` was an additional read-only CLI probe, not an instruction printed in ADE's setup guide. No npm upgrade or install-script permission configuration was applied.

Re-created links (0.005 s):

```bash
mkdir -p "$HOME/.config/herdr-ade"
for tool in herdr-ade herdr-pi; do
  ln -s "$HOME/herdr-ade/target/release/$tool" "$HOME/.local/bin/$tool"
done
ln -s "$HOME/.local/bin/herdr-ade" "$HOME/.local/bin/ha"
```

I wrote the same routing table as step 3, plus this explicitly disclosed model-only probe recipe, using the recipe fields documented in Operations (less than 0.001 s):

```toml
[recipes.claude_coordinator_opus]
kind = "claude"
args = ["--model", "claude-haiku-4-5-20251001", "--disallowedTools", "Agent"]
enabled = true
plain = "No-login Haiku setup probe"
```

This initially omitted the default permission-bypass flag. The result below was instructive; it was not an attempt to claim default permission behavior.

| Command | Result | Seconds / exit |
|---|---|---|
| `herdr-pi setup` | same Pi 0.99.1 installation and generated local integration | 9.205 / 0 |
| `ln -s "$HOME/.herdr-ade/pi/bin/pi" "$HOME/.local/bin/pi"` | link created | 0.001 / 0 |
| `herdr --session scratch-t-0711 plugin link "$HOME/herdr-ade"` | linked pinned local plugin | 0.013 / 0 |
| Headless-server command from step 5, unchanged | PID 1498459, includes one-second sleep | 1.003 / 0 |
| Scratch-repository commands from step 6, unchanged | fresh scratch commit c5ffbe3 | 0.016 / 0 |
| `ha new Billing --repo "$HOME/dev/app"` | created billing | 0.015 / 0 |
| `timeout 20 ha open billing --session scratch-t-0711` | `recipe_permission_missing: claude_coordinator_opus has no permission flag --dangerously-skip-permissions` | 0.192 / 1 |
| `herdr --session scratch-t-0711 pane read w1:p1` | pane not found; no process was launched | 0.002 / 1 |
| `herdr --session scratch-t-0711 agent list` | empty | 0.002 / 0 |
| `claude auth status` | still logged out | 0.227 / 1 |

I rewrote only that temporary config to preserve the flag already required by the shipped recipe, keeping Haiku and disabling Agent (0.029 s):

```toml
args = ["--model", "claude-haiku-4-5-20251001", "--dangerously-skip-permissions", "--disallowedTools", "Agent"]
```

Then:

| Command | Result | Seconds / exit |
|---|---|---|
| `timeout 20 ha open billing --session scratch-t-0711` | `added the Rundown tab (w1:t2)`; `opened billing in workspace w1 (pane w1:p1)` | **4.260 / 0** |
| `herdr --session scratch-t-0711 pane list` | Claude pane plus bundled Rundown | 0.002 / 0 |
| `herdr --session scratch-t-0711 pane read w1:p1` | exact Haiku launch line, Claude welcome/theme selection | 0.002 / 0 |
| `claude auth status` | `loggedIn: false`, `authMethod: none` | 0.217 / 1 |

The visible process stopped at:

```text
claude --model claude-haiku-4-5-20251001 --dangerously-skip-permissions --disallowedTools Agent
Welcome to Claude Code v2.1.287
Let's get started.
Choose the text style that looks best with your terminal
```

No keys were sent to onboarding, no browser flow was opened, and no provider login was performed. `open` success here establishes workspace/process creation and bundled Rundown, **not** an authenticated coordinator ready to work. This also rules out inability to install Claude on this machine as the final blocker. Follow-up cleanup is recorded below.

## Human steps intentionally not performed

| Human step | What the public docs actually say | This run |
|---|---|---|
| GitHub repository access | README and setup explicitly require access to the private repository; gh is optional for GitHub operations | Anonymous access failed. No GitHub sign-in; authorized bundle substituted. Publishing removes the install-time private-access step. |
| ChatGPT/Pi | `herdr-pi setup`, `herdr-pi login`, then doctor; login is local to the machine and credentials must not be copied | Setup completed; doctor names `herdr-pi login openai-codex`. Login help inspected only. No login opened or token transferred. |
| Other Pi providers | Complete each required login; executable routes live in config | Login help lists opencode-go and kimi-coding too. No unnecessary provider was configured or signed in. |
| Claude coordinator | Prerequisites say an agent CLI; provider troubleshooting says use that provider's login flow on the same machine; Operations development gives a pinned npm install | Initially absent; follow-up installed CLI 2.1.287 account-locally and opened a Haiku-configured onboarding pane. `claude auth status` remained logged out. No Claude sign-in on oci. The first-project setup still lacks this explicit default-coordinator installation/login sequence. |
| Web research | Starter routes web-research to agy | Not part of this local no-login scratch task; no agy installation, login, or research process attempted. |

## Repository gates

These ran **after the setup observations**, in the unchanged bundle checkout as the stranger, not against the real account's installed harness. Rustfmt/Clippy components were user-local: `rustup component add rustfmt clippy`, 0.976 s, exit 0.

| Command, under `$HOME/herdr-ade` | Result | Wall time |
|---|---|---:|
| `cargo fmt --check` | pass | 1.183 s |
| `cargo test` | pass: 571 passed, 1 ignored, 0 failed across the reported binaries/integration suites | 133.747 s |
| `cargo clippy --all-targets -- -D warnings` | **baseline failure**, exit 101; four `clippy::nonminimal_bool` diagnostics | 21.768 s |
| `git diff --check` | pass | 0.003 s |

Test/Clippy stdout and stderr were redirected to `$HOME/cargo-test.log` and `$HOME/cargo-clippy.log`; the shell printed the final 24/20 lines and propagated the saved command status. Full failure diagnostics were then read. Locations:

```text
src/branches.rs:231      (a && !sealed) || (!sealed && b)
src/events.rs:451        !extension().is_some_and(...)
src/record_cache.rs:97   !extension().is_some_and(...)
src/review.rs:251        !extension().is_some_and(...)
```

No suppression or source fix was made: this is an observed failure of the pinned snapshot with Rust/Clippy 1.89.0, not a regression introduced by the report. The release install build passed. Both `git -C "$HOME/herdr-ade" status --short` (0.003 s) and `git -C "$HOME/dev/app" status --short` (0.002 s) were empty after the setup; they remained empty after the gates.

## Cleanup and real-account comparison

Only throwaway-owned runtime resources were stopped or deleted:

1. `ha ticker stop` as the stranger: 0.429 s, exit 0. Subsequent final plan read still showed revision 2, 0 of 1 done.
2. `herdr session stop scratch-t-0711` as the stranger: 0.202 s, `stopped session scratch-t-0711`.
3. `herdr session delete scratch-t-0711`: 0.001 s, deleted. `herdr session list`: 0.001 s, only stranger's stopped default remained.
4. Checked `getent passwd herdr-stranger`, UID **1002**, home **`/home/herdr-stranger`**, before deletion. `ps -u herdr-stranger -o pid,ppid,comm` was already empty.
5. `sudo pkill -u herdr-stranger`: 0.024 s, exit 1, no remaining matching processes.
6. Rechecked UID/home, then `sudo userdel -r herdr-stranger`: 1.010 s. Only warning: no mail spool at `/var/mail/herdr-stranger`. Account, group and home subsequently absent.
7. `rm /tmp/herdr-ade.bundle`: 0.003 s; bundle absent.
8. `sudo find /tmp /var/tmp -xdev -uid 1002 -print` found one gate-created `/tmp/hp-test-log-1364603`. Verified regular file UID/GID 1002 with `stat`, then `sudo rm /tmp/hp-test-log-1364603`: 0.008 s. Repeated find produced no output.
9. `ps -U 1002 -o pid,ppid,comm`: no processes. Final checks: throwaway home and bundle absent, worktree diff check clean.

No system package was installed, so there was no system package to undo. Removing the home removed the public Herdr download, Rustup/toolchains, Node/npm, Pi package and generated integration/settings, linked plugin checkout/builds, scratch repository, ADE project/plan, logs, and downloaded documentation. No separately linked plugin remained outside that deleted account.

Read-only `ubuntu` checks before/after (`herdr status`, `session list`, `workspace list`, `tab list`, process metadata):

| Item | Before | After |
|---|---|---|
| Herdr client/server | 0.9.1 / 0.9.1, running, compatible | same, no restart needed/stale binary |
| Default socket | `/home/ubuntu/.config/herdr/herdr.sock` | same |
| Existing Herdr process PIDs/start times | 2822825 / Oct 3 02:15:53; 3223223 / Sep 22 12:55:50; 3550184 / Sep 23 01:24:43 | all three unchanged |
| Home workspace/tab | wAZ / wAZ:t1 | unchanged |
| This lane | wBN:t2, t-0711 | still working, same id |
| Parallel lane | wBN:t1, t-0710 | no longer listed |

**Concurrent activity caveat:** t-0710 disappeared between snapshots, and an `ubuntu` herdr-ade process with start time 05:43:04 appeared. I did not investigate its content or attribute its lifecycle. No command in this probe addressed, closed, or restarted an ubuntu pane/server, and no throwaway process could acquire ubuntu's UID through this setup. The snapshots confirm unchanged Herdr server processes and this lane's identity, not a promise that another live lane would remain static. No real configuration, binary directory, credentials, or login store was directly inspected, written, or copied; the explicitly permitted read-only Herdr list/status commands were the only real-session inspection. The only real-repository operations beyond read-only inspection were the explicitly requested bundle and this report's Git delivery.

### Follow-up cleanup and final check

The second account was fully removed too:

- As the stranger: `ha ticker stop` (0.404 s), `herdr session stop scratch-t-0711` (0.303 s), and `herdr session delete scratch-t-0711` (0.001 s), all exit 0. This stopped the onboarding-only Claude process as well as its Herdr session.
- Checked `getent passwd herdr-stranger` and empty `ps -u herdr-stranger -o pid,ppid,comm`; `sudo pkill -u herdr-stranger` took 0.025 s and returned 1, no remaining processes.
- Rechecked UID 1002/home `/home/herdr-stranger`, then `sudo userdel -r herdr-stranger` (0.866 s; same harmless missing-mail-spool warning), and `rm /tmp/herdr-ade.bundle` (0.003 s).
- UID-scoped temporary-file inspection found two **empty**, throwaway-owned directories: `/tmp/claude-1002` and `/tmp/cc-socks`. Verified each UID/GID with `stat`, then used non-recursive `sudo rmdir` (0.008 s each), so unrelated contents could not be removed.
- Final `sudo find /tmp /var/tmp -xdev -uid 1002 -print`, `getent passwd/group herdr-stranger`, and UID-1002 process listing were empty. Home and bundle absent at **05:54:05Z**.
- Repeated ubuntu `herdr status`, session/workspace/tab lists and process start times at 05:53:55Z. Its three original Herdr PIDs/start times, default socket, 0.9.1 version, home tab, and t-0711 identity were still unchanged. Additional parallel tabs/workspaces appeared while this ran; their content was not read and their processes were not addressed. The earlier transient ubuntu herdr-ade process was no longer listed. This remains a concurrent-activity observation, not a change made by the probe.

There were still **no system installs** to undo. The second home removal also deleted the account-local Claude package and its new, unauthenticated configuration directory.

## Token use and limits

- Throwaway test: **zero provider/model calls**; no provider credentials or logins. One Haiku-configured Claude CLI process reached first-run onboarding only; it did not perform inference.
- Reporting lane: exact input/output/cache-token totals are **not available in the exposed command environment**. Only `PI_*` variable names were inspected; no token-counter variable was present. I deliberately did not read the protected real Pi session/store to obtain a number. This is an unavailable measurement, not a claim of zero lane usage.
- Not verified: authenticated coordinator/lane operation, permission enforcement after login, handoff/compaction (P3-D9), remote lanes, merge/review/install behavior, or parent nesting on public Herdr. No new features, bypasses, compatibility paths, or project-memory edits were introduced.
