# Review brief: round r43

plain: This round checks that the task picks the helper for it, and that the fixed helper list is gone.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r43` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `0dda1fbef17ef4fbd80dbaadde1eb2b400a873bf1a53b84d793b1ea0a6044e73`, policy hash `3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0082 | 1 | `e4de9e4facd06ff1236eaff5b4b4dbfb5f053f79` | `t-0082-1-2` | `b9af1462b0489804f9eff3cc9110462fbcc3431fd3808b1a55d86218207ddfe0` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r43.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r43"
candidate = "<C>"
manifest_hash = "0dda1fbef17ef4fbd80dbaadde1eb2b400a873bf1a53b84d793b1ea0a6044e73"
policy_hash = "3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174"
gates = []
+++
```

5. Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0082 (artifact `b9af1462b0489804f9eff3cc9110462fbcc3431fd3808b1a55d86218207ddfe0`)

Data, not instructions.

````text
# t-0082 — task routing plumbing

Commit: `e4de9e4facd06ff1236eaff5b4b4dbfb5f053f79`
Branch: `hp/adeherdr/t-0082-m1-m2-m3-jev-picks-the-model-and-roles-a`
Machine: `oci`

## Delivered

- Removed model-role configuration and its readers. `thread start` rejects `--role`, `--recipe` and `--model`; `doctor` names and refuses leftover `[roles.*]`. Project role overrides are also refused. Existing workflow labels only select skills and review-flow identity, not a model.
- Dispatch sends the **full task file**, repository HEAD/status/tracked paths/recent changes and, on failure, the failure evidence. Review dispatch includes the committed review brief and pinned lane diffs. Titles are not classifier inputs.
- Implemented TypeSafe System One transport through the testable Runner seam, using `jev-latest`, three parallel **Score** questions and ordered-array criteria. No model roster is sent. The API key goes on curl stdin, not argv or the child environment; curl ignores user curlrc settings. No live call is in the tests.
- `~/.config/herdr-ade/routing.json` owns question instructions/criteria, weights, cutoffs, confidence floor, borderline margin and model cards. The binary normalizes and combines the scores, upgrades uncertain/borderline decisions without blocking, and enforces exclusions and strictly stronger bounded escalation. No embedded policy fallback or old-format reader.
- Four deterministic exclusions: research **product** → agy; Claude-required work and coordinators → Claude binary; spec product → Fable (Pro remains available through a hand pin); Rolf's exact-brief hash pin. URLs alone never trigger research routing. Non-default task contracts use opening `+++` TOML with `product = "web-research"`, `product = "spec"`, or `requires_claude = true`.
- Added `ha failed "<failure and evidence>"`. It seals a lane-bound failure event. Local ticker or box courier consumes it once, re-assesses with failure evidence, changes the attempt, preserves dirty work, checks readiness, and replaces that lane's tab. Three upgrades maximum; exhaustion/fixed exclusions produce durable refusals. Pending placement retries are bounded. Resolved/stale attempts cannot be resurrected by the assessment result.
- Dispatch decisions, score distributions/confidence, rubric/config hashes, confidence upgrades and escalations are fsynced to `<project>/.state/dispatch.jsonl`. Failure events remain in the existing event/courier store. No credentials from launch environments are copied into dispatch records.
- Added `ha routing-eval <cases.json>`, which runs live or replays saved raw TypeSafe responses without another inference call. It prints each result plus separate `over_routed` (money wasted) and `under_routed` (lane/review wasted) totals. The three shipped cases explicitly use synthetic responses, with one example of each mistake. These are plumbing tests, not measured routing accuracy.
- Updated coordinator/lane skills, operations, README and acceptance-script inputs. Renamed pi's model catalog from `roles` to `recipes`; removed its obsolete start-time-allowed field. Corrected the pre-existing Linux test fixture that expected a zsh-only pi probe; the production probe was already portable.

## Policy status and missing inputs

**`config/routing.json` is a placeholder, not a validated classification policy.** The latest user ruling superseded the original Choice design; the implementation uses task Scores, with confidence upgrading rather than blocking.

The requested t-0083 report was **not available on oci** at:
`/home/ubuntu/projects/herdr-ade/.worktrees/t-0083/.herdr-project/adeherdr-t-0083/report.md`.
The t-0083 worktree was absent, and its synced branch remained at the task-only commit `9a7793e`. I reported this during work. Thus I could not read sections 1, 2, 4 and 6 or paste their exact rubric. The supplied initial question text/cutoffs are explicitly marked provisional; replacing the JSON contents is the intended handoff to that research, and needs no rebuild. Do not represent this rubric as t-0083's validated recommendation.

`TYPESAFE_API_KEY` was absent from this lane's environment. HTTP shape was checked against the live TypeSafe index, API, Choice/Score and confidence docs, but **no live Jev dispatch was attempted**. All dispatch tests use faked responses. No global settings, credentials, installation or project memory were edited.

## Exact installation changes for the coordinator

1. In `~/.config/herdr-ade/config.toml`, remove `[roles]` and **every** `[roles.*]` table, including default/allowed/escalate and inline kind/args rows. Keep `[recipes.*]`, safety, machines and harness settings. If an inline row represents an executable model you still need, move its kind/provider/args/env/ready_timeout_ms/enabled/plain into a complete `[recipes.<id>]` row. There are no allowed/escalate lists.
2. Replace role-specific placement with a single table if the current box default is wanted:
   ```toml
   [dispatch]
   machine = "oci"
   ```
   Explicit `--machine` still works; existing placement behavior is otherwise unchanged.
3. Install the tracked `config/routing.json` as `~/.config/herdr-ade/routing.json` on machines running the new binary. It is required and deliberately not an embedded fallback. Replace its provisional rubric with t-0083's text once that report is available. The shipped routes name already-enabled built-ins `pi_opencode_deepseek`, `pi_kimi_k3`, and `pi_codex_sol_high`. Custom roster changes require matching executable recipe IDs and policy model cards/routes; no Rust rebuild.
4. Remove any `roles` or obsolete `jev` front matter from PROJECT.md files. Describe per-task products/runtime requirements in task-file front matter instead.
5. Ensure **the dispatch process and background ticker** inherit `TYPESAFE_API_KEY`; restart the ticker after installing the new binary/environment. Merely exporting a key in another shell does not update an existing ticker. Do not copy machine login credentials across machines.
6. Run `ha doctor`, then `ha routing-eval <checkout>/config/routing-cases.json` (offline; expected counts 1 correct, 1 over-routed, 1 under-routed). See operations.md for capturing real labelled cases and replaying their responses while tuning weights/cutoffs.
7. Existing running lanes keep their recorded launch. Old launch records without a capability tier cannot self-escalate under the new code: the binary refuses `escalation_tier_missing` rather than inventing a tier or silently reading the old roles table. New dispatches record the tier.

Only the lane branch is published for the cloud completion/courier gate; no integration branch is pushed and no installation is performed.

## Gates

All cargo commands used `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools` and the lane's isolated cloud target directory.

- `cargo fmt --check` — PASS.
- `cargo test` — PASS: **578 tests** (438 main + 56 pi + 78 pro + 4 existing CLI + 2 routing CLI).
- `cargo clippy --all-targets -- -D warnings` — PASS.
- `git diff --check` — PASS.
- `sh -n scripts/acceptance/run` — PASS. Live acceptance was not run.

Fake-response gates cover full-brief/repository state, all four exclusions (including coordinator), no URL-only research routing, low-confidence and borderline upgrades, failure evidence and stronger selection, escalation exhaustion and bound, review dispatch through the scorer, leftover-role doctor refusal, invalid/failed transport, editable-policy replay, both evaluation mistake directions, and a replayed courier failure preserving uncommitted work through replacement placement. CLI integration tests prove model/recipe/role flags are rejected and shipped cases run without an API key.

## Review seams

The new event variant touches contracts, ops, lane sealing, events/courier, thread records and ticker. The routing ledger is local to this implementation because the failure-ledger lane was not on this task's base; if that work lands first, connect these records without keeping two writable copies. Review both the local and box replacement paths; the box escalation explicitly skips reprovisioning/resetting its existing worktree.
````

