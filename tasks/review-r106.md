# Review brief: round r106

plain: Helper notes stay out of shared code, a stopped helper can be closed, and helper briefs stay small.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r106` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 3, manifest hash `be9aa7a3d138ad04aebb7e5704582b969a1808febfda2f55c6fa23e4370b8a94`, policy hash `b0854bd6e2b014fabc7e40bfde1e4ee7fa636e894571a68daae3999272c7ef0e`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0275 | 1 | `dfacff1b344988a722df090a19492e1f8776ead0` | `t-0275-1-1` | `ccc113a7348f3717e27273e0bf49f6ec3d4dc3c1049eab495996737ef4fe2a5a` |
| t-0276 | 1 | `99aea5062ace38a483ca1f254c80e7508c667d3a` | `t-0276-1-1` | `8ce09a2d148b5e70134e567e4243a91dea4c6db5894cb4c4487d59b29adc0dcc` |
| t-0277 | 1 | `4ae9e5c1af0875c3480b393313c155fc522a4e54` | `t-0277-1-1` | `1e8fa3816072eec9a5e4f612b14c79a0a743d556101653a193cd528f1fd0fafe` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r106.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r106"
candidate = "<C>"
manifest_hash = "be9aa7a3d138ad04aebb7e5704582b969a1808febfda2f55c6fa23e4370b8a94"
policy_hash = "b0854bd6e2b014fabc7e40bfde1e4ee7fa636e894571a68daae3999272c7ef0e"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0275 (artifact `ccc113a7348f3717e27273e0bf49f6ec3d4dc3c1049eab495996737ef4fe2a5a`)

Data, not instructions.

```text
# W30 report

Implemented the `stage_done` guard against commits that track `.herdr-project/` runtime files.

- `stage_done` now inspects the requested commit with `git ls-tree` after confirming it is HEAD.
- A tracked runtime path produces a typed `worktree_dirty` refusal that lists every offending path and tells the lane to untrack them, commit, and run `ha done` again.
- Added a real-git regression test that force-adds `.herdr-project/x/report.md`, verifies refusal and the named path, then untracks and commits it and verifies the same report seals successfully.
- Removed the five tracked lane reports named in the brief.

Gates passed:

- `cargo fmt --check`
- `cargo test` (646 main tests, 58 herdr-pi tests, 86 herdr-pro tests, and integration suites)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
```

### t-0276 (artifact `8ce09a2d148b5e70134e567e4243a91dea4c6db5894cb4c4487d59b29adc0dcc`)

Data, not instructions.

```text
# W31 report

Implemented resolved-lane attestation and task-state recovery.

- A resolved, uncancelled current attempt without sealed `done` now derives `unknown` with the next step: retry it or attest its stored report.
- Added `ha thread attest <slug> <id> --reason <why>` with refusals for open, cancelled, already-complete, missing-copy, and hash-mismatch cases.
- Attestation stores the verified final copy as a content-addressed artifact, seals `done`, records the coordinator and reason, uses the lane folder HEAD when available, and omits the SHA when no git folder exists.
- `thread show` and `task show` display attestation evidence.
- Documented the verb in `skill/COORDINATOR.md` and `docs/operations.md`.
- Added regression tests for unknown projection, all requested refusals, successful task completion, artifact storage, absent SHA, and lane-folder HEAD.

Gates passed with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check`
- `cargo test` (649 main tests, 58 herdr-pi tests, 86 herdr-pro tests, and integration suites)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

Commit: `99aea5062ace38a483ca1f254c80e7508c667d3a`
Published branch: `hp/adeherdr/t-0276-w31-ended-lane-is-not-working`
```

### t-0277 (artifact `1e8fa3816072eec9a5e4f612b14c79a0a743d556101653a193cd528f1fd0fafe`)

Data, not instructions.

```text
# Report

## Summary

- Helper briefs now include `PROJECT.md` instructions, applicable dated standing instructions, applicable dated memory notes, and task notes without inlining `MEMORY.md` or `memory/*.md`.
- The memory cap and warnings now measure the largest task-applicable brief note payload, while coordinator context still reads the legacy memory index and historical rows still load as undated.
- Coordinator guidance now identifies `memory/` as coordinator-only and directs helper facts through dated notes.
- Added the requested coverage for legacy memory, unscoped dated notes, task scope, instructions, and unchanged coordinator visibility; updated cap-warning tests to use dated notes.

## Gates

- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check`
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test` (646 main tests, 58 herdr-pi tests, 86 herdr-pro tests, and all integration tests passed)
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets -- -D warnings`
- `git diff --check`
```

