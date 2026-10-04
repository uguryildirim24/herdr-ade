# Permanent live regressions

A defect found live gets `<defect>/repro` here when the wall can express it.
Run on the trusted oci host: `tools/wall/regressions/D30/repro --instance 3`
when instance 3 is available. Never run a repro on an occupied instance.
Instances **5–8 and 9–12 are reserved for the gate**, as independent sets.
The first slot in the acquired set runs prove and the final fixture journey;
three regression workers own the remaining slots. Additional repros queue on
those workers, never sharing a slot concurrently. Manual use of a reserved
slot must hold `/home/ubuntu/.cache/herdr-wall-gate/instance-N.lock`.
The gate acquires a complete set before building or changing any instance,
releasing partial claims before trying the other set. Two campaigns can run
at once; both sets busy makes a lane exit 75 without mutation.
Each executable resets only the passed instance, prints EXPECTED and ACTUAL,
and returns nonzero while the defect exists. Repros need an installed build.
The gate discovers every `*/repro`; a missing/non-executable/broken repro cannot
silently pass. D27 disallows guest.py's historical assisted-open fallback. D30
corrupts an actual started lane, checking garbage, truncation and empty records.

## Gate contract

Run `tools/wall/gate` in the review worktree. This is a **trusted host gate**, like
`cargo test`, not a lane command or a new lane boundary exception. It builds the
candidate with its existing Cargo target directory; creates the candidate's
shared hash-addressed stage once and attaches it to the acquired set (5–8 or
9–12). Each slot has private writable guest images. It runs all repros on the
last three slots in parallel with every prove fault on the first, then the
Linux scripted fixture journey on the first; captures evidence
and resets all four slots in parallel. A failed worker cancels the other host
process groups and waits for their exit before capture/reset. It reinstalls the candidate after prove's mixed-build fault,
so the final journey runs the candidate, not the alternate fault image. Use the canonical gate command `tools/wall/gate` (or
`./tools/wall/gate`) when declaring it. It is now declared on every herdr-ade
review. The review path selector always retains a declared wall gate for changes under
`src/`, `assets/`, `mods/`, or `tools/wall/`, regardless of its paths allowlist.

`--candidate /worktree` can exercise main or a scratch revert using the current
host tools. `--alternate-build /build/debug` supplies an existing different ADE
image for prove's mixed-build install fault (default: oci's installed build).
An absent or identical alternate is INCOMPLETE, never a pretend install proof.
Pinned pi is reused across ADE binary stages; reset is offline.
`--slot-set 9` restricts development runs to 9–12 without touching busy 5–8.

Exactly one `WALL GATE PASS`, `WALL GATE FAIL: <round>: <reason>` or
`WALL GATE INCOMPLETE: <reason>` line is emitted, with an evidence path and
elapsed time on separate lines. Failure and incompleteness exit nonzero.
Contention exits **75** with `gate instance busy`: ADE retries the same sealed
candidate automatically, without rejecting code or requiring another seal.
ADE reuses the other gates' passing receipts only for the same seal, candidate
and exact selection, validating their environment and full log hashes again.
A changed candidate/selection reruns everything; the wall gate always reruns.
ADE marks its wall invocation `ADE_WALL_REVIEW=1`. The gate creates a
flock-held pending review reservation beside the slot locks and waits for up
to 1500 seconds. While pending, lane invocations yield with exit 75 without
claiming slots. A process exit/crash releases the reservation; the scheduler
removes stale entries. Acquiring a set removes the pending reservation, so
lanes can use the other free set while the review runs.
The final wall line appears in the REVIEW notice. Other projects still select
their own gates; this gate is declared on every herdr-ade review.

The **1500-second campaign budget** reserves 90 seconds for failure capture/reset.
Review admission has a separate 1500-second wait budget; ADE gives the wall
command 3300 seconds for both, while other gates retain 1800 seconds. Parallel cleanup uses
per-slot capture/reset limits inside that reservation, leaving time to publish
the result. A command timeout kills its host process group, including privileged
controllers, then captures the sandbox before resetting it.
`command-durations.jsonl` records every command's elapsed duration and result;
`round-durations.jsonl` records each complete regression, prove and fixture round;
`prove/round-durations.tsv` records each individual fault duration. Assignment
is recorded in `regression-instances.json`; `stage-N.log` records the shared
stage attached to each slot. Commands, exact build IDs, all prove captures,
repro output, pre-delete journey records and before/after ubuntu identities live
under `/home/ubuntu/.cache/herdr-wall-gate/wall-gate-*`, outside git and all reset
paths. `slots.json` records the set, PID, review status and UTC start time.
The gate never resets or installs instances outside its acquired set.

The two declared non-failures are prove's clock finding (no injectable clock)
and the Claude trust probe (Mac only). Guest findings about assisted coordinator
starts or repaired provider configuration fail the gate rather than masking a
production defect. The scripted reviewer verifies exact
fixture files and their sole plumbing criterion before issuing MERGE. It is not
a model reviewer or evidence of arbitrary semantic acceptance. The ordinary
scripted fault reviewer still REJECTs; only the gate's explicit fixture control
selects this narrowly scoped check. The loop verifies both seals/courier, both
reviewed files on the local bare remote, and real project deletion.
