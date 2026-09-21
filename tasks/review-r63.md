# Review brief: round r63

plain: This round stops rounds that are checked side by side from sending each other back.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r63` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `c97c198fbc1668f1da802686c6222528096a2f445b1788ae5edb33f0ad00cda3`, policy hash `73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0144 | 1 | `c6bd1937fbbcc7789d3bddff19e0b3747529f0f2` | `t-0144-1-1` | `166db8996770e1cabdbaac586db253a070b22ef30f7fb65dbcf7410e72664050` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r63.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r63"
candidate = "<C>"
manifest_hash = "c97c198fbc1668f1da802686c6222528096a2f445b1788ae5edb33f0ad00cda3"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0144 (artifact `166db8996770e1cabdbaac586db253a070b22ef30f7fb65dbcf7410e72664050`)

Data, not instructions.

```text
# t-0144 report

Implemented option 1: judge a moved base by the paths changed on the integration branch.

This is the smaller, safer design because lane and review branches are deliberately created from committed briefs, and HANDOFF is the repository's canonical checkpoint. Moving those outputs off the integration branch would break that existing evidence flow. Their presence is harmless; only project changes need compatibility review.

## Changes

- `round merge` now walks `B..head` on the integration branch's first-parent history and examines each commit's first-parent delta. This catches substantive changes even if a later commit reverts them.
- These are the exact bookkeeping paths the harness writes and now ignores for repair decisions:
  - `tasks/t-<at least four digits>.md` — lane and reviewer task briefs
  - `tasks/review-r<digits>.md` — round review briefs
  - `tasks/reviews/code-r<digits>.md` — review verdicts
  - `HANDOFF.md` and `HANDOFF.json` — manual and post-merge checkpoints
- Dialogue turn files and every other path remain project changes and trigger automatic repair review.
- The merge still integrates a reviewed verdict over intervening bookkeeping under the existing repository lock, then checkpoints normally.
- Updated the coordinator skill and operations documentation; removed the old claim that every integration-head move requires repair.

## Scratch sequences

The real temporary-git fixture `round::tests::two_reviewing_rounds_merge_in_turn_with_automatic_repair` runs both sequences with two reviews alive together:

1. Bookkeeping-only movement:
   - review `r1`, producing `B1` and its verdict;
   - review `r2`, producing `B2` whose parent is `B1`;
   - merge `r1` while `B2` is the integration head;
   - result: `r1` enters its merge intent and reaches `RoundPhase::Merged`; no repair branch or second reviewer is created.
2. Real intervening merge:
   - after `r1` lands its lane's `src/lane1.rs` change and checkpoint, merge `r2`, whose verdict was based on `B2`;
   - result: `RepairReviewStarted`, branch `review/r2-2`, with an automatically started reviewer whose task names the earlier candidate and verdict.

The path-boundary fixture also confirms that dialogue turns and source files are not mistaken for bookkeeping.

## Gates

With `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check` — passed
- `cargo test` — passed (545 main tests, 56 herdr-pi tests, 79 herdr-pro tests, and integration tests)
- `cargo clippy --all-targets -- -D warnings` — passed
- `git diff --check` — passed

Commit: `c6bd1937fbbcc7789d3bddff19e0b3747529f0f2`
```

