+++
verdict = "MERGE"
round = "r91"
candidate = "abb53f650b6901d83e0c21a2351ca233894c3787"
manifest_hash = "08c1577a7ea71ee54a8a89ed360c4b9bea467e466453c84ba974727dec866be3"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# Code review r91

Verdict: **MERGE**.

## t-0224

The local build and atomic install loop now completes before dispatch parsing, saved-machine lookup, SSH work, settings copy, and process proofs. Installing `herdr-ade` invokes the production re-exec callback immediately; when its bytes differ, `exec` replaces the process and cannot return, so a later box-lookup failure is emitted by the newly installed image. The regression checks that the local build and re-exec boundary precede a failing machine lookup.

Local and box build evidence remains paired with the same repository records. Box commits are added only after each successful remote build, and task proof recording still runs after local installs, remote installs, settings, and process checks. The remote build script's commit marker and declared PATH handling are unchanged.

No review fix was needed.

## Gates

`cargo fmt --check` — exit 0

```text
(no output)
```

`cargo test` — exit 0

```text
test result: ok. 622 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 36.72s
test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.81s
test result: ok. 82 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.83s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.87s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

`cargo clippy --all-targets -- -D warnings` — exit 0

```text
Checking ratatui v0.30.2
Finished `dev` profile [unoptimized + debuginfo] target(s) in 24.73s
```

`git diff --check` — exit 0

```text
(no output)
```
