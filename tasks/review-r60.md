# Review brief: round r60

plain: This round measures the part that picks a helper on what really happened, and makes the cloud box checks tell the truth.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r60` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 2, manifest hash `a2f1e638209c895afa1eb99784f14afd1dcad9c48a16771a921987a810f65726`, policy hash `73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0132 | 1 | `b0b581cd698b2a2673f6dd956693dcda4ac329cb` | `t-0132-1-1` | `fc7e07e048ba6362c5f0246eb18cf753b43403379bf6d6b19d61625e4d116886` |
| t-0133 | 1 | `12eea44f3bc78df8e65e45c4579390e6972a9979` | `t-0133-1-1` | `96c2c903b607c2389435a79e877e47ca10836b7b694c609d6c8f43dbc6d56193` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r60.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r60"
candidate = "<C>"
manifest_hash = "a2f1e638209c895afa1eb99784f14afd1dcad9c48a16771a921987a810f65726"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0132 (artifact `fc7e07e048ba6362c5f0246eb18cf753b43403379bf6d6b19d61625e4d116886`)

Data, not instructions.

````text
# t-0132 — measured, price-aware routing

## Delivered

- Replaced the three weighted 0–3 questions and fixed cutoffs with one Score, `required_index`. Its four levels map through `index_values` to real Artificial Analysis Coding Index units: `0.0`, `76.2`, `77.1`, `77.2`.
- Model cards now carry `coding_index` and blended `price_per_million`. Dispatch interpolates the Score into index units and chooses the lowest-price enabled model that clears it. Confidence below `0.50` raises to the cheapest capable higher tier; failures still require a strictly higher tier.
- The default measured candidates are Kimi K3 high (76.2, $5.40) and Sol high (77.2, $7.20). Astra high is deliberately not a candidate: its 77.1 at $18 is dominated by Sol. DeepSeek V4.1 Flash is not assigned a made-up Coding Index; the published data used by this project has no Coding Index for it.
- Kept r55’s constraints in the shipped policy: reviewers floor at Sol, and the irreversible/top answer (raw Score 3) floors at Sol.
- Removed `weights`, `routes`, `borderline_margin`, normalized cutoffs and the version-1 reader. Policy version 2 is the only accepted shape.

## Outcomes, not labels

`ha routing-eval <project>` now makes no Jev call and reads what happened:

- sealed DONE events define finished lanes;
- the thread launch gives the final model and escalation count;
- the matched `.state/dispatch.jsonl` row gives failure evidence and a saved assessment;
- the carrying round says whether it merged and, for newly tracked rounds, whether any REJECT occurred.

Each JSON outcome reports `model_run`, `escalated`, `failure`, `round`, `merged_without_reject`, saved assessment, current-policy selection, required index and result. A run is observed good enough only when it did not escalate and its round merged without a REJECT. A policy choice different from the model that actually ran is `untried`, never guessed correct or incorrect.

Round records now persist observed REJECT count. New rounds start at zero; both `round advance` and direct repair through `round review` record a REJECT once. Historical round TOML still loads. An old record from before this field reports an unknown round outcome rather than falsely claiming no REJECT.

Deleted `config/routing-history-cases.json`: its 90 `expected` values described where work was sent, not need. Deleted the synthetic `config/routing-cases.json` and the labelled-case evaluator/test that existed only for that format.

## Confidence measurement

The deleted 90-case cohort contained **zero saved TypeSafe responses**, as its own audit said; therefore the honest old-design count is **0 of 0 scored saved cases clear 0.65**, and the new-design count is **0 of 0 rescored saved cases clear 0.50**. No confidence comparison can be manufactured from model labels. I made no Jev calls; this box has no key and the brief forbids treating dispatch destinations as labels.

The new evaluator reports `saved_assessments` and `confidence_clear` for real dispatch rows as they accumulate. A scratch-root offline command against an empty project produced 0 finished lanes, 0 saved assessments and 0 clear assessments, confirming that it does not call Jev. The unit outcome fixture proves one completed Kimi-equivalent run with confidence 0.90 is joined to a merged, rejection-free round and confirmed good.

## Proposed live `routing.json`

Install this exact version-2 file as `~/.config/herdr-ade/routing.json` on each machine:

```json
{
  "version": 2,
  "note": "A task asks for a required Artificial Analysis Coding Index. Dispatch picks the cheapest measured model that clears it. Prices are blended dollars per million tokens. Astra high is not a candidate because Sol high has a 0.1 higher Coding Index at 40 percent of its price.",
  "model": "jev-latest",
  "questions": {
    "required_index": {
      "type": "score",
      "instructions": "Choose the lowest published Coding Index anchor sufficient to complete the full brief correctly. Judge the reasoning and verification needed, not length, filenames, model names, or dramatic wording. Use 0 for prescribed or local work; 76.2 for substantial coding that still follows established contracts; 77.1 for novel coupled design; and 77.2 only for the hardest subtle work or a change that is costly, durable, security-sensitive, or difficult to undo.",
      "criteria": [
        "0.0 index: prescribed, mechanical, local, or readily verified work",
        "76.2 index: substantial multi-file coding within established contracts",
        "77.1 index: novel design with coupled invariants or uncertain verification",
        "77.2 index: exceptionally subtle work, or an irreversible, security, durable-state, install, or cross-system risk"
      ]
    }
  },
  "index_values": [0.0, 76.2, 77.1, 77.2],
  "models": {
    "pi_kimi_k3": {
      "tier": 1,
      "coding_index": 76.2,
      "price_per_million": 5.4,
      "description": "Kimi K3 high: Coding Index 76.2 at $5.40 per million blended tokens."
    },
    "pi_codex_sol_high": {
      "tier": 2,
      "coding_index": 77.2,
      "price_per_million": 7.2,
      "description": "GPT Sol high: Coding Index 77.2 at $7.20 per million blended tokens."
    }
  },
  "confidence_floor": 0.5,
  "pins": {},
  "role_floors": {"reviewer": "pi_codex_sol_high"},
  "answer_floors": [
    {"question": "required_index", "min_score": 3, "recipe": "pi_codex_sol_high"}
  ]
}
```

