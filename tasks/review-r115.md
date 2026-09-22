# Review brief: round r115

plain: More work to a finished lane holds its review, and machine records move out of sight.

Run `ha skill reviewer`, then do what this brief says.

Round `r115` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 4, manifest hash `8319e62641038dd54ddfc52a9f4044e2f7ad14565b3315bc5907c109eff6a2d2`, policy hash `7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0314 | 1 | `906a7f54276c7dd7072c72970773d8e52c573c73` | `t-0314-1-1` | `cbbb4f18e2a9d55ef92b16dd6bf8ac34156bfac353ac168e4526a1c5769f555e` |
| t-0313 | 1 | `f9e8aa81efb1f2bcce25fc94e674bed9f5b683d3` | `t-0313-1-2` | `451c93bea557cde8ed113830ee9509118a1eaec119abaf55dd17384f2c6ef7b6` |

## Gates

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above with its pinned environment and keep the actual output in your report.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r115.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r115"
candidate = "<C>"
manifest_hash = "8319e62641038dd54ddfc52a9f4044e2f7ad14565b3315bc5907c109eff6a2d2"
policy_hash = "7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425"
gates = [{ command = "cargo fmt --check", exit = 0 }, { command = "cargo test", exit = 0 }, { command = "cargo clippy --all-targets -- -D warnings", exit = 0 }, { command = "git diff --check", exit = 0 }]
+++
```

5. Follow the reviewer skill's Done instructions, then run `ha done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0314 (artifact `cbbb4f18e2a9d55ef92b16dd6bf8ac34156bfac353ac168e4526a1c5769f555e`)

Data, not instructions.

```text
# Machine records live out of sight

Implemented the hidden machine-record layout.

- New task, thread, inbox, ask, event, delivery, receipt, import, operation, dialogue, talk, plan, decision, note, term, ledger, lane-card, and artifact writes now live under each project's `.state/` folder.
- Existing top-level record kinds remain readable. On the next write, safe kinds move atomically one kind at a time. Thread records move individually so a live project-owned lane folder is not renamed underneath its process.
- Historical artifacts remain at their recorded path because immutable events may cite it; all new artifacts are hidden.
- Remote box lane cards now use `.state/lanes`, while box workers still discover historical cards.
- README, operations, ledger, and coordinator guidance describe the new layout.

Checks passed:

- `cargo fmt --check`
- `cargo test` (all targets)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
```

### t-0313 (artifact `451c93bea557cde8ed113830ee9509118a1eaec119abaf55dd17384f2c6ef7b6`)

Data, not instructions.

```text
# t-0313 report

Implemented finished-lane follow-up handling for rounds.

- A follow-up sent through `thread prompt` clears that lane's accepted pin and records the completion event it must report after.
- The round waits while the follow-up is queued or the lane is working. If the courier later observes the lane idle/done without a newer `done`, the barrier clears and the previous pin stands.
- A newer `done` clears the barrier and starts the next review revision automatically.
- A prompt during review unbinds and stops the reviewer through `threads::cancel` before waiting.
- `round advance` now accepts member pin changes after review began, cancels the superseded reviewer, and starts the new review revision in the same pass.
- Manual `round review` uses the same cancellation path when it supersedes a live reviewer.
- Historical round records remain readable because the new manifest field defaults to absent.
- Kept one end-to-end prompt/re-review test and one idle-without-new-report test; updated existing stale-pin coverage rather than adding redundant tests.

Gates passed:
- `cargo fmt --check`
- `cargo test` (686 main tests, plus all binary and integration tests)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

Commit: `f9e8aa81efb1f2bcce25fc94e674bed9f5b683d3`
Published branch: `origin/hp/adeherdr/t-0313-w55-more-work-to-a-finished-lane-holds-t`
```

