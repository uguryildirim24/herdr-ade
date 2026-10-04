# Permanent live regressions

A defect found live gets `<defect>/repro` here when the wall can express it.
Run on the trusted oci host: `tools/wall/regressions/D30/repro --instance 3`
when instance 3 is available. Never run a repro on an occupied instance.
Instances **5–8 are reserved for the gate**: 5 runs prove and the final fixture
journey; three independent regression workers own 6, 7 and 8. Additional repros
queue on those workers, never sharing a slot concurrently. Manual use of a
reserved slot must hold `/home/ubuntu/.cache/herdr-wall-gate/instance-N.lock`.
The gate acquires all four locks before building or changing any instance;
if any is busy it releases the locks it acquired and exits 75 without mutation.
Each executable resets only the passed instance, prints EXPECTED and ACTUAL,
and returns nonzero while the defect exists. Repros need an installed build.
The gate discovers every `*/repro`; a missing/non-executable/broken repro cannot
silently pass. D27 disallows guest.py's historical assisted-open fallback. D30
corrupts an actual started lane, checking garbage, truncation and empty records.

## Gate contract

Run `tools/wall/gate` in the review worktree. This is a **trusted host gate**, like
`cargo test`, not a lane command or a new lane boundary exception. It builds the
candidate with its existing Cargo target directory; creates the candidate's
shared hash-addressed stage once and attaches it to instances 5–8. Each slot has
private writable guest images. It runs all repros on 6–8 in parallel with every
prove fault on 5, then the Linux scripted fixture journey on 5; captures evidence
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

Exactly one `WALL GATE PASS`, `WALL GATE FAIL: <round>: <reason>` or
`WALL GATE INCOMPLETE: <reason>` line is emitted, with an evidence path and
elapsed time on separate lines. Failure and incompleteness exit nonzero.
Contention exits **75** with `gate instance busy`: ADE retries the same sealed
candidate automatically, without rejecting code or requiring another seal.
The final wall line appears in the REVIEW notice. Other projects still select
their own gates; this gate is declared on every herdr-ade review.

The **1500-second total budget** reserves 90 seconds for failure capture/reset,
below the review harness's 1800-second command timeout. Parallel cleanup uses
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
paths. The gate never resets or installs instances outside 5–8.

The two declared non-failures are prove's clock finding (no injectable clock)
and the Claude trust probe (Mac only). Guest findings about assisted coordinator
starts or repaired provider configuration fail the gate rather than masking a
production defect. The scripted reviewer verifies exact
fixture files and their sole plumbing criterion before issuing MERGE. It is not
a model reviewer or evidence of arbitrary semantic acceptance. The ordinary
scripted fault reviewer still REJECTs; only the gate's explicit fixture control
selects this narrowly scoped check. The loop verifies both seals/courier, both
reviewed files on the local bare remote, and real project deletion.
