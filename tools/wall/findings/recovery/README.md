# Wall 1 — crashes and recovery

**3 confirmed defects, 2 suspicions.** Ranked below by how much unattended work
or diagnosis they cost Rolf. This is Linux local + local-SSH box testing, not Mac
or real-model acceptance. No production Rust was changed.

## Permanent regressions

The fixed findings now run in the sandbox gate:

- R01 / D37: `tools/wall/regressions/D37/repro`
- R03 / D38: `tools/wall/regressions/D38/repro`
- R02 / D39: `tools/wall/regressions/D39/repro`

They share `tools/wall/regressions/support.py`. Each requires `--instance N`,
resets only that instance, prints EXPECTED/ACTUAL and exits nonzero unless the
recovery is established. Optional `--evidence DIR` selects a new host capture;
otherwise captures go under `~/.cache/herdr-wall-regressions/`. Run only in an
available numbered instance with the candidate installed, for example:

```sh
tools/wall/regressions/D37/repro --instance 3
tools/wall/regressions/D38/repro --instance 3
tools/wall/regressions/D39/repro --instance 3
sudo tools/wall/wall --instance 3 reset
```

The gate discovers these alongside D27 and D30, with isolated workers on its
locked instances 6–8 while prove runs on locked instance 5.
The shared helper wakes the existing tickers while awaiting setup/recovery,
using their normal `.ticker.wake` signal; it never edits lifecycle records or
changes the clock/backoff algorithm. D38 leaves the ticker's normal cadence
alone during the outage/backoff assertion, then wakes it after restoration.
D39 still observes 35 seconds without an automatic restart. D37 retains its
40-second broken-build window but stops early on positive working evidence.
The rest of this document is the original historical wall-1 campaign: its
build, default-instance commands, evidence names and unfixed observations are
not instructions to run the permanent regressions on the default instance.

## Historical build and reproduction

All confirmed runs used this lane's own build:

- Commit: `301bdd4a96e778a59179fab9d76c65bc350738b8`.
- `ha --version`: `herdr-ade 0.1.0+301bdd4.1791085392` on both accounts.
- Installed, stripped `herdr-ade` SHA-256:
  `e7fa371d8560c129817f96262b6b9948a9994e39fa4174ade90f4877dcdbe4c2`.
- Built with `cargo build --bins`, installed using this checkout's
  `sudo tools/wall/wall install --build "$CARGO_TARGET_DIR/debug" --herdr
  /home/ubuntu/.local/bin/herdr --node /home/ubuntu/.local/bin/node --npm
  /home/ubuntu/.local/bin/npm`.
- The pre-install default actually reported `0.1.0+81e7e5f.1791081237`, not the
  older version anticipated by the brief. No faults ran against that build.

**Default instance only.** These commands omit the instance flag and work
with both v1 and the current wall controller. The two scoped fault helpers
accept `default` and the current dispatcher's `0`; they reject sandboxes 1–8.
Do not run these repros while another person is using the default sandbox.
Each executable resets both default accounts, uses only scripted agents, prints
EXPECTED/ACTUAL, captures evidence, and exits **1 for its reproduced defect**.
Setup/injection failures raise an error instead of printing `Rxx REPRODUCED`.
A corrected implementation should exit 0 when the signature is absent.

From this repository, with the above build installed:

```sh
L="$PWD/.herdr-project/adeherdr-t-0794/library"
# Historical names (removed after promotion):
# repro-01 --evidence "$L/new-r01"
# repro-02 --evidence "$L/new-r02"
# repro-03 --evidence "$L/new-r03"
# Repros leave their evidence-bearing state available for inspection.
sudo tools/wall/wall reset
```

Use a **new** evidence directory each time. `support.py` contains the fully
expanded reset/open/request/control/start commands; every command is printed.
It waits for asynchronous `ha open` to register the coordinator before calling
`guest.py open`: v1's old fallback otherwise mislabels a pending start as D27.
It also waits for a remote lane card before calling `guest.py wait`.
Neither `wall` nor `guest.py` was modified.

Evidence locations below are relative to
`.herdr-project/adeherdr-t-0794/library/` (untracked lane deliverables). Each
named evidence directory contains the controller's allowlisted `local.tar` and
`box.tar`. Text logs contain exact commands, timestamps and observations. All
captures preceded the next reset; no provider credentials were used or exported.

## 1. R01 — Remote retry acknowledges an old bootstrap and never starts

**Cost:** a killed box lane cannot resume through the advertised retry; repeated
retry just allocates another bare shell that is displayed as Working.

**Build:** `0.1.0+301bdd4.1791085392`, commit `301bdd4a96e778a59179fab9d76c65bc350738b8`.

