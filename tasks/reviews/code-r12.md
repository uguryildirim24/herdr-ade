+++
verdict = "REJECT"
round = "r12"
candidate = "31bcc5b0cd52d19355fb3899a464367f230896cc"
manifest_hash = "84dedd3694c4eb1ba39d5c9acfe5dc6e8e866145179f606fe97f7e8699576011"
policy_hash = "e1529634be8dfdf8688cad0bfcfd99dfa67f7f19fe2d1c3c7ab9f12ee7c0eec3"
gates = []
+++

# Round r12 review

## t-0023 — box completion courier

REJECT. The hash-checked create-only event and artifact import is a useful base, but the completed work does not satisfy the completion contract yet.

What must change:

1. Make one courier pass serve one saved machine across all projects. Today `tick_slow` calls the courier inside each project while `Memory::machine_is_due` is keyed only by machine. The first project advances that shared cadence on every due tick, so another project using the same box never runs its courier. The same pass also calls `herdr --machine` for `agent list` and `pane list` before the multiplexed helper, paying the second SSH bridge that §4.3 explicitly removes. The box-local helper must return the live lists and distribute one answer to every project using that machine.
2. Bring the lane commit home before importing DONE. No courier path calls `remote_for_url`, fetches the lane branch, or verifies `FETCH_HEAD` against `done.sha`; that helper is used only while starting a lane. A DONE event can therefore be delivered while the Mac does not have the pinned commit. Fetch from the configured URL-matched remote and verify the exact sha before the canonical event and cursor advance.
3. Complete D5 recovery and receipt checking. The helper scans every event on every pass rather than answering after the taken cursor, does not recover box X1/X2/X2b operations, and carries neither bootstrap receipts nor box receipt hashes. Event and artifact hashes alone do not establish the required matching box/Mac receipt.
4. Implement the required box pane probes and machine visibility. Doctor currently checks only whether four executable paths exist over plain SSH; it does not create/read a pane shell, verify the first `pi` hit, or prove the logins. `board::compute` explicitly drops every remote thread, and normal completion summaries do not name the machine.

Review fixes retained:

- Box readiness now resolves only an enabled, complete saved-machine profile; the config fallback and its dead helpers are removed.
- BLOCKED/GONE transitions are no longer marked delivered when the coordinator writer is suspended or unavailable; they remain pending for retry.

## Formal gates

The brief lists no gates.

## Additional checks

These are inspection checks, not entries in the round's empty gate list.

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(no output; exit 0)

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets --locked -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.72s

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked
running 4 tests
...
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.26s

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo build --release --locked
    Finished `release` profile [optimized] target(s) in 13.76s
```

No real SSH or box command was run.
