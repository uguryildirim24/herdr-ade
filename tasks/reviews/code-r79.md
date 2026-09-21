+++
verdict = "MERGE"
round = "r79"
candidate = "ebd209ad67e3570346570bb606f03c300042c8ad"
manifest_hash = "41bd8b0578f8c1e6b564a86a2ab8fc8951a0fcbd74834b2c93c1cebac40976b3"
policy_hash = "8e43e0ebbd105ebf9f903667c622cb0dad8aa881e4ceb5a344db4659ff0338da"
gates = []
+++

# Review

## t-0186

MERGE. Remote resolution removes the lane build folder only through the worktree-removal path, with `git worktree remove && rm -rf`, after the in-use, completion, dirty-data and retained-data checks. Open threads protect their matching build folders from orphan reporting. The launch and cleanup paths use the same project-and-thread helper.

Doctor emits the disk and orphan findings through its typed check records. I fixed three missing-evidence cases in the repaired integration: an unavailable local disk reading and an unparseable remote disk reading are warnings rather than false low-disk failures, and unreadable project or thread state cannot turn a possibly owned build folder into a known orphan. A measured low-disk result still fails at the configured threshold, while a measured orphan still reports its size and fails.

The r78 task-record work remains intact. The full suite includes its task projection, unreadable-evidence, milestone and state tests, and all passed after the repair merge.

The round manifest records no gates, so the front matter remains `gates = []`. Rolf requested the following four checks for this repair review; all passed on the box.

## Checks run on the box

`cargo fmt --check`

```text
(no output; exit 0)
```

`cargo test`

```text
test result: ok. 591 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 79 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

`cargo clippy --all-targets -- -D warnings`

```text
Checking herdr-ade v0.1.0 (/home/ubuntu/projects/herdr-ade/.worktrees/t-0191)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 28.67s
```

`git diff --check`

```text
(no output; exit 0)
```
