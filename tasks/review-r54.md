# Review brief: round r54

plain: This round checks that the sign in rows ask how each helper is really reached.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r54` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `3f79436518d323a21edcf6cc5b58ef400e578046faad3c1a92597acc3cff8f12`, policy hash `518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0116 | 1 | `ce295b5ada5566f3dfa13239da8f30457a2184ff` | `t-0116-1-1` | `c0db9da2002bd651c09a60ef57138a7a6f8c38ae9e33274c9ab408ac213ccbae` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r54.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r54"
candidate = "<C>"
manifest_hash = "3f79436518d323a21edcf6cc5b58ef400e578046faad3c1a92597acc3cff8f12"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0116 (artifact `c0db9da2002bd651c09a60ef57138a7a6f8c38ae9e33274c9ab408ac213ccbae`)

Data, not instructions.

```text
# t-0116 — recipe-derived box readiness

## Changes

`src/doctor.rs` now derives box readiness rows from the merged, enabled recipe configuration, including configured overrides:

- `kind = "pi"` uses `herdr-pi check <provider>` once per provider; no standalone login is inferred from model or recipe names.
- Native kinds use the same runtime-specific probe definitions as placement. These definitions describe commands, not a second list of required logins.
- Disabled/removed recipes leave no login/provider probe or row behind. Unsupported native kinds and inconsistent pi provider arguments fail explicitly.
- The pane tools probe derives native executable requirements from those same recipes, so it cannot preserve an obsolete native requirement or omit a newly configured native Codex recipe.

No compatibility paths, fallbacks, flags, installations, credentials, or project memory changes.

## Claude and agy

The shipped enabled recipes actually start native `claude` and `agy`, so asking for those binaries is correct, not a mistaken substitute for pi readiness. Claude uses `claude auth status`; agy uses the existing `agy models` runtime probe (not a dedicated authentication-status command). Both fail legitimately when their binary is absent. The old unconditional list selected these two correctly only by coincidence; their inclusion now follows their recipes and disappears if they are disabled or moved to pi.

Read-only checks on `oci` during this attempt confirmed:

- `herdr-pi check openai-codex`: `ok: true`, login `openai-codex ready`.
- `command -v` under the lane PATH: `claude`, `agy`, and `codex` all absent.

The built candidate was tested with deterministic box fixtures; it was not installed or used to run a full live doctor against the coordinator's machines.

## Tests and gates

Added coverage for pi-only Codex access with no native binaries/login, provider deduplication, missing native binaries for all three supported kinds, removed/disabled recipes, configured recipe overrides through the full report, and fail-closed invalid readiness definitions. Existing box capacity, reachability, wrapper and provider failure tests remain green.

All gates used `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools` and the lane's external `CARGO_TARGET_DIR`:

- `cargo fmt --check`: PASS.
- `cargo test`: PASS, 674 tests (522 + 56 + 78 unit tests; 18 integration tests).
- `cargo clippy --all-targets -- -D warnings`: PASS.
- `git diff --check`: PASS.

An initial targeted command used `--lib`, but this package has no library target; reran against its binary targets. One initial fixture registered a pane response after a first-match fake, corrected before the full passing run.

## Durable lesson

Runtime probe definitions are not a deployment inventory. Derive both login rows and native tool requirements from executable recipes; pi provider names do not imply standalone CLI credentials.
```