**Exact steps:** run `repro-01` above. It resets; opens the scripted coordinator;
writes box `control.json = {"hold":["working"]}`; starts
`guest.py remote-lane`; waits for `working`; executes
`wall fault kill-process t-0001 --box`; waits for `status="failed"`; executes
`ha thread retry wall t-0001 --reason "R01 killed the remote agent"` locally;
waits for attempt 2 placement and another 40 seconds (beyond the 30-second
readiness window); reads overview, records and real box process-info.

- **Injection:** the recorded, UID/executable/cwd-verified scripted PID was killed.
  The original attempt became `failure_class="process_gone"`.
- **Expected recovery:** one replacement agent receives the frozen brief on the
  preserved checkout; otherwise a bounded, actionable startup failure.
- **Actual recovery:** a new pane/card is placed, but no agent is launched.
  A second manual retry also stalled in the exploratory run. Cancel worked and
  explicitly retained the recorded checkout; it did not complete the task.
- **Semantic error:** overview says `Working — starting (checking agent
  readiness)` indefinitely even though launch was never submitted.

**Evidence:** `repro-01.log` (EXPECTED/ACTUAL at lines 304–306),
`repro-01/local.tar`, `repro-01/box.tar`; independent first occurrence:
`working-box-stuck/`, `working-box/`, `working-box.log`, `box-cancel.log`.
Local `.herdr-ade/wall/.state/threads/t-0001.toml` after retry:

```toml
status = "open"
attempt = 2
bootstrap = "acknowledged"
prompt_pending = false
brief_submitted = false
launch_attempts = 0
startup_wait_started = "2026-10-04T04:14:05Z"
pane_id = "w3:p1"
```

The old identity still names `w2:p1`. The box lane card names attempt 2,
`w3:p1`. `herdr pane process-info --pane w3:p1` proves its only foreground
process is `/bin/bash`; `herdr agent list` is empty. Only one lane worktree
exists. The old pane is closed, so this is not a duplicate-agent explanation.

**Suspected path:** `src/threads.rs:1695–1740` (`retry_inner`) increments the
attempt without clearing `bootstrap`; `src/threads.rs:1358–1380`
(`finish_placement`) also retains it. Remote `thread_pass_observed` runs before
`launch_pass` (`src/ticker.rs:3598–3627`); the stale `acknowledged` takes the
receipt branch at `src/ticker.rs:2607–2624`, clearing `prompt_pending` before
launch. The launch claim requires that flag (`src/ticker.rs:3178–3186`).

**Smallest suggested fix:** clear attempt-scoped bootstrap/submission evidence
when selecting a replacement attempt, before placement or observation can run.
Preserve the explicit same-attempt parked-session resume path. Test the real
remote state-pass-before-launch ordering; local retries alone miss this.

**Not tested:** real pi resumption, automatic nonzero retry budgets, remote
reviewer replacement, and every provider/failure-class combination. Remote
pre-bootstrap retry **did** work when there was no old acknowledgement
(`early-box/`); local acknowledged retries also worked. This finding is narrower
than “all retries are broken.” No fix was applied.

## 2. R03 — A known SSH outage disappears from health during backoff

**Cost:** a coordinator receives a reassuring health check while the connection
is still down, and must search logs to distinguish loss of observation from work.

**Build:** `0.1.0+301bdd4.1791085392`, commit `301bdd4a96e778a59179fab9d76c65bc350738b8`.

