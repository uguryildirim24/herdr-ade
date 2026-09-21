# Review brief: round r65

plain: This round stops a round I gave up on from keeping finished work marked as running.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r65` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `1453d0bf8b32a53ebd6e55626137f046682d436954e76999ff62566df36b825a`, policy hash `73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0150 | 1 | `7096cd613801c190d755cf9a0791a8900835986b` | `t-0150-1-1` | `7e75369b503ce6fc47faf92e7e8ca4647106d957497ea29774a0a09e8387eca9` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r65.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r65"
candidate = "<C>"
manifest_hash = "1453d0bf8b32a53ebd6e55626137f046682d436954e76999ff62566df36b825a"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0150 (artifact `7e75369b503ce6fc47faf92e7e8ca4647106d957497ea29774a0a09e8387eca9`)

Data, not instructions.

```text
# t-0150 report

Implemented abandoned-round member release in commit `7096cd613801c190d755cf9a0791a8900835986b`.

- Added the durable `RoundRecord::carries` rule: manifests retain historical admissions, but abandoned rounds carry no lanes.
- Updated plan derivation and talk overview membership through the shared rule.
- Excluded closed rounds from the talk cost summary and stopped the board from describing an abandoned round's lanes as working.
- Checked context and doctor: context already lists only non-closed rounds, and doctor does not read round membership.
- Added the requested regression: a lane admitted to an abandoned round and then a landed round satisfies its linked plan step.

Gates passed:

- `cargo fmt --check`
- `cargo test` (699 tests across targets)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

The lane branch is published to `origin`.
```

