# Review brief: round r58

plain: This round lets rounds of work open and get checked side by side, so none waits for another.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r58` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `0bb401183a4f628a8f6dab66744cc91cebf19865c847d8ca7b200b853e77cd7b`, policy hash `73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0128 | 1 | `df6f91abe5a284965fd45ae7045be29faba4acf8` | `t-0128-1-1` | `a48939599022e02a9c65564511c789248a425b80704405cc2bb5152689d98195` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r58.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r58"
candidate = "<C>"
manifest_hash = "0bb401183a4f628a8f6dab66744cc91cebf19865c847d8ca7b200b853e77cd7b"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0128 (artifact `a48939599022e02a9c65564511c789248a425b80704405cc2bb5152689d98195`)

Data, not instructions.

````text
# t-0128 — rounds stop waiting for each other

Commit: `df6f91abe5a284965fd45ae7045be29faba4acf8`
Branch: `hp/adeherdr/t-0128-rounds-stop-waiting-for-each-other`
Machine: `oci`

## Delivered

- Removed `require_branch_available`, the `round_head_reserved` refusal, its hints, and the reservation tests. Opening and reviewing multiple rounds against the same integration branch is now allowed.
- Review branches and worktrees remain round/revision-specific (`review/rN[-K]` and `.worktrees/review-rN[-K]`), so overlapping reviews do not share outputs.
- A fresh merge now holds the repository lock continuously from its integration-head check through the ref effect. The lock order remains repository lock, then project lock. Per-round operation locks remain independent.
- When the integration head is a newer descendant of reviewed B, `round merge` creates the next repair review revision on that exact new base and invokes the shared reviewer-start effect itself. It returns `RepairReviewStarted` rather than failing for coordinator action.
- The round's `attention` field records the old base, new base, repair branch, and reviewer in one line; `round show` now prints that line.
- Repair revisions commit a new B on the new base. Their reviewer task names the earlier candidate C and verdict V and directs the reviewer to carry C, including prior review fixes, over the new base.
- A clean Git merge still gets repair review. I chose this because textual non-conflict does not prove semantic compatibility between independently reviewed changes; the moved pairing itself was never reviewed.
- Historical record shape is unchanged. Existing `ReviewIntent.reuse_brief` remains loadable; no reservation flag, shim, or compatibility mode was added.

## Prose removed or replaced

- `skill/COORDINATOR.md`: removed the manual `merge_conflict` → `round review` → `round advance` instruction and replaced it with automatic moved-base repair. Removed the claim that abandonment releases an integration branch.
- `docs/operations.md`: removed the manual conflict-repair instructions, old-B reuse description, and branch-release wording. Documented automatic repair and the clean-merge decision.
- `src/cli.rs`: removed “release its integration branch” from `round abandon` help.
- `skill/REVIEWER.md` contained no sentence telling a reviewer to finish or abandon another round before opening one, so none was removed there.

## Scratch sequence

Ran the qualifying regression against an isolated `tempfile` ADE root and a real scratch Git repository (production round code and real Git; only the Herdr transport is scripted by the test fixture):

```text
cargo test round::tests::two_reviewing_rounds_merge_in_turn_with_automatic_repair -- --exact --nocapture
SCRATCH root=/tmp/.tmp6z5N19 r1=merged head=cae59252edd061dd906ba02053e3102bc12708aa r2=under_review
SCRATCH r2=automatic_repair branch=review/r2-2 reviewer=t-0005 B=bffd1e47b5572a7539061095626f192fda63547f
SCRATCH r2=merged head=330763847df241ff8e63eef177aed3919d1a3617
... ok
```

The sequence opens both rounds on `main`, admits and completes one lane in each, creates both review branches/worktrees, reviews r2 then r1, merges r1, observes r2's moved base, automatically creates and starts `review/r2-2`, seals the repair verdict, and merges r2. The complete captured output is `library/scratch-sequence.txt`. No throwaway agent pane was started.

## Regression coverage

- `two_rounds_can_open_on_the_same_integration_branch`
- `two_reviewing_rounds_merge_in_turn_with_automatic_repair`
- Updated manual repair coverage verifies a new brief commit whose parent is the new base and a task carrying earlier C/V.
- Existing crash recovery, historical-record migration, exact-verdict, dirty-checkout, rewind, and repository-lock tests remain green.

## Gates

All run with `PATH=/bin:$PATH` on the final source:

- `cargo fmt --check`: PASS.
- `cargo test`: PASS, 682 tests (530 ADE + 55 pi + 78 Pro + 6 CLI + 8 actionable context + 2 record context + 3 routing CLI).
- `cargo clippy --all-targets -- -D warnings`: PASS.
- `git diff --check`: PASS.

Logs are under `library/gates/`.

## Notes

- The brief's Mac-only `DEVELOPER_DIR` path is not present on this Linux box; the required cloud command used `PATH=/bin:$PATH`.
- No project memory, live ADE root, installed binary, or live integration branch was changed.
````