**Exact steps:** run `repro-03`. Reset/start a box lane held at `working`; wait
for a working courier observation; run
`sudo tools/wall/faults/pause-transport --instance default 95` concurrently.
(The original capture used the equivalent `wall fault disconnect 95`.)
Wait for `.ticker.health` to record `unreachable`, then observe another ticker
beat while the injection is still active. Read overview and ticker status.
Restore transport before the evidence export (the fault's `finally` does this).

- **Injection:** only default-box SSH transport was SIGSTOPed. The script checks
  that the disconnect command is still running at the decisive assertion.
  The real courier logged a banner-exchange timeout. The lane was not killed.
- **Expected recovery:** keep the known failed-check reason and time visible
  through backoff; clear it only on new successful evidence. Do not say GONE.
- **Actual recovery:** the timeout is shown for one beat; the next skipped poll
  removes it from health. The thread never receives the failed-check evidence.
- **Semantic error:** overview says `working; last checked ...; next check
  pending`, not that the attempted check failed. The timestamp is correctly old,
  but existing failure evidence is hidden. No false process-death event occurred.

**Evidence:** `repro-03.log:203–288`, `repro-03/local.tar`,
`repro-03/box.tar`; independent occurrence in `services-box.log` / `services-box/`.
The final executable was revalidated in `validated-03-pidfd.log` and
`validated-03-pidfd/` after eliminating a v1 fault-controller race. After
transport restoration, `outage-recovered.log` / `outage-recovered/` show a fresh
05:02:56Z courier observation of the **same** working PID 1288773, attempt 1.
In `.herdr-ade/.ticker.log`, the first run records:

```text
04:28:23Z ... unreachable this tick: unreachable: Connection timed out during banner exchange
04:28:43Z ... cleared: ... unreachable this tick ...
```

The injected 95-second outage had not ended at the second observation.
`.herdr-ade/wall/.state/threads/t-0001.toml` retained:

```toml
last_state = "working"
last_observed = "2026-10-04T04:27:58Z"
observation_attempted = "2026-10-04T04:27:58Z"
observation_error = ""
failure_class = "unknown"
```

**Suspected path:** the unreachable early return at
`src/ticker.rs:1374–1382` skips `record_failed_observation` (unlike other failure
branches). `src/ticker.rs:971` clears per-tick machine views; a backed-off
machine is skipped at `src/ticker.rs:1352–1353`. The health rebuild then loses
the error although `MachineMemory.outage.last_error` still knows it. Backoff is
in `src/steps.rs:917–939` (eight skipped ticks).

**Smallest suggested fix:** persist failed observations on the unreachable
branch too, and retain known outage health while polling is deferred. Only a
successful observation should clear it; don't remove backoff or infer death.

**Not tested:** a ten-minute outage notice, physical network loss, loss of all
host connectivity, and UI renderers other than CLI overview/ticker status.
This is not a claim that dated remote snapshots must always appear Unknown.

## 3. R02 — A killed waiting agent remains “process unknown” at a live shell

**Cost:** when input becomes available, the displayed advice sends the
coordinator to investigate a healthy connection instead of offering retry.

**Build:** `0.1.0+301bdd4.1791085392`, commit `301bdd4a96e778a59179fab9d76c65bc350738b8`.

**Exact steps:** run `repro-02`. Reset with local
`control.json = {"finish":"waiting","hold":[]}`; `guest.py lane`; wait for
`seal-end` and the waiting event; `wall fault kill-process t-0001`; wait 35
seconds; query real process-info, overview and the durable records.

- **Injection:** the verified scripted agent PID was killed; the pane and
  reachable Herdr server remained. Process-info lists only `/bin/bash`.
- **Expected recovery:** preserve the waiting seal; show process absence and
  `ha thread retry ...` for when input is ready. Do **not** automatically restart
  work merely because a waiting process died.
- **Actual recovery:** the wait is retained correctly, but process absence is
  never reflected. A valid `thread prompt --text-file` refuses because there is
  no agent and recommends retry; manual retry queues a replacement.
- **Semantic error:** overview says `process unknown; check the connection
  before choosing prompt or retry` despite direct shell-only process evidence.

**Evidence:** `repro-02.log`, `repro-02/local.tar`, `repro-02/box.tar`;
independent earlier `waiting-local/` / `waiting-local.log`; the correctly formed
prompt refusal and manual retry are in `r02-recovery.log:1–2`.
The local record retains `status="open"`, `failure_class="unknown"`,
`bootstrap="acknowledged"`, and the killed PID in `identity.process`.
`.state/ops/t-0001-1-1.toml` remains sealed with `kind="waiting"`; the event's
text is `Scripted wall input needed`. Overview's exact failing advice is above.

**Suspected path:** `src/ticker.rs:2130–2131` skips death reconciliation for a
sealed attempt via `src/threads.rs:3282–3291` (appropriate for avoiding an
automatic restart, insufficient for observing liveness). The local row at
`src/threads.rs:5124–5136` reports unknown when a pane has no registered agent
without consulting the already-supported process-info probe.
`src/project_view.rs:95–102,144–178` turns that into the wrong recovery advice.

**Smallest suggested fix:** separate liveness evidence from restart eligibility.
For an identity-matched, shell-only pane, render absent while preserving the
waiting event and requiring a deliberate continuation. Keep Unknown when the
probe fails or identity cannot be verified.

**Not tested:** adopted waiting agents, opaque foreground children, PID reuse,
real pi sessions or remote process-only loss after waiting. Box **pane** loss
after waiting was tested separately and correctly retained the wait with an
absent-process retry suggestion (`waiting-box/`).

## Suspicions — not confirmed findings

1. **PID reuse may retain a dead reserved helper.** `src/ops.rs:733–741`
   checks `kill -0` only, not the helper's executable/start identity. Ordinary
   dead-helper recovery passed. No real PID-reuse injection was attempted;
   no record was rewritten to manufacture it.
2. **Unrecorded terminal creation window.** `src/threads.rs:1079–1128` creates a
   terminal before writing its ownership. A crash between the successful Herdr
   side effect and `thread::update` may leak a pane on retry. The placement
   crash tests hit `partial="worktree_add"`, not this exact interval. A future
   fault should hold the real create reply, not fabricate a Herdr response.

## Coverage and limits

“Passed” below means the observed plumbing/recovery, **not semantic acceptance
of a model's work**. This is a targeted campaign, not all 80 point/target/machine
combinations. All unmatched cells are untested.

| Kill point | Local evidence and result | Box evidence and result |
|---|---|---|
| Placement | Hard ticker kill at `partial=worktree_add`; restart preserved attempt 1 and one checkout (`early-local.log`, `early-local/`) | Same **controlling local ticker** kill during remote placement; restart completed placement (`early-box-repeat.log`, `early-box/`) |
| Launch before readiness | **Not hit.** Closest measured point: scripted `ready`, before brief submission | **Not hit.** Same distinction |
| Bootstrap boundary | Agent killed after `ready`, before brief/skill; retry ran and sealed (`early-local/`) | Same passed without stale acknowledgement (`early-box/`); interior bootstrap write not hit |
| Working | Agent kill, retry, pane kill during replacement, retry: attempt 3 sealed; one worktree/no old lane panes (`working-local/`). Server SIGKILL: unreachable correctly Unknown; rebooted server/retry queued a replacement (`server-local/`) | Agent kill: R01. Local and box ticker SIGKILL/restart preserved agent; verified same-pane `rebind` worked. Box server SIGKILL/restart plus cancel tested, but post-crash resumed work **not established** (`services-box/`) |
| Waiting | Agent SIGKILL: R02; correctly formed prompt refuses and manual retry queues (`r02-recovery.log`) | Pane close plus box ticker SIGKILL, `ha recover`/ticker start retained the wait; cancel completed without losing it (`waiting-box/`) |
| Mid-seal | Real helper SIGKILL while op reserved using `seal_delay_ms=5000`; dead reservation abandoned, `ha done` resealed and parked (`seal-local/`) | Same passed, with real branch publication and courier import (`seal-box/`) |
| Parked | Ticker then server SIGKILL/restart kept the exact done seal and parked state (`early-local/`) | Box ticker then server SIGKILL/restart kept exact seal (`early-box/`). No pane/agent exists to kill once parked |
| Reviewer mid-review | Agent kill; `ha review retry wall` reused reviewer `t-0002`, attempt 2; no duplicate reviewer (`reviewer-local/`) | Not exercised |
| Reviewer mid-verdict | Second kill hit real reserved done helper; reseal yielded one consumed honest REJECT, reviewer resolved, no merge (`reviewer-local/`) | Not exercised |
| Candidate landing to bare remote | **Not exercised:** scripted reviewer honestly rejects; no manufactured MERGE verdict | Lane-ref publication on reseal was real; it is **not** accepted-candidate landing |

Also covered: 65/95-second SSH outages with agents preserved (R03). Uncovered:
agent exec before readiness; the interior of bootstrap or the staged-op/event
atomic write; second kill in that exact write; physical reboot, Mac behavior,
reviewer-on-box, candidate fast-forward/push interruption, parked follow-up
completion, cross-pane rebind, and adopt recovery. No exhaustive pane/worktree
leak claim is made outside the recorded snapshots. Cancel intentionally kept
recorded unlanded worktrees; those are retained work, not unowned orphans.

Second failures were real: local attempt 2 lost its pane, the recovered reviewer
lost its sealing helper, remote placement recovery lost its pre-bootstrap
agent, and parked lanes lost ticker then server. Local double-retry and remote
pre-bootstrap retry eventually sealed; acknowledged remote retry did not.
No manual ADE record edits were used for recovery.

Harness-fixture limits encountered (not new ADE findings): the scripted worker
cannot recommit its identical file after a post-commit restart; hold `working`
for retry tests or reseal the already committed work directly. The v1 guest
wait helper captures the pane once, so waiting before placement/card creation
can miss it. V1's `disconnect` can abort on an exited SSH child while collecting
PIDs; `pause-transport` provides the same confined injection with pidfds and
ignores already-exited children. That failed injection is separately captured
in `validated-03-injection-failed/`, not counted as an ADE defect. Logged
exploratory setup errors are not counted as injections. D21–D31 and the missing
clock were not re-reported.

## Gates and finish

The pinned Rust gates and wall Python suite pass (769 Rust tests passed,
1 pre-existing ignored; 8 Python tests passed); detailed results are in the
lane report/library. Final replay logs `validated-01.log`, `validated-02.log`,
and `validated-03-pidfd.log` each contain their REPRODUCED marker and exit 1.
Negative repro exits are intentional, not gate failures.
Runtime evidence stays untracked. Only these findings/utilities and the
default-instance-scoped `tools/wall/faults/crash-service` and
`tools/wall/faults/pause-transport` are repository changes.
The default sandbox is reset again after evidence capture before sealing.
