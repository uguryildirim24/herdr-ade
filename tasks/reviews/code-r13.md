+++
verdict = "MERGE"
round = "r13"
candidate = "3bd71580c3da8a6d74e7fd40a9a7ed3e864dbf5f"
manifest_hash = "6c356fff6f453541ef3fff5350ee04d25f36157fcb4d78c945b177086bdf9602"
policy_hash = "84e2f6b7857ba3fd723fb6e4436f922c8185c08a2f7b750813154c1d08ab027e"
gates = []
+++

# Round r13 review

## Verdict

MERGE. The Pro lane now waits for the rollout, trust prompt, blocked agent, dead Codex process, or the three-minute outer bound. The timeout messages report that bound.

## t-0028

The event polling and fake-clock coverage are sound. A rollout after the old 30-second limit succeeds; trust and death stop immediately; no rollout reaches the outer bound.

I added one review commit, `review(pro): keep the rollout bound local`. It removes an unused `--ready-timeout-ms` option and lane-record field that were described as recipe plumbing but were not connected to any recipe. `herdr-pro start` is a direct launcher, so it has no ADE recipe at this boundary. Start and resume use the required 180-second bound without adding a dead public seam or touching turn behavior.

## Checks

The round listed no project gates. I ran the focused formatter and Pro lane tests for this defect:

```text
$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(no output; exit 0)

$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked pro::lane::tests -- --nocapture
running 16 tests
...
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 53 filtered out; finished in 0.05s
...
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out; finished in 0.00s
```
