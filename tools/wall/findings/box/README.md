# Box resilience findings

This historical report describes the builds below, not current test results. External evidence archives are not included. See [host requirements](../../README.md#host-requirements) before using the reproducers.

Two confirmed findings, ranked by the interruption to Rolf's unattended work.
One separate suspicion. No production changes. Finding IDs B01/B02 are local
identifiers, not allocations in the D-series.

## Environment and replay

All manual scenarios used **instance 1**, local `wall1` and box `wallbox1`,
with saved machine `wall-box-1`. This is Linux-to-Linux over real local SSH,
not a Mac or a physical box restart. Agents are scripted; ADE, Herdr, Git,
sealing, courier and the local bare remote are real.

Candidate installed from this lane's base:

- Commit `b29e2a93d7fe623ba7157d8e5eb3ed7427044a95`.
- `herdr-ade 0.1.0+b29e2a9.1791104643`.
- ADE SHA256 `12813cb5fc28daf5374419bbfa46c92938acc55f1b73c2d3bf4a4838c01d50b9`.

Mixed-version alternate:

- Commit `245269e9b6cfd4bf041299988fe083ac02a24652`.
- `herdr-ade 0.1.0+245269e.1791094525`.
- ADE SHA256 `dc18aaf3bba19e19264f7a9eae2b278665931b1a4227dcf0ae03930e0c11f38f`.
- Existing staged build:
  `/var/lib/herdr-wall-builds/00e56e520ff9b25acd71bbc9e43408a2b9a4eacd631f79228af84c8243806405/bin`.

Evidence paths below are relative to the preserved lane library:
`/path/to/evidence/`. Every named evidence directory
contains `local.tar` and `box.tar`, captured before its instance was reset.
The adjacent logs retain commands, versions and observations. They are runtime
deliverables, intentionally not committed.

Run from the repository on the prepared Linux host; the scripts invoke the existing controller:

```sh
tools/wall/findings/box/repro-02 --instance 1 --evidence /absolute/new/b02
tools/wall/findings/box/repro-01 --instance 1 \
  --alternate-build /absolute/different-commit/build/bin \
  --evidence /absolute/new/b01
```

Each resets the selected instance first, prints EXPECTED and ACTUAL, and exits
1 when the defect is present. Never run on an occupied instance. B01 requires
a genuinely different build; reset restores the installed candidate. A failure
to establish the scenario is an assertion/error, not a successful reproduction.
Both executable repros returned 1 with their `REPRODUCED` marker in the resumed
attempt. They leave evidence available; reset after inspection.

## 1. B02 — Connection loss at start leaves a task with no recoverable lane

**Cost:** after connectivity recovers, the requested work still never starts.
The coordinator must reconstruct and submit the start; there is no lane to retry.
This is a new path to D24's job-without-lane symptom: matching builds and an SSH
outage, not version skew. D24's deferred-version placement does not cover it.

**Build:** candidate above, on both sides. No mixed-version rollout in this repro.

**Exact steps:** `repro-02` resets, opens the sandbox project, sets the box's
scripted agent to wait at `working`, then runs `wall fault disconnect 65`.
One second later it calls ordinary `ha thread start wall` with the sandbox
repository, task file, `wall_lane` recipe, recorded request, acceptance and
explicit `--machine wall-box-1`. After the outage completes it verifies box
`ha --version`, allows 35 seconds of ticker observation, and reads tasks/lanes.
Those seconds describe the observation window, not an acceptance deadline.

**EXPECTED:** preserve the requested start as a recoverable attempt on that
machine and resume it after connectivity returns.

**ACTUAL:** start returns 1 with `unreachable: Connection timed out during banner
exchange`. `job-0001` survives with `attempts = []`, but there are zero lane
records. Overview says `None` under current work and `start an attempt` for the
open task. Reconnection succeeds; no start resumes.

**Evidence:** `repro-02-resumed.log`, especially its EXPECTED/ACTUAL block;
`repro-02-resumed/local.tar`, member
`.herdr-ade/wall/.state/tasks/job-0001.toml`, and the absence of a lane member.
The ACTUAL block retains request `q-1791108104430-862185-0`, acceptance
`Scripted fault plumbing observed`, `attempts: []` and `lanes: []`.
Earlier independent observation: `start-outage.log` and `start-outage/`.

**Separate conclusions:** simulated network outage succeeded (start timed out
while the controller was still pausing transport); box connectivity recovered;
start-intent recovery failed. The empty task remains visible, so this is not a
claim that the original request vanished.

**Likely path:** `src/cli.rs:1641` creates/resolves the task before calling start.
`src/threads.rs:290` resolves placement before allocation. Its deferred branch
handles provider readiness and `version_skew:` only; `src/threads.rs:332`
returns an unreachable error before `thread::allocate` at line 428. There is
therefore no persisted attempt for the ticker to recover.

**Smallest suggested fix:** persist a pinned, unplaced attempt for transient
connection loss, using the same deferred-start lifecycle as version skew, but
retaining the lost-connection classification. Keep request/task/recipe/machine
and frozen inputs linked so recovery does not need a second start command.

**Not tested:** every outage length during placement, connection loss after each
individual provisioning step, default-machine fallback, or concurrent starts.
No claim about Mac SSH behavior or arbitrary provider-backed work.

## 2. B01 — Deferred placement starts work on a held machine

**Cost:** maintenance holds cannot be trusted: a lane begins work on the box
while the coordinator expects it to remain unused.

**Build:** candidate local; alternate box to defer placement, then candidate box
again. All versions and hashes are in `repro-01-resumed.log`.

**Exact steps:** `repro-01` resets, opens the project, sets box control to wait at
`working`, installs the alternate on the box only, and starts a remote lane.
It checks that version skew left `provider_wait_started` set with no checkout
or pane. It runs `ha machine hold wall-box-1`, restores the candidate on the box
with `fault install /var/lib/herdr-wall-1/bin --box`, then observes placement
and verifies the hold file for that exact saved machine is still present.

**EXPECTED:** no new checkout, pane or agent until `ha machine release`.
Already-running work need not be interrupted by a hold; this lane has never
been placed, so that exception does not apply.

**ACTUAL:** the machine remains held but attempt 1 gets checkout
`/home/wall-1/box/repo/.worktrees/t-0001`, pane `w2:p1`,
`brief_submitted = true`, and reaches the scripted `working` checkpoint.
The hold file is `cc3ad3e140ce7f09b8ff60495ea90fb6.hold` in the replay.

**Evidence:** `repro-01-resumed.log:185-255` records the unplaced state, hold,
build restoration, fresh placement and ACTUAL block. `repro-01-resumed/local.tar`
contains `.herdr-ade/wall/.state/threads/t-0001.toml`; `box.tar` retains scripted
run evidence. The root-level hold directory is not part of the standard
archive, so the log explicitly reads its filenames. Earlier replay:
`repro-01.log` and `repro-01/`.

**Separate conclusions:** mixed-version deferral and the machine hold both
succeeded; D24 recovery successfully started the lane once builds matched;
that recovery was semantically wrong because it ignored the active hold.

**Likely path:** fresh placement checks `project::machine_held` at
`src/threads.rs:554`. `src/ticker.rs:1542` probes and resumes deferred starts;
`src/threads.rs:659` (`resume_provider_start`) reloads the remote profile and
calls `place_recovery` (`src/threads.rs:1425`) without rechecking the hold.
Readiness becoming true is not equivalent to the machine being released.

**Smallest suggested fix:** respect the existing machine hold at deferred
placement, keep the same pending attempt and resume it on release. Do not
spend launch retries or move the lane to another machine while held.

**Not tested:** the provider-readiness variant of this same path, rename of a
held machine, a hold concurrent with already-started provisioning, or a
successful release path after a future fix. The repro includes the last check
when the lane correctly remains unplaced.

## Separate suspicion (not a confirmed new finding)

**S01 — Mixed-version local install can leave its previous ticker running.**
`mixed-local-replacement.log` shows alternate CLI version `245269e` but ticker
version `b29e2a9`, PID 538309, after the local install operation. A second
`ticker start` produced alternate ticker PID 549963
(`mixed-local-ticker-retry.log`). `mixed-local-replacement/` captures the state.
This may be a replacement race, an incomplete controller operation, or overlap
with known D50. There is no reset-first deterministic repro or new defect
allocation. Do not count this as a successful mixed-version ticker window until
both CLI and running ticker identities have been observed.

## Coverage and boundaries

| Scenario | Scope | Measured result / evidence |
| --- | --- | --- |
| Working lane, 1/65/180-second simulated network outages | Complete for those three sampled durations | Same live box process and attempt 1 after reconnect; one seal and published immutable ref after release. `disconnect-{1,65,180}-rerun.log`, `disconnect-{1,65,180}/`. |
| Start during 65-second outage | Complete for explicit-machine pre-placement path | B02; no recoverable lane. |
| Before-seal through sealing/courier, 65-second outage | Complete for sampled window | Box seals during outage; courier later imports one event and parks the lane. `seal-outage-65.log`, `seal-outage-65/`. Event SHA and bare ref both `c6c1d11fa4d47beaafed9cf19fa1c74147f5fdf8`. |
| Logical box restart during work | Complete for sampled checkpoint and manual retry | Unsealed lane becomes GONE, not alive; configured zero automatic retries respected. Ordinary retry starts attempt 2 and it seals/parks. `reboot-work.log`, `reboot-work/`, `reboot-work-recovered/`. |
| Logical box restart during seal | Complete for helper start and reserved operation | No false done seal. Reserved op `t-0001-1-1` becomes `abandoned`; local lane GONE. `reboot-seal/`, `reboot-seal-reserved.log`, `reboot-seal-reserved/`. |
| Restart after courier imported seal | Complete for sampled checkpoint | One retained seal and immutable ref; finished task not restarted. `after-courier-restart.log`, `after-courier-restart/`. |
| Restart strictly inside courier import | Uncovered | No claim that a checkpoint after import exercises an atomic import interruption. |
| Box-only mixed-version rollout | Partial | Existing lane seals; start and courier progress observed, matching builds resume work, review starts. `mixed-box.log`, `mixed-box-{skew,matched}/`. B01 isolates the deferred-placement path. |
| Local-only mixed-version rollout | Partial | CLI and ticker mismatch observed (S01); explicit ticker replacement establishes alternate ticker; queued start and seals progress after matching builds. `mixed-local-continue.log`, `mixed-local-{skew,matched}/`. |
| Reviews across skew | Partial | Scripted REJECT retained in `review-skew/local.tar` (`review-1.toml`); next review starts while box runs alternate. Not evidence of real-model acceptance or a successful merge/install. |
| Held machines | Complete for B01 path only | A hold made after deferred allocation is ignored. |
| Courier freshness/duplication | Partial | Outage errors clear; sampled seals import once. Working lanes remained alive; unsealed restarted-box lanes became GONE. No exhaustive event ordering or session matrix; D54 is already known. |

Uncovered: the full phase × duration cross-product (including 1/180-second
outages during sealing, review and provisioning), sub-step timing races,
repeated/flapping outages, physical reboot, real Mac launchd/SSH/handoff,
remote Git hosting outages, clock skew, arbitrary version/protocol combinations,
and provider-backed semantic review. Publishing here uses a local bare remote;
SSH transport loss does not make that filesystem unavailable. The immutable
`seals/.../<sha>` ref is the publication proof; the initial `hp/...` branch can
remain at its frozen base by design.

Known D24, D50 and D54 are not re-reported as new findings. A sampled recovery is
not proof that the entire Mac-to-box journey is clear of defects. The most useful
follow-up is to fix and replay B02/B01, then cover precise courier interruption
and the remaining phase/duration cells.
