+++
verdict = "MERGE"
round = "r39"
candidate = "7bf9881310ece376ccf7bac1f7de1b3479d5d439"
manifest_hash = "dae9ba2eef2102c685b173fe1d0cb4d9a75d258c8a295266fac286e21ce81e23"
policy_hash = "3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174"
gates = []
+++

# Round r39

MERGE. The manifest still names revision 1 and the single pinned lane. No project gates were listed.

## t-0086

The box now applies the same trailing slash and `.git` normalization as the Mac before comparing repository URLs. A mismatch names the wanted URL and every URL found on the box. I fixed the diagnostic to pass the wanted URL to `printf` as data; the lane's double-quoted message could evaluate shell syntax contained in a configured URL. The mismatch test now uses a hostile URL and proves it remains literal.

The installer fingerprints its running executable before replacing local binaries, then stops with `ha harness install` when the installed `herdr-ade` differs from the running image. I changed fingerprint capture from a silent optional check to a required result: failure to locate, resolve, or read the running executable now stops the install instead of silently restoring the second-run trap.

## Review checks

These were additional review checks, not configured round gates.

```text
$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
cargo fmt --check exit: 0
```

```text
$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --no-fail-fast

test result: ok. 468 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 56 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 78 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

```text
$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets -- -D warnings
    Checking ratatui-widgets v0.3.2
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.96s
```
