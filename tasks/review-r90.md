# Review brief: round r90

plain: Every coordinator keeps working on its own and asks only what is truly Rolf's to decide.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r90` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `254538c2c5de6e34ba8d4274c5ce79c2b49c6bc6e0dd147ba76ed15ab8821df5`, policy hash `d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0213 | 1 | `3726e95cc1a4cf57280b912318fee4f087e5ab42` | `t-0213-1-1` | `d45b66dc3afad7b2a3caefd1198f4b6043c48568e1d87dafa8e508bbbb763d86` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r90.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r90"
candidate = "<C>"
manifest_hash = "254538c2c5de6e34ba8d4274c5ce79c2b49c6bc6e0dd147ba76ed15ab8821df5"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0213 (artifact `d45b66dc3afad7b2a3caefd1198f4b6043c48568e1d87dafa8e508bbbb763d86`)

Data, not instructions.

```text
# W13 report

Implemented autonomous coordinator progress:

- `start_threads` now defaults to `auto`; an explicit `propose` setting remains supported and tested.
- The coordinator skill now tells coordinators to decide reversible/default choices, record and announce them, continue other work, and ask Rolf only for money, irreversible/outside-machine actions, or taste/direction.
- `ha ask` now rejects punctuation/case/spacing variants of both open and answered questions and reports the earlier ask id and answer.
- Waiting lane evidence now drives a task next step of `wait for Rolf: ...`.
- The ticker now sends a bounded continue prompt to an idle coordinator with actionable open task steps. It skips nudges for pending Rolf messages, working lanes, all-waiting tasks, priming, cooldown, and no intervening coordinator turn. The interval is `[coordinator].idle_nudge_minutes`, default 20. Successful nudges are append-only ledger facts.
- Updated README, getting-started, operations, ledger, and coordinator skill text.

Gates passed with `PATH=/bin:$PATH`:

- `cargo fmt --check`
- `cargo test` (611 main tests, 57 pi tests, 78 pro tests, integration suites all green)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
```


## Repair revision

This revision reviews the integration base `6b3b86930641dc236b4a87fced6b49e37608f5b7`.
