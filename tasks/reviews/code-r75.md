+++
verdict = "MERGE"
round = "r75"
candidate = "402ad4961a898596f71811bf5f7e4b0d790ba635"
manifest_hash = "dddc98fce9728b722f06c494cfdb11dbea2c0243eff00041cbd550b1118f8550"
policy_hash = "8e43e0ebbd105ebf9f903667c622cb0dad8aa881e4ceb5a344db4659ff0338da"
gates = []
+++

# Review r75 repair

## Verdict

MERGE. The earlier candidate integrates with r74's retry, cancel, rebind, and adopt commands after one repair commit.

## t-0172

The typed failure work remains intact. The repair review fixed the overlap with r74:

- `thread retry` and a bound `round retry` now use the durable failure class. Provider, lost-connection, and gone-process recovery stay on the same recipe; failed work alone advances retries and fallbacks; unknown evidence refuses recovery without spending an attempt.
- A reviewer process that never appears stays on its existing thread and consumes one same-recipe retry. It no longer also increments the round's pre-binding start counter or allocates a fresh reviewer through untyped routing.
- Pending typed reviewer recovery is treated as live recovery, while exhausted or unknown failures stay bound and wait.
- A resolved reviewer with `cleanup_pending` is not considered gone, so rebind and adopt cannot place a second reviewer before the old target is known closed. The live gone check now matches the current `process gone:` row text.
- The merged code and instructions contain no calls to the removed `thread restart`, `round abandon`, or `round reviewer` verbs.

The pinned lane, current brief, and earlier reviewed candidate are ancestors of the candidate.

## Gates

`PATH=/bin:$PATH cargo fmt --check`

```text
(no output; exit 0)
```

`cargo test`

```text
test result: ok. 568 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 56 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 79 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
All integration suites passed (8, 8, 2, and 2 tests).
```

`cargo clippy --all-targets -- -D warnings`

```text
Compiling herdr-ade v0.1.0 (/home/ubuntu/projects/herdr-ade/.worktrees/t-0180)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 8.58s
```

`git diff --check`

```text
(no output; exit 0)
```
