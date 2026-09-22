# Review brief: round r86

plain: A new web helper lane starts on the current tool, and its first answer comes back.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r86` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `70e8d96f748f4a6fd14ac7ddc5b29578852fac2a3041072b4c0d2b4de642d419`, policy hash `9698216e305f216fae9b6ee0c22dc18d3ae6746e02ba7fcb779887a8dcd5c01f`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0198 | 1 | `edbde2237f4f0a3eab3b7683832deb6ba70e41d0` | `t-0198-1-1` | `304c5370e7ff3a1ebc32b3dcd72a906a58e7f08f1c998ab20b5e4d391d4f1cee` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r86.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r86"
candidate = "<C>"
manifest_hash = "70e8d96f748f4a6fd14ac7ddc5b29578852fac2a3041072b4c0d2b4de642d419"
policy_hash = "9698216e305f216fae9b6ee0c22dc18d3ae6746e02ba7fcb779887a8dcd5c01f"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0198 (artifact `304c5370e7ff3a1ebc32b3dcd72a906a58e7f08f1c998ab20b5e4d391d4f1cee`)

Data, not instructions.

```text
# t-0198 report

## Result

- `herdr-pro start` and `resume` now finish when Herdr reports a ready, unblocked agent and the trust prompt is absent. They no longer wait for a Codex rollout; a new lane records `rollout = None` while retaining Herdr's session id.
- A first turn may begin without a rollout. It types the packet load first, waits up to the load bound for the rollout, matches it through the existing session-id/cwd/start-time rules, reads from byte zero, and persists the rollout path and discovered session id.
- Rollout waiting ends early if the agent blocks or disappears. A missing rollout at the bound writes a failed turn with `failure_class = "unknown"` and an explicit reason.
- New-tab shell startup races no longer fail the lane: `agent_pane_busy` is retried every 250 ms within the 120 s readiness bound. Other start errors remain final.
- No README, docs, or skill text stated the removed startup-rollout guarantee, so none needed changing.

## Tests

- Added coverage for start returning ready with no rollout.
- Added coverage for a first input creating a rollout and the collector reading its initial records.
- Added coverage for a rollout that never appears becoming a bounded typed turn failure.
- Added coverage for `agent_pane_busy` succeeding on retry.
- Passed `cargo fmt --check`.
- Passed `cargo test` (596 main, 57 herdr-pi, 78 herdr-pro, and integration suites).
- Passed `cargo clippy --all-targets -- -D warnings`.
- Passed `git diff --check`.

## Caveat

The Pro lane runs only on the Mac, so I did not perform the live Codex check on this box. The coordinator should run that after installation.

## Commit

`edbde22` — `fix(pro): defer rollout discovery to first turn`
```

