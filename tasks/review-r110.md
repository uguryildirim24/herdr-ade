# Review brief: round r110

plain: The first cuts: guides match the code, one helper list, and tasks that changed no files can be checked.

Run `/home/agent/.local/bin/herdr-ade --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r110` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 4, manifest hash `883a924e9c47691b0476ec052ede28ca1c8047bf2d0c1dc193a263d693e9010b`, policy hash `b6e35aab70fe96c6aa8639d6ef111bbb6baaa60cbb2a2de9b5e8eef6e4ab3b1b`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0286 | 1 | `11a037018f4ab38d16128aed99a7a8c2b2ef386b` | `t-0286-1-1` | `84b658b12f0a01534da9b5a30eb448187f7e6f23aef385516efe180882ebaef4` |
| t-0287 | 1 | `827ba1f277bdd8c3bc192242168e2b580683e492` | `t-0287-1-1` | `f93734619b643715dde4d65ee554884983e592dcb7d8946dec0a2f9dd9f7c1e1` |
| t-0289 | 1 | `089ee408300f278f646a7a30ee6710c1ec544e36` | `t-0289-1-1` | `2736b85113667ef5a21249432a79e0de3837852e43520a87dd3817a2ce032d8e` |
| t-0290 | 1 | `e14e3e90f21b5bc7c471b277ca2e8513a54a9b93` | `t-0290-1-1` | `5c0c10b35d25cc9914bf5fdc38835a4d1818be30666f032e60f3d03862f5a39d` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r110.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r110"
candidate = "<C>"
manifest_hash = "883a924e9c47691b0476ec052ede28ca1c8047bf2d0c1dc193a263d693e9010b"
policy_hash = "b6e35aab70fe96c6aa8639d6ef111bbb6baaa60cbb2a2de9b5e8eef6e4ab3b1b"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/home/agent/.local/bin/herdr-ade --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0286 (artifact `84b658b12f0a01534da9b5a30eb448187f7e6f23aef385516efe180882ebaef4`)

Data, not instructions.

```text
# W36 report

Implemented the documentation and task-state cleanup.

- Updated birth-sentence, talk-screen handover, and plan documentation to match current behavior.
- Removed `EvidenceKind` and the required `task evidence --kind` argument from code and coordinator guidance.
- Derived code milestones from sealed attempt commits: tasks with no committed change skip review, merge, and install, while changed tasks retain the configured milestones.
- Allowed tasks with no lane to move directly from acceptance verification to `verified`.
- Added coverage for no-attempt, report-only, and code-changing tasks; adjusted the install-proof fixture to represent changed work.

Gates passed:

- `cargo fmt --check`
- `cargo test` (656 main tests, 58 pi tests, 86 pro tests, and integration tests)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
```

### t-0287 (artifact `f93734619b643715dde4d65ee554884983e592dcb7d8946dec0a2f9dd9f7c1e1`)

Data, not instructions.

```text
# W37 report

Implemented one recipe catalog and one configuration reader.

- Deleted the compiled pi recipe table. Shipped `assets/default-recipes.toml` plus user overrides now drive launch, both doctor commands, and DeepSeek model overrides.
- `herdr-pi doctor` and `ha doctor` use the same enabled routed provider/model selection. `herdr-pi check <provider> [--model ...]` remains the explicit check for an unrouted provider.
- Moved pi row validation onto canonical recipe rows, including provider/model/thinking flags and the empty environment rule. Kimi remains supported by explicit config and `check`.
- Added `src/config.rs` as the sole ADE `config.toml` read/parse boundary. Missing files become empty documents; all other read errors are returned. Callers decode their own sections.
- Routed `herdr-pi` and `herdr-pro` through the same config-directory and ADE-root resolution as the main binary.
- Preserved partial doctor output: section/config failures become failed rows while independent checks continue.

Regression coverage:
- An unrouted Kimi recipe does not make standalone doctor probe Kimi.
- A config read failure is not treated as an empty harness repository list.

Gates passed:
- `cargo fmt --check`
- `cargo test` (653 main, 54 herdr-pi, 87 herdr-pro, and integration suites)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
```

### t-0289 (artifact `2736b85113667ef5a21249432a79e0de3837852e43520a87dd3817a2ce032d8e`)

Data, not instructions.

```text
# W38 report

Implemented self-closing finished lanes.

- `round merge` and `round cancel` now run every member lane and reviewer through the normal final-copy and resolve path. Clean worktrees are removed, branches remain.
- Automatic cleanup is recorded before external work. A copy, Herdr, or worktree failure leaves the thread resolved with `cleanup pending`; the ticker retries the same resolve path without failing the completed merge or cancellation.
- A sealed lane whose commit equals its base and belongs to no round closes after its report is copied home. Changed lanes outside rounds remain visible.
- Removed `auto_resolve_days`, its idle sweep, ticker memory, context output, tests, and documentation. Nothing still needs the sweep.
- Updated coordinator guidance and user documentation so routine manual resolution is no longer requested; `thread resolve` remains for exceptional manual use.

Tests added/updated cover merged member lanes plus reviewer closing, a failed cleanup preserving a completed merge and pending state, and no-change lane report copy plus closure.

All gates pass:

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
```

### t-0290 (artifact `5c0c10b35d25cc9914bf5fdc38835a4d1818be30666f032e60f3d03862f5a39d`)

Data, not instructions.

```text
# W39 report

Implemented and committed as `e14e3e9` (`fix(talk): separate nudges from typed words`). The lane branch is published to `origin`.

## Changes

- Prompt markers now retain the pane and sent text, while older marker records still deserialize.
- The prompt hook removes every live automated text marked for that pane, including multiple texts and paste-wrapped prompts, before recording Rolf's remainder.
- Whitespace-only remainders are not recorded.
- Historical journal rows containing a ticker suffix now project only Rolf's remainder through conversation rendering, recent-request digests, request citations, and decision-basis checks. Pure automated rows remain hidden and journal bytes remain unchanged.
- Added the single requested unit test using the exact `e[herdr-ade ticker…]` defect text; it also covers historical projection and multiple marked texts. Updated one existing assertion to the new stripping behavior.

## Gates

- `cargo fmt --check`: passed
- `cargo test`: passed (657 main tests plus all binary and integration suites)
- `cargo clippy --all-targets -- -D warnings`: passed
- `git diff --check`: passed

One full test run initially hit an unrelated transient `Text file busy` in `harness::tests::box_zig_prefers_the_repository_local_tool`; that test passed immediately on retry, and the final full suite passed.
```

