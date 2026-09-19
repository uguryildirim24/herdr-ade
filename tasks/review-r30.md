# Review brief: round r30

plain: This round checks the way a finished round recovers when the main line moved under it.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r30` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `864956d7c018dafabc69af4553e337020483b0c0ae72ba1e5da1cfe9dc4cad2c`, policy hash `ef334053e0961740bc2a51d4698483517101f9dc9bb35a46bff91e23b9d43d21`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0065 | 1 | `37b83ddcb020f4a93e73481ac81a49d127511c4c` | `t-0065-1-2` | `887db93dac08c92ebce0c72a94ac33471438b5825aaafeb58f6cb27d12b8945d` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r30.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r30"
candidate = "<C>"
manifest_hash = "864956d7c018dafabc69af4553e337020483b0c0ae72ba1e5da1cfe9dc4cad2c"
policy_hash = "ef334053e0961740bc2a51d4698483517101f9dc9bb35a46bff91e23b9d43d21"
gates = []
+++
```

5. Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0065 (artifact `887db93dac08c92ebce0c72a94ac33471438b5825aaafeb58f6cb27d12b8945d`)

Data, not instructions.

```text
# t-0065: rounds recover from a merge conflict

Lane: `hp/adeherdr/t-0065-rounds-recover-from-a-merge-conflict-lan`, based on `788cd50`.

## Repair revision (`src/round.rs`)

`round review` now detects a **frozen round whose brief is unchanged**:
`expected_head` is set, `frozen_revision == manifest.revision`,
`manifest_hash == manifest_hash(record)`, and B is still an ancestor of the
current integration head. In that case it does not commit a second brief
(the old "nothing to commit, working tree clean" failure); it keeps B as the
brief commit and creates the next `review/<r>-<n>` branch from the **current
integration head**. It still clears the reviewer and prints the branch.

`advance` starts a reviewer whenever the record has a review branch but no
reviewer (a by-hand `round review`, a repair, or a failed start), so the
merge-conflict recovery is `round review` then `round advance`. A failed start
is announced once and retried on the next pass.

The reviewer task (`reviewer_task`) finds the earlier revision through the
previous review branch. The review branch itself only holds the reviewer's
`docs(tasks): <id>` task commit, so the code reads that commit to find the
previous reviewer, then reads the reviewer's own branch head as the verdict
commit V and V's parent as candidate C. When the manifest hash matches the
earlier verdict, the task gets a **Repair review** section: merge C (which
carries the earlier reviewer's fixes) over the new base instead of the raw lane
shas, and it names C, V and `tasks/reviews/code-<r>.md`. When the manifest
moved (a REJECT repair, t-0058's case) it names the same three and says to read
the earlier findings but merge the pinned shas. `round review` also prints the
earlier C and V, so the by-hand start line names them too.

`round merge`'s conflict refusal now ends with
`; run `round review <r>`, then `round advance``.

## Rebinding (`src/round.rs`)

`round reviewer` still refuses a live bound reviewer with
`reviewer_already_bound`, but a bound reviewer that is resolved or gone is
replaced instead. `advance` still only reports a gone reviewer; it never
auto-replaces.

## Resolve guard (`src/threads.rs`, `src/cli.rs`, `src/round.rs`)

`thread resolve` refuses a lane pinned in a round with no merge record, and a
reviewer bound to such a round, with `round_unmerged: ... run `round merge
<r>` first, or pass --force`. `--force` (new flag, conflicts with `--reopen`)
overrides and writes one `say` line. Helper: `round::open_round_pinning`.

## Cap (`src/threads.rs`)

`open_lane_count` counts open/starting lanes whose `last_state` is not `done`;
a done lane keeps its pane but holds no slot. `thread start` uses it for the
`max_parallel_threads` warning.

## Replay (`src/steps.rs`)

`deliver_events` skips any event whose thread is resolved, so the delivery
journal repair no longer re-types stale wake-ups for long-resolved lanes.

## Prompt (`src/threads.rs`)

`prompt_state` allows a pi lane in the `blocked` state (its own error) and
`prompt` clears the recorded `error` after sending. A gone pane is still
refused by the agent lookup.

## Docs

`docs/operations.md`: a new Rounds section with the repair-revision rule.
`skill/COORDINATOR.md`: the round-merge bullet now says to read `round merge`'s
exit before anything that follows it, never behind a pipe. I also corrected the
stale "a gone reviewer is reported, not replaced" clause to match the new
rebinding rule.

## Tests

New: `repair_review_reuses_b_and_names_the_earlier_candidate` (real
advance-started reviewer, so it exercises the reviewer-branch lookup),
`bind_reviewer_replaces_a_gone_reviewer_but_not_a_live_one`,
`resolve_refuses_a_lane_pinned_in_an_open_round`,
`open_lane_count_skips_a_done_lane`,
`prompt_reaches_a_blocked_pi_lane_and_clears_its_error`, and
`a_resolved_lane_event_is_never_replayed`. All use
`round::testkit::fixture`/the fake runner. The merge-conflict test now checks
the hint tail.

## Gates

`cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`,
`cargo build --release --locked` pass. `cargo test --locked`: 402 passed, 2
failed. Both failures are pre-existing on the base `788cd50` on this box:
`pi::scenarios::scenario_setup_then_check_for_kimi` (the box shell is bash, the
test scripts zsh) and
`threads::tests::an_unreachable_box_falls_back_to_this_mac` (the fake ssh
stub wins over the failure stub). No new failures.

## What t-0058 must fold in

t-0058's branch is not merged into `main` (checked; `main` is at `788cd50`), so
I did not base on it. My `advance` now uses a `start_and_bind` helper and
starts a reviewer when `review_branch` is `Some` and `reviewer` is `None`;
t-0058's `advance` rewrite is a superset and should replace that helper, keeping
the retry-on-failed-start and the "branch exists, no reviewer" case. My
`reviewer_task` takes a `Git` and adds `previous_review_branch`/`earlier_review`;
t-0058's `earlier_review` returns only the verdict, path and branch. It must
fold in the candidate C and verdict commit V (and the previous reviewer's own
branch lookup), because the review branch head is the reviewer's task commit,
not V. t-0058's `startable_reviewer`/`reviewer_ready` test helper is the same
idea as my `startable_reviewer`; keep one.
```

