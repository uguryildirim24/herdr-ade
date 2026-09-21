+++
verdict = "MERGE"
round = "r55"
candidate = "eb67e960d40ea7789e0a5c43cacf3828001430df"
manifest_hash = "252b694089500e5fcae794393f5fe50e0155c254c15e6d505aa4f2ad9fc08e4e"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# r55 review

## t-0119 — MERGE

Merged the pinned sha `1d912780b37a66452c423d25a8d6236d3b87b247` (not the branch name)
into this review lane. No code fixes were needed; the candidate C is the merge commit
`eb67e960d40ea7789e0a5c43cacf3828001430df`.

The round adds an optional routing floor that raises a dispatch after ordinary scoring,
confidence and escalation, without becoming a pin:

- `role_floors` and `answer_floors` are optional and no floors ship. `Policy::validate`
  requires each floor target to have a model card, and `validate_recipes` refuses a
  missing or disabled floor recipe the same way it refuses a bad route.
- `apply_floors` raises only to a strictly higher tier, keeps an equal-or-higher scored
  pick, records every triggered floor above that pick, and lets the strongest tier win.
  A floor never lowers a decision and never pins one.
- Failure escalation still searches the route ladder for a strictly higher tier, so
  `escalation_exhausted` keeps its meaning above a floor.
- A raised pick records `rule = "jev-scores-floor"` beside a `floors` array naming the
  recipe, tier and cause (role, or answer with the question, raw threshold and observed
  score). Ordinary picks remain `jev-scores`; exclusions and human pins still bypass both.
- The reviewer floor reaches both paths: `round advance` and an ordinary
  `thread start --workflow reviewer` pass the same workflow into `resolve_launch`, and
  the stale CLI help line is corrected.
- The explicit endpoint size refusal (HTTP 413, or 400/422 with a JSON
  `max_tokens_exceeded` code) now falls back to the top route tier, or a strictly higher
  role floor, records `rule = "jev-size-fallback"` with a redacted cause, and stores no
  invented assessment. The local 256 KiB guard and every non-size failure still refuse.
- `routing-eval` replays the optional `workflow` (default `lane`) and applies both kinds
  of floors, so offline tuning matches dispatch.

## Checks run on oci

The round lists no gates. Independently ran the lane's four gates in the merged tree with
`CARGO_TARGET_DIR=/home/ubuntu/build/lanes/adeherdr-t-0122`:

```
$ cargo fmt --check
(no output; exit 0)

$ git diff --check
(no output; exit 0)

$ cargo test
     Running unittests src/main.rs              533 passed; 0 failed
     Running unittests src/bin/herdr-pi.rs       56 passed; 0 failed
     Running unittests src/bin/herdr-pro.rs      78 passed; 0 failed
     Running tests/cli.rs                         6 passed; 0 failed
     Running tests/context_actionable.rs          8 passed; 0 failed
     Running tests/context_records.rs             2 passed; 0 failed
     Running tests/routing_cli.rs                 3 passed; 0 failed
(total 686 passed; 0 failed; exit 0)

$ cargo clippy --all-targets -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 20.34s
(exit 0)
```

This review does not make the coordinator's post-merge policy edit; it only confirms the
mechanism. Only this lane branch is published to the URL-matched origin remote for the
cloud completion gate; no integration or main branch is pushed.
