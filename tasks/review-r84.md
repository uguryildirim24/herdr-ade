# Review brief: round r84

plain: Every coordinator waits for the harness fix and never goes around it.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r84` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `bd64714da47fdf99035e8ab06288460b0911f69608c450c795b17dc3dbc897e5`, policy hash `d523603c07eaba1d3f8e4dbae733f1c5746755d35294ffa5fa08cfe24ff202b3`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0202 | 1 | `068c0200ea5a0f23e9a8eb57ae9685055be736d6` | `t-0202-1-1` | `35103b751532ca1137da17583b0de1f9b9a7d6940ef684be71cec28c3b36d3cb` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r84.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r84"
candidate = "<C>"
manifest_hash = "bd64714da47fdf99035e8ab06288460b0911f69608c450c795b17dc3dbc897e5"
policy_hash = "d523603c07eaba1d3f8e4dbae733f1c5746755d35294ffa5fa08cfe24ff202b3"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0202 (artifact `35103b751532ca1137da17583b0de1f9b9a7d6940ef684be71cec28c3b36d3cb`)

Data, not instructions.

```text
# t-0202 report

## Result

- Updated `skill/COORDINATOR.md` to require harness failures to be fixed through a lane, round, and `ha harness install`.
- Explicitly forbade hand-start/adopt substitutions, pane input that forces awaited state, and project-specific hand-made repositories or files.
- Required coordinators to tell peers to wait for the coming install and to tell Rolf what is blocked and which fix unblocks it.
- Kept `thread adopt` documented only for a verified process already belonging to the lane.
- Updated `skill/PI.md` to route starts through `thread start` and blocked-provider recovery through `thread retry`; removed direct pane typing as recovery.
- Reviewed `skill/COORDINATOR.md`, `skill/LANE.md`, `skill/PICKUP.md`, `skill/REVIEWER.md`, and `skill/PI.md` for workaround guidance.

## Gates

All passed with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check`
- `cargo test` (603 library tests, 57 herdr-pi tests, 79 herdr-pro tests, and integration suites)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

## Commit

`068c0200ea5a0f23e9a8eb57ae9685055be736d6`
```


## Repair revision

This revision reviews the integration base `48490141aeec27a0dc650a5a088f078f9b911c1d`.
