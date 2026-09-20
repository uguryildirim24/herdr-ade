+++
verdict = "MERGE"
round = "r52"
candidate = "e749017bb034fb425b78659fba61b915e948956d"
manifest_hash = "b2c064a83bde8c69d3d94a5a9f5c70099b7cefe3837216dec65f74955313dc3c"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = ["PATH=/bin:$PATH cargo fmt --check", "PATH=/bin:$PATH cargo test", "PATH=/bin:$PATH cargo clippy --all-targets -- -D warnings", "git diff --check", "git diff --check a485e93 HEAD"]
+++

# Round r52 review

## t-0112 — MERGE

Merged pinned commit `657256fee7ba3931856c43ecab4f75b5fefc7c6a`.
No blocking findings or review fixes.

Placement now follows recipe selection and checks readiness before allocating a
thread, tab, or worktree. Native checks share the doctor's runtime probes; box
checks use the lane PATH. Provider checks remain on the candidate machine.
Explicit machine choices never move silently. Default-box failure checks local
readiness, and refusal names every candidate and its missing capability.
Successful placement records the machine, attempts, and reason in the dispatch
ledger. Tests cover research fallback, ready provider placement, explicit
refusal without a thread, and diagnostics when neither candidate is ready.

The pinned lane and brief commit `cf632d05d5e41a7bae595178060df97c6b7ca59d`
are ancestors of the candidate.

## Checks on oci

The review brief lists no required gates. These additional checks all passed:

```text
$ PATH=/bin:$PATH cargo fmt --check
(no output; exit 0)

$ PATH=/bin:$PATH cargo test
running 2 tests
test shipped_cases_run_offline_and_report_both_error_directions ... ok
test coordinator_cannot_select_a_role_recipe_or_model ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

Full suite: 517 main, 56 pi, 78 Pro, and 18 integration tests; 669 passed,
none failed.

```text
$ PATH=/bin:$PATH cargo clippy --all-targets -- -D warnings
    Checking ratatui-crossterm v0.1.2
    Checking ratatui-widgets v0.3.2
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 19.50s

$ git diff --check
(no output; exit 0)

$ git diff --check a485e93 HEAD
(no output; exit 0)
```

No live authenticated launch was performed. The cloud mirror has no
`.state/rounds/r52.toml`: `ha --root /home/ubuntu/.herdr-ade round show adeherdr r52`
returned `round_manifest_unavailable`. Hashes above come from the committed
brief; current-manifest validation remains the coordinator's merge gate.
