+++
verdict = "MERGE"
round = "r75"
candidate = "34ec2f86bd506c952dd230a4c55e81207178e5b9"
manifest_hash = "dddc98fce9728b722f06c494cfdb11dbea2c0243eff00041cbd550b1118f8550"
policy_hash = "8e43e0ebbd105ebf9f903667c622cb0dad8aa881e4ceb5a344db4659ff0338da"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

# Review r75

## Verdict

MERGE. The typed failure records now keep missing evidence as unknown, keep provider and connection failures on the same recipe, and reserve fallback selection for failed work.

## t-0172

The lane supplied the five durable failure classes, class-driven projections and recovery, Pro stop handling, and ledger closure. Review fixes were needed before merge:

- A provider retry shared the failed-work counter, so the next work failure could switch models early. Separate same-recipe and failed-work counters now prevent that.
- A Pro completion with no answer and no failure event guessed provider failure. It now records unknown; an explicit stopped-thinking or stream error remains provider failure and never becomes a reply.
- Gone local and remote processes were labelled but not restarted. They now schedule bounded same-recipe replacement, while a whole missing session is left to session recovery rather than guessed to be many dead workers.
- Sustained machine loss now records connection lost and clears that evidence when the machine answers.
- Plain-language refusals were still ordinary errors and entered the ledger. They now carry the structural refusal marker.
- Herdr's structured `tab_not_found` and `pane_not_found` replies can arrive on stderr. The runner now treats those replies as completed cleanup, closes an older matching ledger entry, and creates no new one.

Historical launch records load the new retry counter as zero. The pinned lane commit and review brief are ancestors of the candidate.

## Gates

`cargo fmt --check`

```text
(no output; exit 0)
```

`cargo test`

```text
running 2 tests
test workflow_help_describes_routing_match ... ok
test coordinator_cannot_select_a_role_recipe_or_model ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
```

The same command's main suite reported 569 passed, the Pi suite 56 passed, the Pro suite 79 passed, and all integration suites passed.

`cargo clippy --all-targets -- -D warnings`

```text
Compiling herdr-ade v0.1.0 (/home/ubuntu/projects/herdr-ade/.worktrees/t-0177)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 11.43s
```

`git diff --check`

```text
(no output; exit 0)
```
