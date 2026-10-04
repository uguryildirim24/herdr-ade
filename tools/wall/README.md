# The wall on oci

A disposable live ADE harness, not a unit-test fake. No production Rust behavior
is changed. The host controller needs oci's existing `sudo`; agents do not get it.

## Install, reset, enter

From the checked-out harness on oci:

```bash
cargo build --bins
sudo tools/wall/wall install \
  --build "${CARGO_TARGET_DIR:-target}/debug" \
  --herdr /home/ubuntu/.local/bin/herdr \
  --node /home/ubuntu/.local/bin/node \
  --npm /home/ubuntu/.local/bin/npm
sudo tools/wall/wall reset
sudo tools/wall/wall enter
```

`install` stages **only** the explicitly named binaries, npm executable package,
a fresh pinned pi download and these tools. It never imports ubuntu's npm
configuration, provider credentials or SSH identity. It generates new sandbox
SSH identities. `reset` is offline, stops only wall-owned processes/cgroups,
wipes/remounts the scratch disk, and rebuilds both accounts. The baseline git
commit is deterministic. **Reset also removes sandbox provider logins.**

Inside, `HOME=/home/wall`, root is `$HOME/.herdr-ade`, socket is
`$HOME/.config/herdr/herdr.sock`; `ha`, `herdr`, `pi`, node and npm are sandbox
binaries. `sudo tools/wall/wall enter --box` enters `wallbox`, the second machine
at `/home/wall/box`. A real saved Herdr machine named `wall-box` uses local SSH
on 127.0.0.1:22286. Local entry uses port 22285. Neither port is publicly bound.

The two SSH services have separate cgroups and mount namespaces. Host homes
(including ubuntu and root) and host `/run` sockets are hidden; host filesystems
are read-only. Only `/home/wall` is exposed writable: a **2 GiB tmpfs**, containing
both roots, logs, repositories, bare remotes and worktrees. `/tmp`, `/var/tmp`,
`/dev/shm` and `/run` have separately bounded private tmpfs mounts. Each cgroup
has a 2 GiB memory limit and 512-task limit. Accounts have only their private
group, no sudo, and cannot signal ubuntu processes. System tools/libraries and
DNS configuration are read-only; outbound networking remains available for pi.
This is filesystem/UID/resource isolation, not a VM or network-denial firewall.
Never give a wall lane the host controller's sudo access **inside** the sandbox.

## Start a scripted lane

