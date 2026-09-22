# Review brief: round r92

plain: Installing twice on the same code still shows what each machine runs, and proven jobs stay proven.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r92` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `f17bac1fbdb5a5af50147fe10d6e98811b15518661183a2180441fb05752aa05`, policy hash `d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0222 | 1 | `67e5cc8ded54b58c55d6c2e5d5aebe71d53bdfa6` | `t-0222-1-1` | `b5d8014e7ac122b2dc6745799e67e8bc2272c9ff4df8c020726e6f146c46459b` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r92.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r92"
candidate = "<C>"
manifest_hash = "f17bac1fbdb5a5af50147fe10d6e98811b15518661183a2180441fb05752aa05"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0222 (artifact `b5d8014e7ac122b2dc6745799e67e8bc2272c9ff4df8c020726e6f146c46459b`)

Data, not instructions.

```text
# W14 report

Commit: `67e5cc8ded54b58c55d6c2e5d5aebe71d53bdfa6`

## What changed

- The box ticker proof now reads one lock-file snapshot per attempt, accepts it only when both the build and PID parse, and retries incomplete snapshots. An incomplete record ends as unknown, never as stale with an empty build.
- Local installation now compares the built and installed binaries by reported commit before copying. A rerun of the same commit leaves the installed inode alone, so already-running talk screens and the proof agree.
- A task that reached verified after a successful running-process check stays verified after a later unknown check. Its generated/task-show status says `verified; latest process check unknown`, and it remains terminal, so the ticker does not nudge the coordinator to recheck every verified job.

I chose not to reinstall an identical-commit binary because it is the smaller correct fix: no talk-screen restart machinery is needed, and machine-specific build stamps cannot replace an otherwise equivalent running image.

## Tests

Added regression coverage for an incomplete ticker record, same-commit inode preservation, and verified-state preservation/display after a later unknown process check. Updated the install scenario for the pre-install version comparisons.

Passed:

- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check`
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test` (624 main tests, 57 pi tests, 82 pro tests, and integration tests)
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets -- -D warnings`
- `git diff --check`
```


## Repair revision

This revision reviews the integration base `4e5a3d91fe0a97015f68467a69e3237878835d19`.
