+++
verdict = "MERGE"
round = "r86"
candidate = "aab66ab9056399cb19c27b3d6565af1ee4278eed"
manifest_hash = "70e8d96f748f4a6fd14ac7ddc5b29578852fac2a3041072b4c0d2b4de642d419"
policy_hash = "9698216e305f216fae9b6ee0c22dc18d3ae6746e02ba7fcb779887a8dcd5c01f"
gates = []
+++

# Round r86 review

## Verdict

MERGE. The pinned lane fixes both observed Codex 0.155.1 startup failures without changing the persisted lane schema or the established path for lanes that already have a rollout.

## t-0198

- `start` and `resume` now require Herdr's ready state and still reject blocked agents and the visible trust prompt, but they no longer require a rollout before returning.
- The first turn snapshots an existing rollout at its end as before. If there is no rollout, it sends the packet load first, waits for the new file, opens it at byte zero, and persists the discovered path and session id. Later turns continue from the end of the existing rollout.
- The rollout wait ends on a found file, a blocked agent, a gone agent/process, or the 30-second load bound. Missing evidence is recorded as an `unknown` failed turn rather than a provider or work failure.
- The shell-start retry is limited to errors containing Herdr's `agent_pane_busy` code and remains bounded by the 120-second startup deadline. Other errors return immediately.
- Historical records still deserialize because no lane field or serde default changed.

No review fix was needed. A live Pro bridge check was not possible on `oci`; the live Codex lane remains an installation check on the Mac.

## Checks

All requested checks exited 0.

### `cargo fmt --check`

```text
exit 0
(no output)
```

### `cargo test`

```text
exit 0
test result: ok. 607 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 37.08s
test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.81s
test result: ok. 78 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.82s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.88s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.14s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

### `cargo clippy --all-targets -- -D warnings`

```text
exit 0
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 23.17s
```

### `git diff --check`

```text
exit 0
(no output)
```
