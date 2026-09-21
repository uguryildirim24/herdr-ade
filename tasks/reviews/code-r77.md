+++
verdict = "MERGE"
round = "r77"
candidate = "289f66bc9f75690caa146410f72ba41e22bbedca"
manifest_hash = "144437fa93168718c8c1816683c367291706919d91537498985043dba8b25d48"
policy_hash = "8e43e0ebbd105ebf9f903667c622cb0dad8aa881e4ceb5a344db4659ff0338da"
gates = []
+++

# Review

## t-0183

MERGE. The pinned change routes every ticker command through the projects root, starts the ticker and long-lived Pro helpers from stable state folders, records the ticker folder, and makes doctor reject a missing one. All ticker start and ensure callers converge on the corrected spawn path.

Pi readiness now carries provider or unknown evidence. Local command errors, malformed output, missing programs, and timeouts do not produce a login remedy. An unknown readiness failure reaches `fail_start` as unknown; recovery stops before selecting a retry or incrementing the task attempt, and the launch-attempt counter is incremented only after readiness succeeds.

The box worktree probe is one SSH command whose script exits successfully for both present and absent paths. A missing path is the healthy answer and creates no ledger failure. A transport failure or missing fact is added to the check errors, so the doctor status is unknown rather than healthy.

I fixed two review findings in `review(runtime): keep timeouts unknown and root ticker loop`:

- `ticker run` now moves the loop itself to the projects root, covering direct invocation as well as every detached start path.
- A timeout cannot be cached or classified as provider evidence even if partial output contains an authentication marker. An SSH transport exit also cannot become provider evidence.

## Gates run on the box

`cargo fmt --check`

```text
(no output; exit 0)
```

`cargo test`

```text
test result: ok. 582 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 79 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`cargo clippy --all-targets -- -D warnings`

```text
Checking ratatui-widgets v0.3.2
Checking ratatui-crossterm v0.1.2
Checking ratatui v0.30.2
Finished `dev` profile [unoptimized + debuginfo] target(s) in 29.17s
```

`git diff --check`

```text
(no output; exit 0)
```
