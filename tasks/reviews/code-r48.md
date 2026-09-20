+++
verdict = "MERGE"
round = "r48"
candidate = "436ec1d07c1fa04c21b985d14cab2b3df000eb1e"
manifest_hash = "1a45301352b8f3555dfaf772069c2e706299b01876e49eca27b944c725c9c33e"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# r48 review — MERGE

plain: The rundown keeps unfinished work visible and counts finished work instead of listing it.

## t-0102

Merged the exact pin `76b2ba29877997cfa1e0cd84be977853a4203f71`. No blocking defect found. Closed rounds and resolved threads become counts; terminal preparations disappear without changing their records. Current preparations, unresolved failures, interrupted merges and divergence remain visible. The existing task parser preserves owners and delegation while omitting finished tasks. Lists have explicit overflow pointers. Inbox receipts and sealed-event acknowledgements follow only the displayed rows.

Review commit `436ec1d` adds two regression tests: an abandoned preparation without a successor leaves its current failed thread visible and its records unchanged; unreadable task bytes produce a diagnostic, not an empty queue. Existing tests cover merge divergence, current-attempt evidence, historical-row removal and bounds. No production-code correction was needed.

## Checks run on oci

The round brief lists no required gates. Independently ran the lane's gates on the merged candidate. Cargo used `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools` and `CARGO_TARGET_DIR=/home/ubuntu/build/lanes/adeherdr-t-0104`.

`cargo fmt --check` — exit 0, no output.

`cargo test` — exit 0, 650 tests passed across seven suites (498 + 56 + 78 + 6 + 8 + 2 + 2). Last lines:

```text
test coordinator_cannot_select_a_role_recipe_or_model ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

`cargo clippy --all-targets -- -D warnings` — exit 0. Last lines:

```text
   Compiling herdr-ade v0.1.0 (/home/ubuntu/projects/herdr-ade/.worktrees/t-0104)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 7.03s
```

`git diff --check` — exit 0, no output.

Full logs: `.herdr-project/adeherdr-t-0104/library/gates/`.

## Evidence and limits

Inspected the measurement script and checked the lane's saved raw outputs against its results file: cloud snapshot 2,321 → 1,424 bytes; history reconstruction 18,269 → 1,672 bytes. Also checked that the latter removes 44 merged rows, 79 abandonment rows and 46 resolved rows while preserving the open round, completion and three messages. These are verification of saved lane evidence, not a newly run before/after measurement. The reconstruction is explicitly modeled, not the unavailable canonical Mac snapshot.

The cloud checkout uses the dispatched lane branch, whose starting commit matches `review/r48`; both brief B and the pinned lane are ancestors of the candidate. The canonical Mac round record is not present on oci, so live manifest freshness could not be independently checked here. The verdict uses the dispatched brief's hashes; the coordinator's merge validation must check them against the canonical record. No integration ref was moved, and nothing was pushed or installed.
