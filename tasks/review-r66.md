# Review brief: round r66

plain: This round makes every command report its result the same way.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r66` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `a94a34e7e0ce7867b9375a9a3d152bbe05d6fe7e7abd71a91c8b799603976af3`, policy hash `73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0149 | 1 | `bf098a46bb46eb624ee3b124a97420247e5939e5` | `t-0149-1-1` | `46875c57eb301368f08d0ff9be52122f16e731b8417dd68b16965ebde0912d82` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r66.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r66"
candidate = "<C>"
manifest_hash = "a94a34e7e0ce7867b9375a9a3d152bbe05d6fe7e7abd71a91c8b799603976af3"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0149 (artifact `46875c57eb301368f08d0ff9be52122f16e731b8417dd68b16965ebde0912d82`)

Data, not instructions.

```text
# t-0149 report

Implemented one result path for every `herdr-ade` command.

- Added global `--json`. It emits exactly one record with `outcome`, `command`, the ordinary human `message`, useful `data`, optional `warnings`, and `reason` on refusal.
- Refusals stay non-zero and now put their reason in the JSON record on stdout rather than requiring stderr parsing.
- Routed command prose through the shared result renderer. Default output still streams the existing text.
- Added typed result data for round phase and merge outcomes, reviewer starts, asks, decisions, plan revisions, thread identities, coordinator pane/workspace identities, inbox counts, and sealed event ids.
- `round advance` now returns its started reviewer ids in both its Rust outcome and JSON `data.started`.
- Updated coordinator instructions and operations docs. Removed the separate plan/decision/ledger JSON shapes in favor of the global result envelope.

## Human text changes

Default command text is unchanged except:

1. `round advance <slug>` previously printed nothing on success. It now prints `started reviewer <id> for <round>` for every start, or `no reviewer started`.
2. Event-driven `round advance` without a slug previously printed nothing on success. It now prints `rounds advanced for the event's projects`.
3. Help output now lists the global `--json` flag. The old command-local `--json` help entries for `plan show`, `decide list/show`, and `ledger list` are gone; those spellings still work through the global flag and return the common envelope.

## Checks

Passed on `oci`:

- `cargo fmt --check`
- `cargo test` (all 718 unit/integration tests across the three binaries)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

Also ran a scratch-root CLI probe confirming `round open --json` and `round show --json` return the record's `phase` (`admitting`) without deriving it from prose.
```

