# Wall 6 — state surfaces

**1 confirmed finding (two failure scenarios), 3 unpromoted suspicions.**
Linux instance **1**, scripted agents, no production Rust changes. The required
single-lane new-project journey passed unassisted through deletion. This is
fixture acceptance, not real-model review or arbitrary semantic acceptance.

## Build, evidence, and running this work

- Base commit: `b3202a1cd04bf8575dde07c7d2c9bed1197cf355`.
- Installed `ha --version`: `herdr-ade 0.1.0+b3202a1.1791109477`.
- Installed stripped ADE SHA-256:
  `e3c790a9cfd4fc5a1a1c7fcdebb54d1ac73869b5e39f2f2241250d3b0340b3d1`.
- Installed Rundown SHA-256:
  `5621959e87b6cccd1cc4d3e291229411b2e92b30974f1eb5a7241ceed6f281fd`.
- Built with `cargo build --bins`; installed with
  `sudo tools/wall/wall --instance 1 install --build "$CARGO_TARGET_DIR/debug"`.
  Initial sandbox version was `0.1.0+b29e2a9.1791104643`; no failures were
  injected until the lane's own base build was installed.
- Evidence paths below are relative to the untracked lane library:
  `.herdr-project/adeherdr-t-0831/library/`. Archives are the controller's
  allowlisted `local.tar` and `box.tar`, captured before the next reset.

Run only on a free, explicitly assigned instance, with this build installed:

```sh
tools/wall/findings/surfaces/repro-01 --instance 1 --evidence /absolute/new/S01
# Exit 1 reproduces S01; setup failures raise rather than pretend to prove it.
# Each scenario starts with a full selected-instance reset.
sudo tools/wall/wall --instance 1 reset
```

`run-scenario --instance N --evidence /absolute/new/dir SCENARIO` runs one of
`corrupt-task`, `corrupt-plan`, `corrupt-event`, `kill-process`, `ticker`, `fill`,
`disconnect`, `reboot`, or `journey`. It captures all requested surfaces plus
records, then exports evidence. It is an observation driver, not a pass/fail
oracle; only `repro-01` asserts the finding. `snapshot.py` and `journey.py` run
inside the sandbox as ordinary unprivileged files. No new faults were needed;
`wall` and `guest.py` were not edited. No provider calls or throwaway panes.

## 1. S01 — Rundown hides unreadable task and seal evidence

**Rank / cost:** highest finding here. Rolf's card looks like ordinary unfinished
work while the coordinator's commands say the task or its completion evidence
cannot be read. Someone must inspect a different surface to discover why work
cannot progress. This is a new path beyond D30: corrupt **lane** records already
produce the correct Rundown diagnostic, but corrupt **task/event** records do not.

**Build:** the version, commit and hashes above, same in both independent runs
of each scenario.

**Exact steps:** `repro-01 --instance 1` runs these two sequences:

1. Reset instance 1; `guest.py lane` uses real `ha open`, prompt hook and
   `ha thread start`. Wait for `t-0001` at `before-seal`.
2. `ha plan set wall --does 'Deliver the scratch fixture'`;
   `ha plan step add wall 'Land fixture' --task job-0001`.
3. Capture the intact surfaces.
4. Task case: `wall --instance 1 fault corrupt .state/tasks/job-0001.toml garbage`.
5. Event case (fresh reset): release `before-seal`, wait for the actual done
   event, capture the finished-but-not-reviewed state, then
   `wall --instance 1 fault corrupt .state/events/t-0001-1-1.toml garbage`.
6. Read overview (human/JSON), context (`--peek --full`), handoff, plan show,
   task list/show, doctor, and `herdr-rundown --print`; export evidence.

The executable discovers the event filename from actual records, not a guessed
ID. Both failures use the built-in corruption operation.

**Injection:** established. The corresponding real file contains
`{invalid wall record`; task list/show exits 1 in both cases. The event case
first establishes that a real seal existed. No historical fixture was invented.

**Expected recovery/visibility:** the harness need not reconstruct damaged
records. It must retain the uncertainty on Rundown, as it already does for D30:
show unreadable records instead of an ordinary task card. The existing overview
`read_error` channel and Rundown's nonzero unavailable exit should carry this.

**Actual recovery:** no automatic repair occurred or was expected. CLI paths
correctly stopped trusting the damaged evidence. In both cases Rundown still
exited **0**, omitted the unreadable-record warning, and displayed:

```text
Wall
... 0 of 1
Deliver the scratch fixture
▐◌▌  Land fixture
```

