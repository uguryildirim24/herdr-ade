+++
verdict = "MERGE"
round = "r62"
candidate = "f7cde156e1467f163bcdf5e0595ee038272f1483"
manifest_hash = "b79f888a6e11b3dbe0bcbd9f092bee12e97851c0e3719cfe8c247e8352b90f43"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++

# Round r62 review

## t-0142 — MERGE

The doctor now recognizes an agentless `~` workspace with one tab and one pane as the machine's home workspace when another workspace exists. It omits that home workspace from both leak checks while a separate agentless workspace remains a failure. The workspace-list counts make the rule work on the Mac and saved machines without fetching local tab and pane lists.

Decision d-0029 accepts the one API limitation: `herdr workspace list` has no custom-label bit, so a workspace hand-labelled exactly `~` is indistinguishable. The repaired code comment now states that limit directly instead of claiming the label check can detect it. No fork change is requested.

No project gates were listed. Additional review checks on candidate `f7cde156e1467f163bcdf5e0595ee038272f1483`:

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(exit 0; no output)

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test
running 2 tests
test workflow_help_describes_policy_floor ... ok
test coordinator_cannot_select_a_role_recipe_or_model ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets -- -D warnings
    Checking ratatui-widgets v0.3.2
    Checking ratatui-crossterm v0.1.2
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.89s

$ git diff --check
(exit 0; no output)
```
