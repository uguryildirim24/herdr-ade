# The wall on oci

A disposable live ADE harness, not a unit-test fake. No production Rust behavior
is changed. The host controller needs oci's existing `sudo`; agents do not get it.

## Instances

The omitted flag preserves the original default: `wall`/`wallbox`, `/home/wall`,
ports 22285/22286, units `herdr-wall-PORT`, and saved machine `wall-box`.
**Do not reset/install/fault the default while another lane uses it.**

Every command accepts `--instance N` before its verb, N = 1..8.

- **1–4:** manually assigned wall-lane slots; use only your assigned instance.
- **5–8:** reserved together for `tools/wall/gate` on every herdr-ade review.
  Instance 5 runs prove and the final fixture journey; independent regression
  workers use 6, 7 and 8 concurrently. The gate holds one nonblocking lock per
  slot (`/home/ubuntu/.cache/herdr-wall-gate/instance-N.lock`). Any occupied lock
  yields the same retryable `gate instance busy` exit 75 before build/install.
  Never use a reserved slot manually without holding its corresponding lock.

Instance N uses
`wallN`/`wallboxN`, `/home/wall-N` (box beneath it), ports `22285+2*N` and
`22286+2*N`, units `herdr-wall-N-PORT`, and saved machine `wall-box-N`. Each has
its own root/socket, remote, mounts, private tmp, cgroups, controls and releases.
The limits below are unchanged per instance. Homes are hidden across instances;
UIDs prevent cross-instance signals. Thread IDs are resolved **only** in the
selected account's records, even if another instance has the same short ID.

```bash
cargo build --bins
sudo tools/wall/wall --instance 1 install --build "${CARGO_TARGET_DIR:-target}/debug"
sudo tools/wall/wall --instance 1 reset
sudo tools/wall/wall --instance 1 enter
sudo tools/wall/wall list
sudo tools/wall/wall --instance 1 prove /tmp/new-proof-1 /absolute/alternate-build
```

Install stages the explicitly selected branch build once, keyed by binary
hashes, in root-owned `/var/lib/herdr-wall-builds`. Instances share this
read-only stage, with private writable executable copies for fault injection.
No old v1 stage is reused. Install another instance with the same `--build` to
reuse the stage; a later `install --build NEW` refreshes only its selected
instance. `list` inventories all nine slots, build version/commit, processes,
tmpfs use and last reset. An untouched v1 default has no reset/build metadata;
its existing version is read without installing or stopping it.

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
commit is deterministic. **Reset retains the one shared sandbox login.**

Inside, `HOME=/home/wall`, root is `$HOME/.herdr-ade`, socket is
`$HOME/.config/herdr/herdr.sock`; `ha`, `herdr`, `pi`, node and npm are sandbox
binaries. `sudo tools/wall/wall enter --box` enters `wallbox`, the second machine
at `/home/wall/box`. A real saved Herdr machine named `wall-box` uses local SSH
on 127.0.0.1:22286. Local entry uses port 22285. Neither port is publicly bound.

The two SSH services have separate cgroups and mount namespaces. Host homes
(including ubuntu and root) and host `/run` sockets are hidden; host filesystems
are read-only. The selected home is exposed writable: a **2 GiB tmpfs**, containing
both roots, logs, repositories, bare remotes and worktrees. `/tmp`, `/var/tmp`,
`/dev/shm` and `/run` have separately bounded private tmpfs mounts. Each cgroup
has a 2 GiB memory limit and 512-task limit. Numbered instances also put both
services under a dedicated `herdrwallN.slice` with **aggregate** 2 GiB memory
and 512 tasks, so the two accounts cannot double the instance budget. The
untouched default keeps its original cgroups. Accounts have their private
group and `wall-auth`, no sudo, and cannot signal ubuntu or other instance
processes. System tools/libraries and
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

On older builds with D27, `ha open` records an invalid-timeout failure. The helper
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
provider login in any installed instance enables local and box `wall_real` lanes
(configured Astra/max; confirm the model ID in the installed catalog after login).
Credentials live in `/var/lib/herdr-wall-auth/auth.json`, outside every home
and reset path. Its directory is root:`wall-auth` 2770; the file is 0660. Only
sandbox users belong to that group, never ubuntu. Nothing is copied from
ubuntu or another machine. All freshly installed local and box accounts link
their private pi `auth.json` to this one file, so login and token refresh are
shared. The privileged controller opens the shared directory/file without
following symlinks and holds descriptors for metadata changes and logout;
non-regular or multiply linked auth entries are refused, not followed into the
host. Normal token refresh can still replace the regular file. The pinned
sandbox pi package resolves auth locks to the shared file
(two `realpath` options), preventing different account symlinks from bypassing
each other's refresh locks. Settings, hooks, models and sessions remain private.

Use any numbered instance for the one `/login openai-codex`. Reset preserves it;
`sudo tools/wall/wall logout` clears it **for all instances**. The already-running
v1 default is deliberately not migrated or restarted: its active mount namespace
and private login stay as they are until its lane finishes and a later explicit
install refreshes it. Its accounts are included in `wall-auth` without stopping
any process. Scripted lanes need no provider login.

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

Additional faults are executable files in `tools/wall/faults/<name>`. The
controller dispatches them with `--instance` and optional `--box` after applying
standard timing options. See [faults/README.md](faults/README.md) for the contract.

## Evidence and findings

```bash
sudo tools/wall/wall evidence /absolute/new/evidence-directory
sudo tools/wall/wall --instance 1 prove /absolute/new/proof-directory /absolute/alternate-build-directory
python3 -m unittest discover -s tools/wall -p 'test_*.py' -v
sudo tools/wall/prove-isolation --instance 3 --peer 4 /absolute/new/isolation-proof
```

`prove-isolation` resets only its two **explicitly numbered** slots. It tests
cross-reset/reboot/disconnect, foreign thread refusal, hidden homes and UID
signal denial, plus real pi auth-backend writes across accounts and reset.
It briefly adds a clearly synthetic auth fixture (not a provider credential),
checks evidence exclusion and removes it without removing any existing login.
Both slots finish clean; do not run it on slots occupied by wall lanes.

`evidence` exports `local.tar`/`box.tar`: project records/artifacts, `.ticker.*`,
Herdr/server logs, scripted runs and control. It deliberately excludes provider
auth, SSH keys, npm caches, pi sessions and the fill file. Capture **before
reset**, to a host path outside the full filesystem. Inspect evidence before
sharing; any log may contain an agent's text. Fresh installs create an empty shared auth file only if none exists.
For a stuck/disconnected server, restore transport first; host service logs are
`sudo journalctl -u herdr-wall-22285 -u herdr-wall-22286`.

`prove` accepts the same instance flag and runs each command from that instance's own clean reset, records exact commands/effects
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
