# Review brief: round r76

plain: Doctor leaves a live lane's worktree alone, and each project can list its own test leftovers.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r76` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `187050bdac3226601b71e850fc9d213620e30012f365483c8445a6f6ebe55244`, policy hash `8e43e0ebbd105ebf9f903667c622cb0dad8aa881e4ceb5a344db4659ff0338da`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0179 | 1 | `1094ced314c43d599d73296840f230ec7bfb26a9` | `t-0179-1-1` | `bfec698af8d9d300dc467f77619ca145e051da366f1f91060e7537ff6299f42e` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r76.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r76"
candidate = "<C>"
manifest_hash = "187050bdac3226601b71e850fc9d213620e30012f365483c8445a6f6ebe55244"
policy_hash = "8e43e0ebbd105ebf9f903667c622cb0dad8aa881e4ceb5a344db4659ff0338da"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0179 (artifact `bfec698af8d9d300dc467f77619ca145e051da366f1f91060e7537ff6299f42e`)

Data, not instructions.

```text
# W2 worktree cleanup follow-ups

Implemented the worktree cleanup fixes.

- Doctor now considers only resolved thread records for claimed finished worktrees. Open/working lanes are left alone.
- Unreadable thread records make the doctor row explicitly unknown instead of treating the checkout as finished.
- Disposable paths accept `*` within a single path component, including `runs/pytest-*` without matching `runs/seed-1`.
- Repository rows in `PROJECT.md` and harness repository rows in `config.toml` can add repository-specific `disposable` lists to the global list. The same resolved list is used for local and box worktrees and review worktrees.
- Updated README, operations/getting-started docs, and the coordinator skill.
- Added regression coverage for live lanes, unknown thread state, wildcard filtering, and repository isolation.

Gates passed:

- `cargo fmt --check`
- `cargo test` (all suites)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

The Mac-only handoff path `/home/agent/projects/herdr/HANDOFF.md` was unavailable on the `oci` box.
```

