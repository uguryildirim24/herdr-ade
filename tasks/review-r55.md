# Review brief: round r55

plain: This round lets a job say which helper it starts from, so a check starts from the careful one.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r55` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `252b694089500e5fcae794393f5fe50e0155c254c15e6d505aa4f2ad9fc08e4e`, policy hash `518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0119 | 1 | `1d912780b37a66452c423d25a8d6236d3b87b247` | `t-0119-1-1` | `3e145110f5bb427fe148eb2e957706ad879cd658e3847e51552cecb91b526fdf` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r55.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r55"
candidate = "<C>"
manifest_hash = "252b694089500e5fcae794393f5fe50e0155c254c15e6d505aa4f2ad9fc08e4e"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0119 (artifact `3e145110f5bb427fe148eb2e957706ad879cd658e3847e51552cecb91b526fdf`)

Data, not instructions.

````text
# t-0119 — routing floors

Commit: `1d912780b37a66452c423d25a8d6236d3b87b247`
Branch: `hp/adeherdr/t-0119-a-role-can-set-the-model-a-lane-starts-f`
Machine: `oci`

## Delivered

- Optional `role_floors` and `answer_floors` in routing.json; neither ships configured. Both use one tier-raising mechanism after ordinary scoring, uncertainty upgrades and strictly stronger escalation. A higher/equal pick is retained, not pinned to the floor recipe.
- Role floors use the shared workflow passed into dispatch. Both `round advance` and ordinary `thread start --workflow reviewer` receive the same floor. CLI help and operations docs now say so.
- Answer floors compare a named question's raw zero-based score against an inclusive `min_score`, independent of weights. Multiple triggered floors combine by maximum tier. Invalid roles/questions/thresholds/model cards, unknown recipes and disabled floor targets are refused.
- Raised dispatches use `rule: "jev-scores-floor"`, with a neighboring `floors` array naming recipes, tiers and role/answer causes. Answer causes include observed score and threshold. Normal picks remain `jev-scores`. Fixed exclusions and human pins retain precedence and bypass scoring/floors as before.
- Offline routing-eval supports optional `workflow` (default `lane`) and applies both kinds of floors. Output preserves workflow for replay.
- Small related fix: explicit endpoint size refusals use the highest route tier (or a higher role floor). HTTP 413 and HTTP 400/422 with JSON `max_tokens_exceeded` are typed size errors. Dispatch records `rule: "jev-size-fallback"` and a redacted `fallback` cause, with no fabricated assessment. Existing escalation bounds, policy-race checks, exclusions and placement/readiness checks remain intact. Authentication, service, transport and malformed-response errors still refuse.

## Coordinator installation

No installation, live policy, credentials or project memory was changed. After installing the binary, add to the live routing policy as decided:

```json
"role_floors": { "reviewer": "pi_codex_sol_high" }
```

For the answer veto, add an entry using the live policy's actual question ID and top criterion index, for example:

```json
"answer_floors": [
  { "question": "blast_radius", "min_score": 3, "recipe": "pi_codex_sol_high" }
]
```

That question name/index is an example from the shipped fixture, not a proposed edit to Rolf's current questions. The target needs an enabled merged recipe and a model card with its capability tier. Floors can name a model outside the score bands; escalation still searches strictly higher tiers in the route ladder.

## Boundaries

The local 256 KiB request guard is unchanged: a full brief that cannot fit locally still refuses without an endpoint call. The new fallback specifically handles the reported picker/server refusal, without widening all failed requests into expensive dispatches. No live Jev call was needed or made; HTTP behavior is covered by fake endpoint responses. No further round is needed for the reported explicit `max_tokens_exceeded` response; broader fallback behavior would be a separate policy choice.

## Gates

All cargo gates used `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`, with the isolated lane target directory.

- `cargo fmt --check` — PASS.
- `cargo test` — PASS, 686 tests (533 main, 56 pi, 78 pro, 19 integration).
- `cargo clippy --all-targets -- -D warnings` — PASS.
- `git diff --check` — PASS.

Tests include automatic/manual reviewers, unchanged unconfigured behavior, inclusive answer thresholds with fractional scores, strongest combined floor, equal-tier non-pinning, higher-band retention, escalation above a floor and exhaustion, off-route floor targets, disabled/missing targets, malformed conditions, ledger provenance, exclusion precedence, offline replay, CLI help, explicit size fallback and unrelated-error refusal.

Only this lane branch is published to the URL-matched origin remote for the cloud completion gate; no integration or main branch is pushed.
````

