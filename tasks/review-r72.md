# Review brief: round r72

plain: This round removes the part that picks a helper by score, and a short table you can edit picks instead.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r72` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `aa68f5e79be6feb79f184905d5fa03f9686280ca2443ffaa503302334d65dfd7`, policy hash `73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0165 | 1 | `44587ad21249dffe93b14cd73931ed9c4ed506ad` | `t-0165-1-1` | `f9b0954213d886547d1c3f9b36bd4e834422337032305762599a792ee911bcc4` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r72.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r72"
candidate = "<C>"
manifest_hash = "aa68f5e79be6feb79f184905d5fa03f9686280ca2443ffaa503302334d65dfd7"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0165 (artifact `f9b0954213d886547d1c3f9b36bd4e834422337032305762599a792ee911bcc4`)

Data, not instructions.

````text
# D1 report

Removed Jev and its TypeSafe client, score/price/tier routing, evaluator command, policy files, probes, documentation, tests, coordinator kind refusal, and tier escalation. Replaced them with ordered `[routing]` rules in `config.toml`, hash pins, and bounded retry/fallback recovery. Launch and ledger records now name `pin`, `default`, or `rule[n]`; old records with removed fields still deserialize.

`git diff --numstat` reports 2,373 deleted lines and 475 added lines (net -1,898). The deleted implementation includes `src/jev.rs` (221), old `src/routing_tests.rs` (1,025), `config/routing.json` (38), 554 old lines from `src/routing.rs`, and 274 old lines from `src/launch.rs`.

Install this starting table in `~/.config/herdr-ade/config.toml` on the Mac and box:

```toml
[routing]
default = "pi_codex_sol_high"
retries = 1
fallback = []

[[routing.rules]]
workflow = "coordinator"
recipe = "claude_coordinator_opus"

[[routing.rules]]
product = "spec"
recipe = "claude_fable_xhigh"

[[routing.rules]]
product = "web-research"
recipe = "agy_gemini_flash"

[[routing.rules]]
requires_claude = true
recipe = "claude_fable_xhigh"
```

Verified:

- `cargo fmt --check`
- `cargo test` (705 tests across all targets)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
````


## Repair revision

This revision reviews the integration base `08071c386d1fc354db7e6cb3369c00bff60cab91`.
