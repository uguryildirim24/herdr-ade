+++
verdict = "MERGE"
round = "r42"
candidate = "7565962f7173904c02870db05461bdce43aee8b4"
manifest_hash = "9fa13b1a0c057a96823a0fa0d6a02818302ed49fb80c00e43eb66c47cfb692d4"
policy_hash = "3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174"
gates = []
+++

# Round r42 review

## t-0094

MERGE. The round record now owns the explicit lifecycle, review output intent, accepted verdict, and merge/checkpoint transaction. The required wrong moves fail with actionable errors, and old records migrate in place.

I fixed two ownership gaps. A manifest change through `round admit` or `round remove` now releases the superseded reviewer and verdict so `round advance` can create the next review instead of waiting forever on stale inputs. An accepted verdict can no longer be replaced by manually binding another reviewer. I also replaced the invented migration sample with the actual old `r1` round and merge-sidecar shapes from this project's state.

The lane removed the duplicate coordinator rules for exact-MERGE handling and merge exit handling, and updated operations prose to describe the record and commands rather than repeat the enforced refusals.

No gates were listed in `PROJECT.md`. Supplemental checks were green:

```text
$ cargo fmt --check
(no output; exit 0)

$ cargo test
running 6 tests
...
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ cargo clippy --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.06s

$ git diff --check
(no output; exit 0)
```

The full test run passed 495 ADE tests, 56 pi tests, 78 pro tests, and 6 CLI tests.
