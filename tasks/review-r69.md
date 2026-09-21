# Review brief: round r69

plain: This round makes your talk tab move to each new version on its own.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r69` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `8e5f700888e3b8c40d7c199a56f4cf527f14596ef43b57e056b8c7fa407b5248`, policy hash `73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0159 | 1 | `116910ac54b861c127ccfcb2ac20a69e134d8835` | `t-0159-1-1` | `bafbb3de3998a7ab4395df8533075430c5267a6f67611683b12343953a5964a2` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r69.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r69"
candidate = "<C>"
manifest_hash = "8e5f700888e3b8c40d7c199a56f4cf527f14596ef43b57e056b8c7fa407b5248"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0159 (artifact `bafbb3de3998a7ab4395df8533075430c5267a6f67611683b12343953a5964a2`)

Data, not instructions.

```text
# t-0159 report

Implemented the talk-screen hand-off fix in `src/talk/screen.rs`.

- Removed the canonical-path equality early return. The screen now compares the installed binary's reported build version with its own `crate::VERSION` every slow poll, even when the running executable is the installed path.
- Kept `HERDR_TALK_REEXEC` as the loop guard, while allowing a later installed version to trigger another hand-off.
- Added a regression test using a symlink whose canonical installed path is the running test executable; a different reported installed build produces a re-exec plan.
- Audited the other long-running pieces. No other install hand-off uses this faulty path-equality shortcut. The ticker is replaced by its versioned lock/start flow, the courier is a short-lived ticker operation, and the installer's path comparison is paired with a captured byte hash to detect replacement of its own file.

Gates passed:

- `cargo fmt --check`
- `cargo test` (566 main tests, plus all binary and integration suites)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

Commit: `116910ac54b861c127ccfcb2ac20a69e134d8835`
Published branch: `hp/adeherdr/t-0159-the-talk-screen-always-moves-to-the-inst`
```

