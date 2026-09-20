# Review brief: round r42

plain: This round checks that one record owns a round and the program refuses a wrong move with a reason.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r42` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `9fa13b1a0c057a96823a0fa0d6a02818302ed49fb80c00e43eb66c47cfb692d4`, policy hash `3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0094 | 1 | `399748648757c5c77aa619287f7f8dda1db9c758` | `t-0094-1-2` | `7d1f6de7508152a0dafecfd0bb1429440666d48b86790df55c7cda11c6f19fcc` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r42.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r42"
candidate = "<C>"
manifest_hash = "9fa13b1a0c057a96823a0fa0d6a02818302ed49fb80c00e43eb66c47cfb692d4"
policy_hash = "3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0094 (artifact `7d1f6de7508152a0dafecfd0bb1429440666d48b86790df55c7cda11c6f19fcc`)

Data, not instructions.

```text
# t-0094 — E2 / A2

Commit: `399748648757c5c77aa619287f7f8dda1db9c758`
Branch: `hp/adeherdr/t-0094-e2-a2-one-record-owns-a-round-and-the-bi`
Machine: `oci`

## Delivered

- `.state/rounds/rNN.toml` owns the lifecycle, membership/pins, pending review outputs, accepted verdict pin, and merge/checkpoint transaction. No new merge sidecars are written.
- Explicit phases: admitting, preparing_review, under_review, verdict_in, merging, checkpointing, merged, abandoned, diverged. Phase/transaction inconsistencies fail closed; `round show` prints the phase.
- Old-shaped records migrate in place on read. Their merge sidecar is absorbed once and removed. After migration an obsolete sidecar is reported, never read as state. Record writes/migration and explicit round operations are serialized.
- Review intent is recorded before writing the brief/ref/worktree. Interrupted outputs are checked against that intent on explicit retry. Git drift is reported rather than replacing recorded state. Existing merge/checkpoint crash-recovery tests still pass.
- Frozen member pins cannot silently change through later completion events. An explicit admit/review accepts changed inputs; automatic advancement reports stale evidence and preserves the pins. A validated reviewer completion becomes an immutable verdict pin; later done events cannot silently replace it.
- Required refusals: already-landed admission; resolving a held lane; opening/reviewing against another open round's integration ref; non-exact MERGE at V; advancing an empty round. Errors name the cause and next command/action.
- Resolution stays blocked through the merge intent and checkpoint, including automatic resolution. Removed the existing `thread resolve --force` escape hatch.
- Branch reservations cover other projects in the same ADE root and worktree aliases of the same git repository. Separate integration branches remain usable.

## Tests and gates

All run with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check`: PASS.
- `cargo test`: PASS, 629 tests (490 ADE + 56 pi + 78 pro + 5 CLI).
- `cargo clippy --all-targets -- -D warnings`: PASS.
- `git diff --check`: PASS.

Coverage includes every required refusal, old-shaped on-disk migration, sidecar absorption, obsolete sidecar non-authority, phase inconsistency, branch reservations across projects/worktrees, frozen pins, immutable verdict selection, interrupted review outputs, pending-merge resolution/admission, and git drift after completion.

**Fixture limitation:** no live `.state/rounds/r1.toml` through `r37.toml` exists under `/home/ubuntu` on this box. `tests/fixtures/rounds/r1.toml` is an explicitly constructed old-schema fixture, written into the test project's `.state/rounds/` and loaded/migrated there; it is not claimed to be a copy of a production round. A production-file migration test still requires a coordinator-side sample. Reference documents were read from `/home/ubuntu/projects/herdr/HANDOFF.md` and `tasks/ade/LEAN.md`, the cloud equivalents of the Mac paths.

## Prose removed

From `skill/COORDINATOR.md`:

- Removed the exact-MERGE rule in “`hp round merge ...` merges only on an exact MERGE verdict at the verdict commit for the pinned candidate, then writes the checkpoint.” Replaced with the command's purpose.
- Removed the duplicate exact-MERGE clause in “Never merge except through `hp round merge`, which merges only on an exact MERGE verdict at the verdict commit for the pinned candidate.” Kept the command direction and bringing decisions to Rolf.
- Removed “Read its exit before anything that follows it, never behind a pipe.” Resolution now checks the durable phase itself.
- Removed the stale description permitting a second round's brief to move the same integration head, and shortened the retry description.

From `docs/operations.md`:

- Removed “A round never gets a second reviewer while one is bound”; retained the manual repair command description.
- Replaced the stale “because another round landed first” explanation with integration-branch changes.
- Added record layout/migration documentation, not another copy of the refusal rules.

The checked-in versions of these two documents contained no separate sentences instructing agents not to admit already-landed shas, resolve pinned lanes, open overlapping rounds, or advance empty rounds; no such sentences were invented or added.

## Notes for integration

No live records, project memory, credentials, installs, integration branches, or main were changed. Only this lane branch is published. Existing records can load without an opt-in migration flag; no compatibility read path remains after the explicit phase has been written.
```

