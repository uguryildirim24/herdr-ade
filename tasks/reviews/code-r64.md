+++
verdict = "MERGE"
round = "r64"
candidate = "98544dbf115c3c9f47d0c1a03817b2bd61f62569"
manifest_hash = "a7a64cadb1dbd3818239d266c4061e3e66fc80fcc33c6bbd75927785c98a5b5e"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++

# Review r64

MERGE. All four pinned lane commits and the brief commit are ancestors of the candidate. The manifest still names exactly the four pinned lanes in revision 4.

## t-0129 — the talk tab stays current and readable

The screen now replaces a stale running build, carries the draft, removes closed work and duplicate work rows, uses task and ask records for its header, gives questions enough room, reports useful cost or running time, folds duplicate announcements, and consumes split mouse reports. The remote server check now runs the box's own `herdr` and reports an unreadable check as unknown.

I fixed one review defect: the first handoff's guard remained in the replacement process forever, so every later install was ignored. The guard now records the exact build already attempted. It blocks only a loop to that build and permits a later installed build to take over.

## t-0131 — a hand-started review is a reviewer

`round reviewer <slug> <round>` now creates and binds the reviewer through the reviewer workflow, so the reviewer skill and model floor are present. The automatic placement path already falls back from an unready box to this Mac.

I fixed one integration race: the manual command and the ticker could both observe an empty reviewer binding and start separate reviewers. The manual path now holds the same advance lock as the automatic path.

## t-0134 — the coordinator remains reachable

The ticker and `open` restore the recorded name on an otherwise matching coordinator pane, and the durable coordinator record counts each repair. The doctor checks that the name resolves and detects an announced inbox set that remains unread. Prompting is now the default, while an explicit `nudge = false` remains effective.

The doctor test conflict was combined rather than choosing either lane: the newer lane-worker routing test and both coordinator reachability tests remain. I removed the conflict marker left by the mechanical merge.

## t-0135 — Rolf's words can be cited

The supported Claude coordinator now installs a prompt-submit hook beside its end-of-turn hook. Direct pane messages enter the existing talk journal verbatim under request ids; talk deliveries reuse their existing ids; harness-generated prompts are marked and excluded. Context lists recent ids, and consequential decisions continue to refuse an absent or unknown basis.

## Supplemental gates requested by the coordinator

The brief listed no gates, so the verdict front matter remains `gates = []`. I nevertheless ran the four candidate checks requested after the review began.

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(clean; no output)

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test
running 2 tests
test workflow_help_describes_policy_floor ... ok
test coordinator_cannot_select_a_role_recipe_or_model ... ok

test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets -- -D warnings
Compiling herdr-ade v0.1.0 (/home/agent/projects/herdr-ade/.worktrees/t-0148)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.59s

$ git diff --check
(clean; no output)
```

The full test run passed 563 main tests, 56 `herdr-pi` tests, 79 `herdr-pro` tests, and the 6 + 8 + 2 + 2 integration tests, with no failures.
