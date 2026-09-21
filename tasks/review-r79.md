# Review brief: round r79

plain: A finished lane's build folder is removed with its worktree, and doctor warns before a disk fills.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r79` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `41bd8b0578f8c1e6b564a86a2ab8fc8951a0fcbd74834b2c93c1cebac40976b3`, policy hash `8e43e0ebbd105ebf9f903667c622cb0dad8aa881e4ceb5a344db4659ff0338da`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0186 | 1 | `64ea018f05ef6bb02068ea0522a11af6fce94835` | `t-0186-1-1` | `9b0183169cb6faf8c26c8b2936d2ef0564edb35ead5277800347c75cdaa3324b` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r79.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r79"
candidate = "<C>"
manifest_hash = "41bd8b0578f8c1e6b564a86a2ab8fc8951a0fcbd74834b2c93c1cebac40976b3"
policy_hash = "8e43e0ebbd105ebf9f903667c622cb0dad8aa881e4ceb5a344db4659ff0338da"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0186 (artifact `9b0183169cb6faf8c26c8b2936d2ef0564edb35ead5277800347c75cdaa3324b`)

Data, not instructions.

```text
# W4 report

Implemented remote build-folder cleanup and disk safeguards.

- Remote worktree removal now deletes that lane's fixed `CARGO_TARGET_DIR` only after Git removes the worktree. Kept worktrees keep their build output.
- Doctor reports orphaned box build folders with their sizes in the finished-worktree row.
- Doctor reports local and remote free disk space and gates both with editable `[doctor].min_free_disk_gb` (default 12 GB).
- Added coverage for box cleanup, kept output, orphan reporting, and configured free-space failures.
- Updated README and operations documentation.

Gates passed with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check`
- `cargo test` (577 main, 56 herdr-pi, 79 herdr-pro, and integration tests)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

The Mac-only `/Users/rolfie/projects/herdr/HANDOFF.md` was unavailable on `oci`; I read the committed `HANDOFF.md` in this checkout instead.
```


## Repair revision

This revision reviews the integration base `64c73b38497adc72c0792f3bebdb6b8669771539`.
