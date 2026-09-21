# Review brief: round r80

plain: Installing and checking the harness records its own proof on each job.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r80` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `14e1c86e863f9c767e5b7f759e05166f132e1697da16657a2401257674a408ac`, policy hash `3ff3234b04a6b9438ce5a97e9ebb249f4698636f90353f2a3946112ef9d3e1d2`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0190 | 1 | `e6cdeb00a760c3bebcdab8cc58c366a5dcf4edc4` | `t-0190-1-1` | `54e38b79f94fde232fd0a3f20519db3bfbaa7a05f96afe025fad41acd2483dde` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r80.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r80"
candidate = "<C>"
manifest_hash = "14e1c86e863f9c767e5b7f759e05166f132e1697da16657a2401257674a408ac"
policy_hash = "3ff3234b04a6b9438ce5a97e9ebb249f4698636f90353f2a3946112ef9d3e1d2"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0190 (artifact `54e38b79f94fde232fd0a3f20519db3bfbaa7a05f96afe025fad41acd2483dde`)

Data, not instructions.

```text
# D5b install proof

Implemented automatic install and runtime evidence for stable tasks.

- `ha harness install` now re-execs into a newly installed self instead of refusing and requiring a second shell invocation.
- One install replaces and waits for local and `oci` tickers, checks installed binaries, inspects each running talk process by executable inode, and returns typed process/build evidence with honest `unknown` reasons.
- Installed commits are matched against merged round heads before machine-specific evidence is added to tasks. A separate running-process record gates acceptance verification and is shown by `task show`.
- Added `task adopt <slug> <job> --thread <id>` with repository and existing-task checks; historical rounds are linked automatically.
- Removed the coordinator's manual ticker/process/install-evidence steps from the skill.

Checks passed:

- `cargo fmt --check`
- `cargo test` (590 main tests, plus all binary and integration suites)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
```

