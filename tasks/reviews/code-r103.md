+++
verdict = "MERGE"
round = "r103"
candidate = "da2348021d5a564574bb9bbfa89a9cb740768b10"
manifest_hash = "d4a97f38517c5546f72eea97d67440cfaa770416cf974dcd3a25e85d9c42665e"
policy_hash = "b0854bd6e2b014fabc7e40bfde1e4ee7fa636e894571a68daae3999272c7ef0e"
gates = []
+++

# Review r103

## t-0266

MERGE. The stop request remains durable when the old ticker's pass exceeds the wait. The initialized replacement does no work before taking the file lock, waits without a lock deadline after release, and publishes its build only after its first pass. Before release, it has a bounded wait and removes its marker if the installer disappears; after release, it is the intended successor rather than an orphan. Local and box proofs both render and parse `pending handoff` while the marker exists. I found no review fix to make.

## Gates

- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check` — exit 0; no output.
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test` — exit 0. Final lines:
  ```text
  test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
  ```
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets -- -D warnings` — exit 0. Final line:
  ```text
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 25.33s
  ```
- `git diff --check` — exit 0; no output.
