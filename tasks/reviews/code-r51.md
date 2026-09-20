+++
verdict = "MERGE"
round = "r51"
candidate = "ba29a021c66e24b0a543a1bb6925e2c1bb462de1"
manifest_hash = "403e92da9486e9e7cc24a612527c925be819a44cf50802ec69dffd064f6d707b"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = ["cargo fmt --check", "cargo test --locked", "cargo clippy --all-targets --locked -- -D warnings"]
+++

# r51 review

## t-0110 — MERGE

Reviewed and merged pinned commit `7b5974dfeb5a50dff674f6c3f6bdcb35f0f4b4cc`, not its branch. No blocking findings or review fixes.

The doctor now includes the dispatch default, explicit project and harness repository machines, box-path repositories paired with the dispatch default, enabled saved profiles, and unresolved remote threads. This covers both default and explicit placement without depending on current activity. Disabled saved profiles are excluded unless explicitly referenced; missing or disabled references still fail resolution. Machine-list errors fail the report rather than returning a healthy empty set. SSH failures now produce failing box rows.

Tests exercise an idle registered machine with no projects or threads, repository box-path placement, unreachable machines, and retained readiness failures. The agy login, native Claude/agy tools, and pi Pro checks remain visible. Removing standalone Codex from the tools requirement matches pi's in-process provider execution; the separate existing Codex login probe is unchanged.

## Checks run on oci

No gates were listed in the round brief. Independently ran the lane's three Rust gates, with:

```sh
export PATH="/bin:$PATH"
export DEVELOPER_DIR=/Library/Developer/CommandLineTools
export CARGO_TARGET_DIR=/home/ubuntu/projects/herdr-ade/.target/t-0111
```

`cargo fmt --check` — exit 0, no output.

`cargo test --locked` — exit 0; all 665 tests passed (513 main, 56 pi, 78 pro, 18 integration). Final lines:

```text
running 2 tests
test shipped_cases_run_offline_and_report_both_error_directions ... ok
test coordinator_cannot_select_a_role_recipe_or_model ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

`cargo clippy --all-targets --locked -- -D warnings` — exit 0. Final lines:

```text
    Checking ratatui-widgets v0.3.2
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 19.23s
```

No live Mac-to-box doctor run or installation was performed. This cloud checkout has the sealed brief but not the coordinator's canonical round record; hashes above are taken from that brief, with freshness enforced by the coordinator's merge check. The brief commit and pinned lane commit are both ancestors of the candidate.