## Verification

Required environment: `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`.

- `cargo fmt --check` — pass.
- `cargo test` — pass: 533 main, 55 pi, 78 pro, 18 integration tests.
- `cargo clippy --all-targets -- -D warnings` — pass.
- `git diff --check` — pass.
- `ha routing-eval` equivalent from this build against a scratch root — pass, offline, zero-record JSON as described above.

No live config, live ADE state, credentials, installation or project memory was read or changed. No throwaway pane and no Jev call were used.
````

### t-0133 (artifact `96c2c903b607c2389435a79e877e47ca10836b7b694c609d6c8f43dbc6d56193`)

Data, not instructions.

````text
# t-0133 report

Commit: `12eea44f3bc78df8e65e45c4579390e6972a9979`

## Result

- `ha harness install` now sends the coordinator's exact `RULES.md` bytes to the saved `oci` machine and writes a `.lane-worker` marker. It intentionally does **not** copy `config.toml` or `routing.json`; dispatch and Jev stay on the Mac coordinator.
- A marked lane worker's `ha doctor` skips coordinator-only recipe, Jev, and saved-machine checks. Pi installation and provider readiness remain checked because box lanes use them.
- Pi readiness now refreshes credentials and makes a tiny, no-tools, no-session print-mode call to one model for each provider. A stale credential now reports `stored sign-in no longer works ...` instead of `login ready`.
- Native Claude, Codex, and agy readiness probes now make their smallest real model calls instead of reading auth status/model lists.
- Successful and failed live answers are cached for the ticker's 15-second interval. Provider output and credentials are not stored.
- Box-side native probes use the same 15-second cache. No probe process or pane was left running.

## Defect tests

- `pi::doctor::tests::a_stale_ready_credential_fails_when_the_model_call_is_refused_and_is_cached`: an auth record says ready, the real Kimi call says `subscription expired`, the row fails plainly, and two checks issue only one real call.
- `doctor::tests::a_lane_worker_does_not_validate_coordinator_routing`: a marked box with no routing policy has no `routing_recipe_missing`/Jev failure and does not enumerate saved machines.
- `scenarios::harness_install_runs_the_box_steps_only_when_oci_is_saved`: install sends the exact RULES bytes and creates the worker marker after the two box builds.

## Gates

All passed with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check`
- `cargo test` — 536 + 56 + 78 + 6 + 8 + 2 + 3 tests
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

## Doctor output

### Before — box (`oci`), actual installed binary

Captured before editing:

```text
[FAIL] this Mac workspaces: 3 of 9 hold no agent and belong to no open lane: w1, w25, w26
[FAIL] recipes: routing_recipe_missing: pi_codex_astra_high
[FAIL] Jev: TYPESAFE_API_KEY must be set in the dispatch process environment
[warn] ticker: not running
[ok  ] provider opencode-go: login ready
[ok  ] provider openai-codex: login ready
[ok  ] provider kimi-coding: login ready
[ok  ] provider pro: not on this machine (no relay)
```

The complete captured relevant output is in `library/doctor-before-box.txt`.

### Before — Mac

The task's recorded Mac output was:

```text
[warn] box oci rules: no generated RULES.md recorded
[ok  ] provider kimi-coding: login ready
```

The box lane cannot execute the Mac binary or read the Mac's live configuration.

### After — box candidate, isolated scratch root

The candidate was run with a scratch HOME, config directory, root, RULES file, and worker marker; it did not read or write live ADE settings:

```text
[ok  ] lane worker: RULES.md is local; dispatch recipes, Jev and saved-machine checks stay on the coordinator
```

There is no `recipes` or `Jev` row. The full isolated output is in `library/doctor-after-box-scratch.txt`; its unrelated pi rows fail because the scratch root deliberately has no pi installation or credentials.

### After — installed Mac and box

A true post-install pair cannot be produced in this lane: the brief forbids touching live `~/.herdr-ade`/`~/.config/herdr-ade`, lanes do not install, and this lane runs on `oci`, not the Mac. After merge, the coordinator should run `ha harness install` (twice if the stale-self refusal appears), then capture:

```text
# Mac
ha doctor

# box
ssh remote-host '~/.local/bin/herdr-ade --root ~/.herdr-ade doctor'
```

The Mac output should replace `no generated RULES.md recorded` with the RULES SHA. The box output should contain the `lane worker` row above and no recipe/Jev rows. The lapsed Kimi subscription should now be a plain failing live-call row rather than `login ready` on both machines until it is renewed or signed in again.

## Publication

Published branch:
`origin/hp/adeherdr/t-0133-the-box-has-its-settings-checks-ask-the`
````


## Repair revision

This revision reviews the integration base `5c6fd5ab9f197b7a9beb2b1ec78c2ec922693489`.