Host commands (substitute your checkout's `tools/wall/wall` path throughout):

```bash
sudo tools/wall/wall enter 'python3 "$HOME/tools/guest.py" lane'
sudo tools/wall/wall enter 'python3 "$HOME/tools/guest.py" remote-lane'
```

The helper opens the scripted coordinator, records a test request through the
real prompt hook, then uses ordinary `ha thread start`. IDs come from records,
not invented panes. The remote variant exercises provisioning, lane cards,
branch publication and courier over real SSH against the shared **local bare
remote** `/home/wall/remote.git`, never GitHub. All repos are scratch fixtures.

On builds with D27, `ha open` records an invalid-timeout failure. The helper
prints `FINDING D27` and explicitly starts that recorded coordinator with Herdr's
valid 30000 ms timeout, then reopens it. This sandbox-only bootstrap preserves
the failure evidence; it is not a production fix or claim that open succeeded
unassisted. The sandbox registers only the production Rundown binary as a local
plugin, with no install/build/startup action.

Edit `$HOME/control.json` **before** starting a lane:

```json
{"delay_ms":500,"hold":["before-seal"],"fail_at":"","finish":"done","seal_delay_ms":0}
```

- `delay_ms`: delay at each checkpoint.
- `hold`: checkpoint names (`working`, `mid-review`, `before-seal`). Release with
  `touch "$HOME/release/<phase>"`. Files/control are per sandbox account.
- `fail_at`: exit 42 at that checkpoint, allowing real gone-process recovery.
- `finish`: `done` or `waiting` (the latter calls real `ha waiting`).
- `seal_delay_ms`: delay each **real git** invocation inside `ha done`; use 5000
  to keep the real sealing helper alive for a deterministic mid-seal kill.
  Normal git calls and production lanes are not delayed.

`scripted-agent.js` reads the frozen brief, reports pi-shaped lifecycle through
Herdr's hook API, commits a scratch change, writes the report and invokes the
real sealing verb. It logs phases/PIDs under `$HOME/runs/`. The sandbox pi wrapper
routes **only explicit `--wall-*` flags** to this stand-in, whose foreground name
is `pi` for Herdr's process check; ordinary arguments run pinned real pi through
the production wrapper/hook/guard. It is **not** a simulation of pi's TUI,
provider/session semantics or idle-only delivery. A scripted reviewer reaches
`mid-review` and seals an honest REJECT; it cannot establish semantic acceptance.
Do not use it to claim a real-model review or successful landing.

## Real pi: one login

Rolf enters the local sandbox, runs `pi`, types `/login openai-codex`, and selects
**ChatGPT Plus/Pro (Codex)**, completing its browser/device flow. That single
provider login **as `wall`** enables local `wall_real` lanes (configured Astra/max;
confirm the model ID in the installed catalog after login). Credentials remain
in wall's own pi agent folder. Nothing is copied from ubuntu or another machine.
Real remote model lanes additionally need the same provider login **as
`wallbox`**; scripted remote lanes need no login. Reset wipes both logins.

Use a new task file and ordinary `ha thread start wall --recipe wall_real` with
an existing request (`$HOME/request` from the helper), exact acceptance and the
scratch repo. Readiness uses `herdr-pi check`, not the scripted OK probe, for
provider-backed arguments. No real provider/model call has been claimed without
Rolf's login. Real pi with its installed hooks is required for D26-style delivery.

Fresh setup also exposed a provider-config defect: with no DeepSeek recipe it
emits `opencode-go = {modelOverrides: {}}`, which pi rejects as invalid model
state even for Codex. Reset logs that finding and removes **only that exact
empty declaration** from this Codex-only sandbox; nonempty providers are kept.
This is sandbox configuration, not a fix to production setup. The native
readiness check must then say credentials are missing, not `invalid_state`.

## Faults: one command, sandbox-only targets

```bash
sudo tools/wall/wall fault kill-pane t-0001 --when t-0001 before-seal
sudo tools/wall/wall fault kill-process t-0001                 # scripted process
sudo tools/wall/wall fault kill-process t-0001 seal --when t-0001 seal-helper
sudo tools/wall/wall fault ticker --when t-0001 before-seal    # stop + restart
sudo tools/wall/wall fault disconnect 5                     # box SSH outage
sudo tools/wall/wall fault reboot 2                         # box cgroup reboot
sudo tools/wall/wall fault fill                             # real confined ENOSPC
sudo tools/wall/wall fault corrupt .state/threads/t-0001.toml truncate
sudo tools/wall/wall fault corrupt .state/threads/t-0001.toml garbage
sudo tools/wall/wall fault clock 3600
sudo tools/wall/wall fault install /absolute/alternate-build-directory
```

`--box` targets the second account's pane/process/ticker/records/disk/build.
`--after <seconds>` delays injection; `--when <thread> <scripted-phase>` waits up
to 60 seconds for that phase before the delay. For `disconnect`/`reboot` the
affected machine is always the box; add `--box --when ...` to watch its lane.
`--when` is a recorded boundary, not proof of being inside an atomic operation;
use `seal-helper` plus `seal_delay_ms` for a genuine mid-seal kill. Real-model
review/landing moments use `--after` and the sandbox's actual records/logs, not
scripted checkpoint claims.

- Pane IDs are resolved from the chosen lane's records on its own socket.
  Process kills resolve a recorded scripted PID or sealing-helper PID and verify
  UID, executable and checkout; arbitrary host PIDs are not accepted.
- Disconnect pauses only SSH transport processes from the box cgroup, including
  existing connections. Box herdr/ticker/agents keep running; connections time
  out. A `finally` resumes transport. Reboot kills **all** box cgroup descendants,
  including detached processes, then starts new herdr/ticker with disk retained.
  This is a logical machine reboot, not an oci/kernel reboot.
- Fill refuses a non-tmpfs target. Remove `$HOME/disk-full` to recover; zero-byte
  file creation can still succeed at ENOSPC, so test an actual write.
- Corruption accepts only existing regular files resolved beneath this sandbox's
  `wall` project. Traversal/outside symlinks/directories are rejected.
- Clock prints a **finding**, not a fake success: ADE has no shared injectable
  clock (`project::now` and direct `jiff::Timestamp::now` calls; Runner deadlines
  separately use monotonic time). Host time is never changed. Clock skew is not
  testable in v1 without a future shared clock boundary.
- Install streams the named build over sandbox SSH and atomically swaps
  **only sandbox herdr-ade** as the unprivileged sandbox account, then replaces
  that root's ticker. Root never writes through sandbox-controlled symlinks.
  It deliberately creates a mixed-build window with existing agents,
  pi guard and helper binaries, as D24 needs. Pick a genuinely different build;
  log versions and hashes before/after. It never calls production `ha harness`.

## Evidence and findings

```bash
sudo tools/wall/wall evidence /absolute/new/evidence-directory
sudo tools/wall/prove /absolute/new/proof-directory /absolute/alternate-build-directory
python3 -m unittest discover -s tools/wall -p 'test_*.py' -v
```

`evidence` exports `local.tar`/`box.tar`: project records/artifacts, `.ticker.*`,
Herdr/server logs, scripted runs and control. It deliberately excludes provider
auth, SSH keys, npm caches, pi sessions and the fill file. Capture **before
reset**, to a host path outside the full filesystem. Inspect evidence before
sharing; any log may contain an agent's text. The fresh pi state has no credentials.
For a stuck/disconnected server, restore transport first; host service logs are
`sudo journalctl -u herdr-wall-22285 -u herdr-wall-22286`.

`prove` runs each command from its own clean reset, records exact commands/effects
and read-only before/after listings of ubuntu's root/socket/processes. It finishes
with a clean reset. `WALL_PROVE_FAULTS='...'` can resume selected rounds. Normal
ubuntu scheduler activity and concurrent installations can change ticker times
and processes; a byte-identical live root is not claimed. The relevant proof is
hidden homes/sockets, UID signal denial, confined writes, stable real server
identities and no host-mutating fault target.

Each finding must include: build IDs/hashes; exact reset/start/control/fault
steps and target machine; expected versus actual; minimal reproducer; original
request/criterion if relevant; record file + fields or log path + exact lines;
and which portions are untested. Separate injection success from recovery or
semantic correctness. Carry evidence forward before another lane resets.

**v1 can test:** Linux local/remote start/seal/courier/recovery, process loss,
SSH loss, logical box reboot, ticker interruption, disk exhaustion, corrupt
records, mixed builds, and (after login) real pi behavior. **It cannot establish:**
clock-skew behavior, Mac ticker/`ha open`/handoff faults, physical reboot or
network partition beyond this local SSH transport, or real model acceptance
without login. Mac-side journeys belong to the separate post-install lane.
