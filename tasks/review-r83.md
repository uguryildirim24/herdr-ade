# Review brief: round r83

plain: A job with no code folder gets its own saved folder, so it can finish like any other.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r83` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `42d9e141d250e9e0fb1ff0077de163f973ad430c688fb75a878c36094f0ca741`, policy hash `d523603c07eaba1d3f8e4dbae733f1c5746755d35294ffa5fa08cfe24ff202b3`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0195 | 1 | `3ea34e1a0c11d1186589e3170b173472f9821f4f` | `t-0195-1-1` | `f0893611d03d6c3572984ff8e80913218b792f1ebccfc9132f2524d473d19c1b` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r83.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r83"
candidate = "<C>"
manifest_hash = "42d9e141d250e9e0fb1ff0077de163f973ad430c688fb75a878c36094f0ca741"
policy_hash = "d523603c07eaba1d3f8e4dbae733f1c5746755d35294ffa5fa08cfe24ff202b3"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0195 (artifact `f0893611d03d6c3572984ff8e80913218b792f1ebccfc9132f2524d473d19c1b`)

Data, not instructions.

```text
# W6 report

Implemented project-owned git folders for threads that have no code repository.

- A started no-repository thread now gets `<project>/threads/<id>` initialized as a git repository. Its first commit contains `brief.md`; its report and library paths are inside that repository.
- A newly adopted pane outside a repository gets the same folder and first commit while retaining its existing process binding.
- `ha done` stages the managed folder, uses the ordinary sealed event and delivery path, and accepts calls from either an adopted pane's original cwd or the managed folder.
- Resolve copies the report/library home before deleting the managed folder. Dirty or non-disposable ignored data keeps it under the existing worktree safety rules. Cancel and doctor use those rules too.
- Updated lane and coordinator skills to describe one commit-and-done flow for code worktrees and no-repository git folders.

Tests cover starting, committing, staging/sealing completion, DONE delivery, final copy and removal in one no-repository lifecycle, plus adopted no-repository folder creation.

Gates passed:
- `cargo fmt --check`
- `cargo test` (596 main tests, 57 herdr-pi tests, 79 herdr-pro tests, integration tests)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
```