**Semantic result:** wrong visibility, not false completed work. The bound step
changed from `running` to `left`; JSON overview returned `read_error = ""`.
For the task case, its same `actions` contains `Unreadable task: task_unreadable`.
For the seal case, overview/context/handoff say `repair the unreadable event
evidence`, while plan show and task list/show fail with `event records are
unreadable`. Rundown discards those action diagnostics. Doctor exited 0 in both
cases; that command currently checks setup/bindings, not all evidence integrity,
so its scope alone is not claimed as a second defect.

**Evidence / exact locations:**

- `repro-01.log:1–24`: EXPECTED/ACTUAL and the two successful-looking cards;
  executable exit **1**.
- `repro-01/corrupt-task/during.log:30–35`: identified task parse failure;
  line 75: empty `read_error`; lines 242–258: task list/show exit 1;
  lines 311–319: Rundown exit 0.
- `repro-01/corrupt-event/during.log:70`: empty `read_error`;
  lines 219–229: plan and task commands refuse unreadable events;
  lines 282–290: Rundown exit 0.
- `repro-01/{corrupt-task,corrupt-event}/capture/{local,box}.tar` retains the
  record files, plan bindings, notices and ticker evidence.
- Independent first observations: `{corrupt-task-run,corrupt-event-run}/` with
  `before.log`, `during.log`, event `sealed.log`, and `capture/` archives.

**Believed cause:** `src/project_view.rs:353–354` counts only unreadable lanes;
`src/project_view.rs:708–714` puts task errors in section rows;
`src/project_view.rs:944–948` sets Rundown `read_error` solely from
`unreadable_lanes`. Event readability lives in `src/task.rs:1101–1127`, but is
not included in that Rundown diagnostic. `src/rundown/view.rs:115–128` consumes
plan/activity/read-error/needs-you fields, not the diagnostic `actions` list.

**Smallest suggested fix:** propagate task and seal-read errors through the
existing shared read-error channel, keeping readable rows. Preserve the
already-correct lane-corruption handling. Do not add another process-health
probe, new switch or replacement record format. The fixing lane can promote
this repro into the permanent regression gate after making it pass; this lane
only delivers the requested findings and intentionally failing repro.

**Not tested:** review-record corruption, arbitrary combinations of damaged
records, permission-denied reads, running TUI cache invalidation, or Mac output.
Rundown `--print` uses the real installed binary, not a renderer fixture.

## Suspicions / unpromoted observations (3)

1. **Two-machine sandbox remote can need a permission repair.** The extra
   local+box `guest.gate_loop()` journey reached reviewer MERGE, but publication
   failed writing an existing bare object directory owned by the box account:
   `unable to write file ./objects/41/5171c7cb470973a1ce02f9a95217bbf0b714d3:
   Permission denied`. `journey-run.log` records the failure;
   `journey-failed-surface.log` records all surfaces and directory ownership;
   `journey-failed/{local,box}.tar` preserves state. This is a shared local-bare
   fixture permissions issue, not evidence of a production Git/SSH defect.
   No production conclusion or deterministic repro claimed. The full gate's
   later two-machine journey passed; the required single-lane journey also
   passed. Shared repository creation mode is worth examining in a tools lane.
2. **Fresh no-agent evidence after box reboot still says Unknown.**
   `reboot-run/during.log:16–31,288` shows overview Unknown / task working but
   doctor says the recorded agent binding is gone. The courier successfully
   recorded an empty `last_state`, `last_group="unknown"`, and fresh
   `last_observed`, with no current observation error. No retained process
   identity was independently checked, and the scenario did not wait for all
   later recovery passes; uncertainty may be intentional. Not promoted.
3. **Plan corruption points at event repair.** `corrupt-plan-run/during.log`
   shows task list/show saying `repair the unreadable event evidence` although
   only `plan.toml` was damaged and no done event existed. JSON/context/handoff
   and Rundown identify the plan error correctly. `EvidenceSnapshot::load`
   combines binding-history and event readability. The misleading remediation
   deserves a narrower follow-up, but no separate executable assertion or
   recovery journey was established here.

The failed-lane task label `[working]` and plan `running` are **not** counted as a
fourth finding: those are lifecycle/started labels (`Mark::Started`), not current
process evidence. On process loss the explicit next action is retry/cancel and
Rundown removes "Working on now". No false done count was observed.

## Coverage and outcomes

