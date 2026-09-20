# Review brief: round r38

plain: This round checks the record the harness keeps of its own failures and the one verb that turns one into work.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r38` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `9dcadc893ac75050bdf0e82cba4b2ccbf5a51a0d28423ba81e817cfac24c3bb5`, policy hash `3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0081 | 1 | `be9f28ae17f7af58c148cff3d2e318d849c063ca` | `t-0081-1-2` | `18fd892f92bd8f92e2d7b2f925a908afb695d07951a030f030633b18b934622e` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r38.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r38"
candidate = "<C>"
manifest_hash = "9dcadc893ac75050bdf0e82cba4b2ccbf5a51a0d28423ba81e817cfac24c3bb5"
policy_hash = "3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0081 (artifact `18fd892f92bd8f92e2d7b2f925a908afb695d07951a030f030633b18b934622e`)

Data, not instructions.

```text
# t-0081 — A3 failure ledger

Commit: `be9f28ae17f7af58c148cff3d2e318d849c063ca`
Branch: `hp/adeherdr/t-0081-a3-the-harness-records-its-own-failures`
Machine: `oci`

## Delivered

- `src/ledger.rs`: locked, append-only `<project>/ledger.jsonl`. Immutable revisions fold into one entry per kind, subject and normalized detail, with stable id, first/last timestamps, evidence, cumulative count and closure. Recurrence reopens the same entry. Normalization removes whitespace/ANSI presentation differences without erasing error codes or paths.
- `ha ledger list [--json]`, `show <id>`, `done <id>`, and `task <id>`. Task writes an actionable brief to stdout, including evidence and times; it can feed `thread start --task-file -` directly.
- Recording for CLI/child-command failures (including timeout/spawn failure), silent reviewer-start failures, merge refusals, thread error/blocked transitions, failed zero-attempt launches, courier/SSH failures, command/reviewer/merge/courier retries and lane/coordinator launch retries.
- Ticker, courier and project-list reads explicitly scope observations to the affected project(s). Unchanged state polls do not inflate counts. Successful recovery stops retry counting without pretending the underlying defect is fixed.
- Context and screen show up to five open repeated/new failures, worst first. Context-read cursor is journaled; peek does not advance it. Digest summaries are capped at 220 characters. Screen descriptions are plain, retain stable failure references, and use the existing single-line clipping.
- Usage and journal semantics: `docs/ledger.md`.

## Gates

All run with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`, using the lane's `CARGO_TARGET_DIR=/home/ubuntu/build/lanes/adeherdr-t-0081`.

- `cargo fmt --check`: PASS.
- `cargo test`: PASS, **593 tests** (454 ADE + 56 pi + 78 Pro + 5 CLI).
- `cargo clippy --all-targets -- -D warnings`: PASS.
- `git diff --check`: PASS.

Coverage includes concurrent folding, closure/reopening, preserved journal bytes, corrupt/torn journal refusal, task output, bounded digest and cursor behavior, scoped attribution, recovery stopping retries, real nonzero subprocess recording, error/blocked transition deduplication and zero-attempt launches, silent `round advance` failures despite successful return, real merge-verdict refusals, courier receipt failure recording, screen selection/plain output, and end-to-end ledger CLI commands.

## Review notes

- Append-only versus folding is resolved with immutable revisions of the **same id**; displayed entries are folded, while historical evidence remains in the journal. Recovery and context cursors also live in that one journal.
- Ledger write errors warn without replacing an operation's original result. Malformed journal rows are not silently skipped. No retrospective import/backfill is performed.
- This records commands run by the ADE harness, not arbitrary agent tool calls. Child argv/stdout/stderr are evidence; environment and stdin are excluded. Global commands with no project binding do not invent a project.
- The initial full test run exposed the known D3 test-fixture mismatch: `src/pi/scenarios.rs` hard-coded zsh while the production doctor correctly selected bash on the box. The fixture now scripts the selected shell/probe; no production pi behavior was changed and no tests were skipped.
- Mac document paths do not exist on the box; read their cloud counterparts at `/home/ubuntu/projects/herdr/HANDOFF.md` and `tasks/ade/LEAN.md`. No project memory, fork code, settings, installed binaries or servers were changed.
- Cloud publication follows the start-line/lane-skill requirement, which overrides the inherited Mac-only no-push prose: publish only this lane branch to the remote whose URL matches the lane card (`origin`, `https://github.com/uguryildirim24/herdr-ade.git`), then seal with `ha done`.
```

