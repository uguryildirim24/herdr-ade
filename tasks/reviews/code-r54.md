+++
verdict = "MERGE"
round = "r54"
candidate = "47a113eeba63258a07dc56b2468bc38a06202df8"
manifest_hash = "3f79436518d323a21edcf6cc5b58ef400e578046faad3c1a92597acc3cff8f12"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check", "git diff 79f3ae4..HEAD --check"]
+++

# r54 review

## t-0116 — MERGE

Merged pinned commit `ce295b5ada5566f3dfa13239da8f30457a2184ff` into this review lane. No code fixes needed.

`src/doctor.rs` now selects remote login/provider checks from the merged, enabled recipes, including configured overrides. Pi recipes check their provider once, without inferring a standalone CLI from a model name. Native recipes share placement's runtime probe definitions, and their executable requirements also drive the pane tools check. Disabled or absent recipes no longer leave login rows behind. Unknown kinds and inconsistent provider arguments fail closed.

Tests cover pi-only Codex readiness without native binaries, provider deduplication, each supported native runtime missing its binary, retired recipes, overrides through the report, and invalid readiness definitions. Existing reachability, capacity, wrapper and provider failure tests pass.

Claude and agy are correctly selected because enabled built-in recipes actually start those tools. Their previous unconditional inclusion was coincidental; now disabling or replacing those recipes removes their native checks. Claude runs `auth status`; agy retains the existing `models` readiness probe, not a dedicated authentication-status command. This review does not establish that `agy models` independently proves authentication.

## Checks run on oci

No gates were mandated in the review brief. Independently reran the lane's Rust checks with `PATH=/bin:$PATH`, `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, and `CARGO_TARGET_DIR=/home/ubuntu/build/lanes/adeherdr-t-0117`.

```text
$ cargo fmt --check
(no output)
exit: 0

$ cargo test
test result: ok. 524 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 28.12s
test result: ok. 56 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.81s
test result: ok. 78 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.82s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.79s
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.14s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
exit: 0

$ cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.12s
exit: 0

$ git diff --check
(no output)
exit: 0

$ git diff 79f3ae4..HEAD --check
(no output)
exit: 0
```

676 tests passed. No live full doctor run, installs, or credential changes.

An additional `git diff 11b613c..HEAD --check` exited 2, reporting trailing spaces on nine blank patch-context lines inside the pre-existing generated `tasks/t-0117.md` brief (last: line 737, `+ `). Those are not lane source changes; the range starting at the actual review-lane starting commit, `79f3ae4`, passes above. The generated input was left unchanged.

## Manifest verification limit

`ha --root /home/ubuntu/.herdr-ade round show adeherdr r54` could not read the round record:

```text
herdr-ade: round_manifest_unavailable: /home/ubuntu/.herdr-ade/adeherdr/.state/rounds/r54.toml cannot be read (No such file or directory (os error 2)); membership is not rebuilt from events
```

The cloud box has no canonical r54 record. The hashes above are copied from the committed brief, not independently confirmed against current coordinator state. No stale manifest was observed; the coordinator's checked merge must confirm freshness. The brief commit and pinned lane commit are ancestors of the candidate.
