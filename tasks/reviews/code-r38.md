+++
verdict = "MERGE"
round = "r38"
candidate = "6dfa7f9f2a26273352b2b066525d52dde3efe863"
manifest_hash = "9dcadc893ac75050bdf0e82cba4b2ccbf5a51a0d28423ba81e817cfac24c3bb5"
policy_hash = "3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174"
gates = []
+++

# Round r38 review

## t-0081

MERGE after one review fix.

- The lane adds the append-only per-project failure ledger, folding repeated evidence under stable ids and exposing list, show, done and task commands.
- It records command, reviewer-start, merge, thread, launch, courier and retry failures without replacing the original operation result when ledger writing fails.
- Context and the project screen select at most five new or repeated open failures. The screen now actually renders its new Failures section; the lane had populated the projection but omitted it from the document.
- Closing an entry now clears retry state correctly while preserving another open detail for the same operation.
- Git predicates that return nonzero as a valid “no” answer no longer create false command failures. Timeouts, signals and real nonzero failures remain recorded.
- The r36 reviewer-start retry flow and this lane's recording hooks are integrated: each failed start folds into the ledger, retries are recorded, and recovery stops retry counting.

## Gates

The round listed no gates, so the verdict gate list is empty.

I also ran these review checks after the fix:

```text
$ cargo fmt --check
(no output; exit 0)

$ cargo test
running 5 tests
...
test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.32s

$ cargo clippy --all-targets -- -D warnings
Checking herdr-ade v0.1.0 (/home/agent/projects/herdr-ade/.worktrees/t-0090)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.82s

$ git diff --check HEAD^..HEAD
(no output; exit 0)
```

The full test command also passed 476 `herdr-ade`, 56 `herdr-pi`, 78 `herdr-pro`, and 5 CLI tests with no failures.