Every captured scenario read **overview, JSON overview, context --peek --full,
handoff, plan show, task list/show, doctor, and Rundown --print**. Records and
notice-batch contents were read alongside them. Surface reads were sequential,
not claimed as one atomic snapshot.

| Scenario / evidence | Injection | Recovery / semantic observation |
| --- | --- | --- |
| Clean held lane; `before-corrupt.log` | Real started lane, before seal | Working, zero done; no inappropriate Unknown. |
| Corrupt lane; `corrupt-thread.log`, `corrupt-thread/` | Built-in garbage corruption | Overview/context/handoff identify exact unreadable lane; task asks repair; Rundown warns and exits 1. D30 behavior retained. |
| Corrupt task; `corrupt-task-run/`, repro | Established | S01; other CLI surfaces retain parse diagnostic. |
| Corrupt event; `corrupt-event-run/`, repro | Established after a real seal | S01; no false accepted/merged state. |
| Corrupt plan; `corrupt-plan-run/` | Established | Plan show and Rundown refuse; context/handoff retain path. Remediation suspicion above. |
| Kill local process; `killed-process.log`, `killed-process/` | UID/executable-checked kill | Needs attention/process gone, configured zero retries exhausted; doctor fails; GONE notice durably submitted. No false seal. |
| Ticker restart; `ticker-run/` | Real ticker stop/start | Same live lane retained, no false completion or unexpected Unknown. |
| Full disk; `fill-run/` | Confined ENOSPC | Doctor fails both shared-filesystem disk checks; after removing fill file doctor recovers. Held lane never sealed, so sealing-at-ENOSPC untested. |
| SSH loss; `disconnect-run/` | 95-second transport pause | Unknown with last successful/failed check times; doctor retains outage, no GONE. After restore overview returns Working and observation_error clears. The known recovery R03 health-loss path did not recur. |
| Box reboot; `reboot-run/` | Cgroup killed; disk retained | No false done; current Unknown/Gone discrepancy left as suspicion. |
| Single-lane journey; `journey-single.log`, `journey-single/capture/` | Fresh reset calls real ha new | Open, first lane, independent exact-fixture review, bare-remote landing, archive and delete all succeed without assistance. |

**Journey details:** `journey-single.log:172–473` distinguishes finished from
reviewed (`0 of 1`, running plan step); lines 644–950 establish exact reviewed
file on bare `main`, task merged, and `1 of 1`. The landing SHA was
`0630c119d9665d3037ea54d9f09e487aa7e5e605`. Archive refuses reopen and preserves
the done step; lines 1388–1422 end with the project absent. Local archive
`gate-loop-before-delete.tar` retains the project after archive and before
delete. Installation was not required or claimed. Goal closure by a real
coordinator/model is outside this fixture journey.

**Notices/outbox:** process loss retains a GONE notice and submitted flag;
successful landing retains `REVIEW review-1 merged ... publication verified;
install not required` in the durable notice batch (`journey-single.log:1340–1356`).
Submission means outbox acceptance, not coordinator consumption. Context was
read with `--peek`; end-to-end idle wake/receipt timing and draft-held delivery
were **not** established. No empty notice titles were observed or re-reported.

**Uncovered / unavailable:** mixed-build surface comparison, kill-pane versus
kill-process, mid-seal kill, corruption of reviews/inbox/outbox, long-draft
holds, waiting-on-Rolf resolution, dropped/withdrawn acceptance, full-disk
sealing, non-default remote session, all other lanes' scenario cross-products,
real pi delivery/provider semantics, Mac, physical reboot, real network
partition, and injectable time. Existing D21–D62 were not re-reported as new
findings. The full gate's broader probes are not counted as this surface matrix.

## Gates and cleanup

Passed `cargo fmt --check`, `cargo test -q`,
`cargo clippy -q --all-targets -- -D warnings`,
`python3 -m unittest discover -s tools/wall -p 'test_*.py'` (26 tests), and
`git diff --check`. Logs live in the library. The S01 repro intentionally exits 1.

The coordinator clarified that the full wall gate is **not required** for a
findings lane. One busy attempt returned 75 without acquiring instance 5; the
already-started second invocation finished with `WALL GATE PASS` at
`/home/ubuntu/.cache/herdr-wall-gate/wall-gate-8b88jplx` (756.053s). No further
full gate runs were made.

Instance 1 was left at a clean reset with the pinned build: `reset-final.log`
and `clean-reset-proof.log` show no lanes, seals, reviews or tasks, and only the
baseline project state. No changes to production code, shared fault tools,
project memory, authentication or runtime installs outside the sandbox.
