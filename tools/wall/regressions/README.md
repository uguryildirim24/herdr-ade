# Permanent live regressions

A defect found live gets `<defect>/repro` here when the wall can express it.
Run on the trusted oci host: `tools/wall/regressions/D30/repro --instance 5`.
Never run a repro on an occupied instance. Instance 5 is reserved for the gate;
manual use must hold `/home/ubuntu/.cache/herdr-wall-gate/instance-5.lock`.
Each executable resets only the passed instance, prints EXPECTED and ACTUAL,
and returns nonzero while the defect exists. Repros need an installed build.
The gate discovers every `*/repro`; a missing/non-executable/broken repro cannot
silently pass. D27 disallows guest.py's historical assisted-open fallback. D30
corrupts an actual started lane, checking garbage, truncation and empty records.

## Gate contract

Run `tools/wall/gate` in the review worktree. This is a **trusted host gate**, like
`cargo test`, not a lane command or a new lane boundary exception. It builds the
candidate with its existing Cargo target directory; installs only instance 5;
runs all repros, every prove fault and a Linux scripted fixture journey; captures
evidence and resets. It reinstalls the candidate after prove's mixed-build fault,
so the final journey runs the candidate, not the alternate fault image. Use the canonical gate command `tools/wall/gate` (or
`./tools/wall/gate`) when declaring it. Do not declare it until it is on main.
The review path selector always retains a declared wall gate for changes under
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
The final wall line appears in the REVIEW notice. No gate is implicitly added
to projects; the coordinator declares it after landing.

The 900-second total budget reserves 90 seconds for failure capture/reset. A
command timeout kills its host process group, including privileged controllers,
then captures the sandbox before resetting it. Commands, exact build ID, all
prove captures, repro output, pre-delete journey records and before/after ubuntu
identities live under `/home/ubuntu/.cache/herdr-wall-gate/wall-gate-*`, outside
git and all reset paths. The gate never resets or installs other instances.

The two declared non-failures are prove's clock finding (no injectable clock)
and the Claude trust probe (Mac only). Guest findings about assisted coordinator
starts or repaired provider configuration fail the gate rather than masking a
production defect. The scripted reviewer verifies exact
fixture files and their sole plumbing criterion before issuing MERGE. It is not
a model reviewer or evidence of arbitrary semantic acceptance. The ordinary
scripted fault reviewer still REJECTs; only the gate's explicit fixture control
selects this narrowly scoped check. The loop verifies both seals/courier, both
reviewed files on the local bare remote, and real project deletion.
