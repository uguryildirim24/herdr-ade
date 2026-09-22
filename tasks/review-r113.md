# Review brief: round r113

plain: The lead's page and briefs read the one page, starting work reuses the task, folders on use, and a fairer question check.

Run `ha skill reviewer`, then do what this brief says.

Round `r113` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 7, manifest hash `d97117f0daa0d28fd61f3dc7eedb57f05b1ec36c5d556bffedac7c05e1fb817a`, policy hash `7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0303 | 1 | `9b6c3eb561fd2352d84e4bb8577937dd07261854` | `t-0303-1-1` | `e26a0bf145333a6f12d1be6637e89fa7b6b904d6f92752d0325e6f0b04bceafb` |
| t-0304 | 1 | `76efbc9afcdc841b5551639fbc83e8e1171bf3ea` | `t-0304-1-1` | `355a8a14a659ac46e3ed04237ea151d1481e73e26ea1730bdc205a70e316a63d` |
| t-0305 | 1 | `505baae8a4f840a3a7d55a9654b4b28effa321b2` | `t-0305-1-1` | `6c28a6fd50d3063d0e28161a281440a9b1ed5e8fa74a341007659e4e41d8a3bc` |
| t-0306 | 1 | `d0dacafe5f2766d260e546f54b242d4a07d40b7d` | `t-0306-1-4` | `91d5f53c1957362675442eb9242ae5880bc841889c7cedeaa4e352e11cd5df57` |

## Gates

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above with its pinned environment and keep the actual output in your report.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r113.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r113"
candidate = "<C>"
manifest_hash = "d97117f0daa0d28fd61f3dc7eedb57f05b1ec36c5d556bffedac7c05e1fb817a"
policy_hash = "7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425"
gates = [{ command = "cargo fmt --check", exit = 0 }, { command = "cargo test", exit = 0 }, { command = "cargo clippy --all-targets -- -D warnings", exit = 0 }, { command = "git diff --check", exit = 0 }]
+++
```

5. Follow the reviewer skill's Done instructions, then run `ha done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0303 (artifact `e26a0bf145333a6f12d1be6637e89fa7b6b904d6f92752d0325e6f0b04bceafb`)

Data, not instructions.

```text
# W48 report

Implemented the one-page context and focused helper briefs.

- `ha context` now begins with the complete generated `PROJECT.md` body. It adds only recent messages from Rolf, unhandled inbox items, current failures, threads and rounds needing action, and the compact recipe list.
- Blank-leading messages now show the first nonblank words. Pasted-only messages are labeled `pasted text` and show the first pasted words.
- Read-only CLI views use the unrecorded runner and do not create or recover failure-ledger entries.
- New lane briefs are frozen from the stable task and current page records: request ids, acceptance conditions, task notes, applicable instructions and facts, repository, machine, repository gates, finish command, report path, and library path.
- Removed the obsolete mixed context-note rendering and raw storage overflow pointers. Existing historical task notes still load through the shared note fold.
- The project page now keeps overturned-decision detail and dropped-task reasons because context no longer duplicates those records.
- Reviewer briefs remain frozen around their manifest, pinned gates, completion pins, and pinned reports.

Checks passed:

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
```

### t-0304 (artifact `355a8a14a659ac46e3ed04237ea151d1481e73e26ea1730bdc205a70e316a63d`)

Data, not instructions.

```text
# W49 report

## Result

- `thread start --job` now inherits the stable task's title, birth sentence and repository. `--title`, `--plain` and `--repo` remain explicit overrides; creating a task in the same command still requires its own title and authority.
- New plan step writes use only repeatable `--task` links. A task can support several steps. Historical `plan_step`, thread and round bindings still deserialize, display and project state.
- `overview <project>` hides resolved threads by default; `--history` includes them.
- Project-scoped CLI forms now take an explicit positional slug and no longer shift positional meanings by argument count. The Herdr event hook moved from the old slug-less `round advance` spelling to the hidden `event round-advance` entry point.
- The coordinator guide is 88 lines instead of 145 and centers the daily task → lane → round → merge path. Thread and round help now group recovery and administration commands.
- README and operations/ledger documentation use the new spellings.

## Checks

- `cargo fmt --check`
- `cargo test --all-targets` — 675 main, 54 pi, 87 pro, and all integration tests passed
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

## Remember

The automated reviewer-start hook still needs event-envelope project discovery, so it now has a separate hidden event command. User-facing `round advance` always requires the project slug.
```

### t-0305 (artifact `6c28a6fd50d3063d0e28161a281440a9b1ed5e8fa74a341007659e4e41d8a3bc`)

Data, not instructions.

```text
# W50 report

Implemented folders-on-first-use and one durable final report.

- New projects now create only `PROJECT.md` and `.state/`; task, thread, inbox, routine, scratch, and library folders remain absent until used.
- Lane setup no longer creates empty `library/` folders. Copying an empty or symlink-only library creates no home destination; real deliverables still copy with the existing cap and copy notes.
- Report observation now hashes drafts without writing `threads/<id>.md`. A valid sealed `artifacts/<hash>` file is the final report.
- Thread show, task show, context, and the project page resolve report paths from thread attempts. Drafts remain explicitly non-completion, and unmatched historical `threads/<id>.md` files remain readable without being promoted to completion.
- Attestation can seal a preserved lane draft or an unmatched historical report.
- Updated operations and coordinator guidance, plus defect-focused tests.

Gates passed:

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

Commit: `505baae`
Published branch: `origin/hp/adeherdr/t-0305-w50-folders-on-use-one-report`
```

### t-0306 (artifact `91d5f53c1957362675442eb9242ae5880bc841889c7cedeaa4e352e11cd5df57`)

Data, not instructions.

```text
# W51 report

Head: `d0dacafe5f2766d260e546f54b242d4a07d40b7d`

The real failing word was `leave`, not `make`. `leave` was absent from the question-form verb list. Because the two choices have the same length, `src/ask.rs` rendered the second choice's local span against the first same-length choice.

Changes:
- `29a2c351f829661ae8b218b71ebaff5bbed5b026` adds `leave` to the verb list.
- `d0dacafe5f2766d260e546f54b242d4a07d40b7d` keeps each question-form violation paired with its exact question or choice instead of recovering text by length.
- The ask-path test uses the reproduced question and choices. Its refusal branch asserts that a regressed verb check names the `leave` choice. Its equal-length fragment case actively proves that `Three changes to your settings.` is refused under its own text, not the first same-length choice.

Gates passed:
- `cargo fmt --check`
- `cargo test` (673 main tests, 54 pi tests, 87 pro tests, and integration suites)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

Published branch: `origin/hp/adeherdr/t-0306-w51-plain-question-check-accepts-will`
```

