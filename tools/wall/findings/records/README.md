# Records, storage and concurrency findings

This historical report describes the builds below, not current test results. External evidence archives are not included. See [host requirements](../../README.md#host-requirements) before using the reproducers.

**3 confirmed findings; 2 separate suspicions.** No production Rust changes.
These are Linux crash-consistency checks on our own disposable default sandbox.
`R01`–`R03` below are local to **records**, not the recovery lane's numbering.
The coordinator assigns global defect numbers.

## Build, scope and reproduction

Every confirmed run used the already-installed lane base, checked before any
failure scenario:

- Commit `b29e2a93d7fe623ba7157d8e5eb3ed7427044a95`.
- `ha --version`: `herdr-ade 0.1.0+b29e2a9.1791104647`.
- Sandbox `herdr-ade` SHA-256:
  `28c7f98b4cdf728a10a4ebdf2d2c4f8c9428250fbca4111562fc29e71ae8e8a9`.

Only the **default instance** was used for these findings. Scripts also accept
`--instance N`; never select an occupied instance. Instance 5 belongs exclusively
to the wall gate. Each `repro-NN` resets its selected instance, uses real ADE and
Herdr binaries with scripted agents, prints EXPECTED/ACTUAL, captures evidence,
and exits **1 while its finding exists**. Setup failure is not a reproduction:
require the explicit `RNN reproduced` line. Repros leave the observed state for
inspection; reset afterwards. No provider/model acceptance is claimed.

```sh
L="/path/to/evidence"
tools/wall/findings/records/repro-01 --evidence "$L/new-plan"
tools/wall/findings/records/repro-02 --evidence "$L/new-inbox"
tools/wall/findings/records/repro-03 --evidence "$L/new-cancel"
sudo tools/wall/wall reset
```

Use a new evidence directory each run. `support.py` records commands and writes
plain readable guest scripts into that directory, transfers them with `wall
enter`, then runs them by path. The new `faults/interrupted-write` helper accepts
only an existing regular record resolving beneath the selected sandbox's
`wall/.state`. It applies empty, half-length or unreadable contents; it does not
change `wall`, `guest.py`, or production writers.

All evidence paths below are relative to the untracked lane library above.
Each `capture/` contains the controller's `local.tar` and `box.tar`; commands and
observations are in `commands.log` and the named outer `.log`. Captures precede
resets. This discovery lane supplies findings, not fixes; permanent regression
promotion belongs with the fixes, so intentionally failing repros are not added
to the passing wall gate's discovery set.

## 1. R03 — Full storage reports a cancelled lane that is still running

**Cost:** Rolf or the coordinator believes work has stopped, but the cancellation
was never recorded and the process remains active. There is no durable pending
cleanup request for the ticker to finish.

**Build:** the exact base/version/hash above.

**Exact steps:** run `repro-03`. Reset; start a scripted lane held at `working`;
stop the sandbox ticker; snapshot its thread record in memory; use the existing
confined `fill` helper; run `ha thread cancel wall t-0001 --reason
"Storage-full cancellation check"`. Read the record; release the fill file;
query the original pane's process-info; export evidence.

- **Condition established:** the tmpfs reached ENOSPC. Cancellation's subsequent
  task-page/plan-page writes also explicitly returned OS error 28.
- **Expected recovery:** return nonzero if the initial durable cancellation
  cannot be written. Report cancellation success only after that transition is
  durable; later pane cleanup may separately remain pending.
- **Actual recovery:** exit 0 and `t-0001 cancelled; pane cleanup_pending and
  worktree kept`. The thread record is byte-for-byte unchanged:
  `status = "open"`, `cancellation_reason = ""`, `cleanup_pending = false`, no
  retirement request. The original scripted pi PID remains foreground.
- **Semantic result:** unsuccessful cancellation is presented as successful
  cancellation with a cleanup tail. Atomic file replacement itself preserved
  the old record correctly.

**Evidence:** `repro-03.log:168–173,272`, `repro-03/capture/local.tar`;
`.state/threads/t-0001.toml`; process-info shows PID `1074269`, pane `w1:p3`,
cwd `/home/wall/repo/.worktrees/t-0001`. Independently observed during
`storage-full-2.log:288–291,385–388` on `t-0003`.

**Suspected cause:** `src/threads.rs:1903–1942` catches *every* `retire` failure
at line 1932, including failure of `begin_retirement` (`2768`, called at `2819`
or `2935`). `retirement_failure` at `3047` produces a pending outcome without
persisting it. `src/cli.rs:1740–1752` still renders cancellation success.

**Smallest suggested fix:** propagate failure of the initial durable transition.
Only convert failures *after* recorded cancellation into cleanup-pending success;
render from the actual durable state. Preserve the existing retryable cleanup
behavior after a successfully recorded cancellation.

**Not tested:** automatic recovery after a real power loss, remote cancellation,
all retirement authorities, or real pi accepting further work. No claim of a
half-written thread record.

## 2. R01 — An empty plan silently turns completed progress into zero

**Cost:** completed work disappears from the plan and done counts without an
unreadable-record signal, precisely the class of regression n-0085 warns about.

**Build:** the exact base/version/hash above.

**Exact steps:** run `repro-01`. Reset; route the scripted reviewer; start one
fixture lane; author a plan and bind `s-1` to its real `job-0001`; allow the
fixture to seal, receive its narrowly scoped scripted review and publish to the
sandbox's bare remote. Stop the ticker. Assert `plan show --json` contains one
**done** step. Apply `wall fault interrupted-write plan.toml empty`; read the
same command again and export evidence.

- **Condition established:** the existing plan became zero bytes; its tasks,
  lane, review and publication evidence stayed intact.
- **Expected recovery:** report an unreadable existing plan, not a normal empty
  card. Preserve the distinction between a missing plan and damaged evidence.
- **Actual recovery:** exit 0, `outcome = "shown"`, `schema = 0`, `revision = 0`,
  `present = true`, `steps = []`. The successful card's done/total count falls
  **1/1 -> 0/0** with no diagnostic about the unreadable plan.
- **Semantic result:** the read path converts loss of recorded progress to a
  successful zero-work projection. It does not reconstruct the lost steps.

**Evidence:** `repro-01.log:174–216`, `repro-01/capture/local.tar`, especially
`.state/plan.toml`, `.state/reviews/review-1.toml`, and the retained `job-0001`
and seal records. `matrix-final/commands.log:811–834` independently observes
empty-plan success; unreadable text and half-length TOML fail explicitly.

**Suspected cause:** `src/contracts.rs:455–469` defaults every Plan field;
`src/plan.rs:57–68` parses an empty TOML document into that default without
read-time schema validation. `show` (`549–566`) treats `Some(default)` as present;
`counts` (`528–539`) then returns zero/zero.

**Smallest suggested fix:** validate the required identity/schema of an existing
plan on read; distinguish a missing path from empty/truncated valid-TOML input.
Retain supported historical outcome fields rather than rejecting old data.

**Not tested:** every syntactically valid partial TOML boundary, automatic
reconstruction of a plan, Mac Rundown rendering, or an actual install changing
counts. The initial done state is established by the real fixture journey, not
an authored `state = "done"` fixture; its review is not arbitrary model acceptance.

## 3. R02 — Unreadable inbox notices disappear as an empty inbox

**Cost:** a coordinator silently loses the notice explaining why unattended work
is waiting, leaving Rolf to discover the blockage himself.

**Build:** the exact base/version/hash above.

**Exact steps:** run `repro-02`. Reset and start the scripted fixture plus held
reviewer. Age only the sandbox's existing coordinator-input-hold timestamp so
the real ticker writes one `coordinator-draft` inbox item. Stop the ticker and
verify that item via `ha inbox list wall --json`. Restore its original bytes
before each condition: unreadable text, half-length, empty. Read `inbox list`
after each condition and export evidence after the last.

- **Condition established:** all three changes were applied to the same real
  ticker-authored `.state/inbox/<timestamp>-coordinator-draft-w1-p1-1.md`.
- **Expected recovery:** retain an unreadable-item row or fail explicitly with
  the file path. Do not equate an unreadable existing notice with no notices.
- **Actual recovery:** each command exits 0, `outcome = "succeeded"`,
  `message = "no unhandled inbox items\n"`, `items = []`, with no warning.
- **Semantic result:** an existing unhandled notice disappears from the
  coordinator's inbox surface; no repair or acknowledgement was performed.

**Evidence:** `repro-02.log:201–226`, `repro-02/capture/local.tar`, and
`matrix-final/commands.log:1073–1130`. The actual notice id is recorded in each
capture and log; it naturally changes across resets.

**Suspected cause:** `src/inbox.rs:35–43` returns `None` for bad frontmatter;
`unhandled` at `225–237` discards read and parse failures with `filter_map`.

**Smallest suggested fix:** carry read/parse errors alongside successfully read
items and display them on the list/context surfaces. Keep the deliberate
filtering of explicitly retired historical projection kinds.

**Not tested:** an event-linked inbox item's replay after its file becomes
unreadable, archive pruning, directory permission failures, or all consumers of
`unhandled`. The reproduction uses a genuine draft notice, not a hand-authored
inbox fixture.

## Separate suspicions — not confirmed findings

1. **Outbox read failure may discard queued obligations.**
   `src/steps.rs:618,659,734` uses a default/empty batch after `read_json` fails.
   The matrix applied all three conditions to `notice-batch.json`, but context
   does not prove the transport consumed that batch. Some setups needed an
   explicitly labelled synthetic Transition fixture because no live batch
   remained. Lost delivery and rewrite after restart are **not established**.
2. **Unreadable coordinator identity may be replaced as though absent.**
   `src/project.rs:546–563,598–600` erases parse errors into `None`, then an
   update defaults the record. The matrix shows no explicit coordinator-record
   warning and a live reviewer labelled session-unreachable. Duplicate launch,
   binding replacement and loss of closed-by-Rolf state are **not established**;
   the separate installation `records_load` check does reject this file.

Notes are not counted as a new defect: their context rendering says no current
facts, but the structured command includes a warning naming the unreadable note.
Reviews explicitly retain a `review-error` row. D30's lane-error row stays visible.
D52 remains known: review/drop returned a dropped task with retirement incomplete
because it was already in review. It is not re-reported here.

## Coverage and limits

### Record reads — 27 condition/surface pairs completed

`matrix --evidence <new-dir>` regenerates the fixtures from reset. Final evidence:
`matrix-final/commands.log`, `matrix-final/capture/`. It restores each record
before moving to the next condition; this is a read matrix, not a recovery proof.

| Kind | Real surface | Unreadable / half-length / empty observations |
|---|---|---|
| Threads | overview | All three keep an explicit Unknown lane-error row (D30 preserved). |
| Tasks | task list | All three return failure rather than an empty task set. |
| Reviews | overview | All three expose review read error; no false done claim. |
| Events | task show | All three fail with unreadable evidence. |
| Plan | plan show | First two fail; empty succeeds with zero steps (R01). |
| Notes | context --peek | All three warn with the note path; current facts are omitted. |
| coordinator.json | overview | All three lack an explicit record warning; see suspicion. |
| Inbox | inbox list | All three return successful empty list (R02). |
| Outbox | context --peek | All three applied, but transport recovery remains untested. |

### Storage full — partial writer coverage, no universal atomicity claim

`storage-full --evidence <new-dir>`; decisive run `storage-full-2.log:275–388`
and its capture. Fill uses the actual bounded tmpfs and is repeated before each
command. Existing record bytes remained unchanged and no new final/temp record
remained at the snapshot. A successful plan write after releasing space proved
that storage recovery permitted progress.

- **Reached ENOSPC at target:** plan replacement, new task, review replacement,
  new immutable note, coordinator replacement. Each returned failure and retained
  the old records. With 4096 bytes released for a 24 KiB note, the note publication
  still failed without leaving a partial final file.
- **Thread cancellation:** falsely succeeds without changing its record (R03).
- **Event sealing:** failed first at `.state/ops/t-0002-1-1.toml`; actual event
  publication under ENOSPC is **not established**.
- **Inbox:** `context` was not the bound coordinator, so its success does not
  establish an inbox writer or acknowledgement write under ENOSPC.
- **Outbox:** no enqueue/flush write reached under ENOSPC. **Uncovered.**
- Source review found the shared flushed temporary-file/rename writer in
  `src/project.rs:102–164` and create-only publication at `175–197`; source
  inspection alone does not prove every caller's live crash consistency.
- No interrupted process during fsync/rename, real power loss, inode exhaustion,
  or filesystem other than tmpfs was exercised.

### Concurrency — partial interleaving coverage

`concurrency --evidence <new-dir>`; `concurrency.log`, its capture:

- 8 simultaneous plan adds, 4 task adds, 4 note adds: every command succeeded;
  all 16 additions retained unique records/steps. No lost update or deadlock.
- Start/cancel: both completed and cancellation persisted, but start had already
  returned when its record was found (`overlap observed False`). A true overlap
  at that boundary is **not established**.
- Done/prompt: real sealing command delayed only in real git calls; overlap
  observed, both returned 0, seal and lane records captured. No deadlock. This
  scripted run does **not** establish real pi's eventual follow-up delivery or
  acceptance; D26 is not re-claimed as fixed.
- Review/drop: simultaneous commands both returned; review was already active.
  Review creation racing drop is **not established**. D52 behavior was observed.
- No broad schedule exploration, remote simultaneous writers, or lock-owner
  process interruption was attempted.

### Historical formats — representative, not exhaustive

`historical --evidence <new-dir>`; `historical.log`, its capture:

- Real lane plus obsolete PR-era fields still loads with identity/title intact.
- Plan with historical `kind`/`what_you_get`, without `does`, retains outcome and
  steps through `plan show`.
- Historical notes JSONL retains a complete note with an interrupted second row;
  context explicitly warns about the interrupted journal.
- Historical task, review, event/delivery, coordinator, inbox and outbox format
  variants were **not exercised live**. Existing gate/unit coverage is not
  presented as live historical-format coverage.

Unavailable scope: Mac behavior, physical storage/power-loss durability, real
provider/model acceptance, and a shared controllable clock. The aged draft marker
is a fixture timestamp, not a simulated clock or a clock-skew proof.
