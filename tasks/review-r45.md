# Review brief: round r45

plain: This round checks that the message list stops repeating what the records already say.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r45` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `99d15a741f3f58461789a5ad6ee4cd87dc840a62b00debe3f2583cc23f0c2c41`, policy hash `518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0097 | 1 | `93c56098d3166ef57234640b433302c53e4fd2c8` | `t-0097-1-2` | `e1818a50484c326a8536cd274f9febf28900e4ae6ba742d5b263856f6a9abbf7` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r45.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r45"
candidate = "<C>"
manifest_hash = "99d15a741f3f58461789a5ad6ee4cd87dc840a62b00debe3f2583cc23f0c2c41"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0097 (artifact `e1818a50484c326a8536cd274f9febf28900e4ae6ba742d5b263856f6a9abbf7`)

Data, not instructions.

```text
# t-0097 — E1: inbox messages, not duplicate records

Commit: `93c56098d3166ef57234640b433302c53e4fd2c8`
Branch: `hp/adeherdr/t-0097-e1-the-message-list-stops-repeating-the`
Machine: `oci`

## Result

Thread and round inbox writers are deleted. Context reads thread records, current-attempt sealed completion evidence, completion preparation records and E2 round records directly. Round phases, pins, reviewer failures/actions and interrupted/diverged merges are visible without inbox projections. Copy warnings, PR metadata/validation problems and lineage mismatch now live on the thread. Round actions and the observed divergent head live on the round. No new settings, feature flags or compatibility paths.

Old removed-kind files are ignored on read, including nudging and inbox handling; they are not migrated or rewritten. Writers refuse those kinds. Local completion wake-up delivery is unchanged, but writes no inbox item. A courier import writes one stable `courier-delivery` message, before advancing its cursor; replay cannot duplicate it.

Context acknowledges only completion evidence actually shown, for the matching coordinator pane/generation. Peek, other panes, old attempts and superseded evidence do not acknowledge it. Report-only state is not confused with sealed done; a waiting event does not hide an available report.

E2 was present at the start pin `04fdd1e`, following r42 (`56d371c`). Read `RoundRecord.phase` and the phase-validating round reader; context uses that reader, not branch-name guesses. This box's lane root has bootstrap state rather than the Mac's live rounds/inbox. The existing round tests and the measurement fixture exercise actual phase-bearing `.state/rounds/*.toml` records. Fork instructions were read from the box checkout at `/home/ubuntu/projects/herdr`.

## Every existing inbox kind

| Kind | Result and reason |
|---|---|
| `thread-state` | Removed: group, pane/machine and auto/merge resolution reason come from thread records. |
| `report-available` | Removed: report hash/path and incomplete-copy notes belong to the thread. |
| `done` | Removed: current attempt's immutable sealed completion supplies sha and report path directly. |
| `waiting` | Removed: current attempt's sealed waiting evidence supplies its reason directly. |
| `preparation-abandoned` | Removed: the op already owns abandonment and context already renders it. |
| `pr` | Removed: thread owns the sanitized PR summary and validation problem; no comment bodies enter context. Ticker's duplicate summary cache was deleted. |
| `lineage-mismatch` | Removed: current identity mismatch is stored on the thread and clears when verified. Its separate notification markers were deleted. |
| `round-advance` | Removed: round owns phase, current action and retry count; MERGE still emits one plain `say` line. |
| `merge-pending` | Removed: embedded merge transaction/phase supplies the interrupted-merge action. Notification marker code deleted. |
| `merge-diverged` | Removed: round phase/transaction plus the observed head describe the divergence. |
| `recipient-changed` | Kept: coordinator transport rebinding needs an independently handled message, not a thread status. |
| `session` | Kept: server restart notice includes the coordinator restart action; no thread/round owns that machine event. |
| `outage` | Kept: machine/GitHub outage and recovery notifications have no thread/round home. |
| `config-changed` | Kept: a policy change notification, not a thread/round fact. |
| `config-error` | Kept: unusable project/routine configuration notice, outside thread/round state. |
| `routine` | Kept: scheduled firing and command output need their own message. |
| `routine-approval` | Kept: a scheduled run was skipped pending approval; the run notice has no thread/round home. |

New message kind: `courier-delivery` records receipt of a sealed envelope from a named machine, rather than restating the done/waiting payload.

## Context size

Reproducible synthetic backlog, **not a claimed measurement of the unavailable Mac inbox**. Same project and records on both binaries: 40 threads, 20 sealed completions, 10 phase-bearing rounds, 65 abandoned preparations; 176 inbox files (65 preparation notices, 40 state notices, 40 report notices, 20 done notices, 10 round notices, one routine). All preparation facts remain in their existing digest section.

| | Before (`04fdd1e`) | After |
|---|---:|---:|
| UTF-8 bytes | 28,853 | 11,161 |
| Lines | 314 | 211 |
| Whitespace words (not tokenizer tokens) | 3,004 | 1,497 |
| Visible inbox messages | 176 | 1 |

Reduction: **61.3% bytes**, 50.2% whitespace words. Only the executable-dependent Commands line was normalized to `Commands: ha`. Both reads used `context demo --peek` against the same temporary project. Before binary built from a `git archive` of the starting commit.

Reproduce: `python3 scripts/context-size.py BEFORE_BINARY AFTER_BINARY OUTPUT_DIR`.
Full outputs: `library/context-before.txt`, `library/context-after.txt` beside this report.

## Gates

All commands used `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check`: PASS.
- `cargo test`: PASS, **637 tests** (496 + 56 + 78 + 6 + 1), none ignored.
- `cargo clippy --all-targets -- -D warnings`: PASS.
- `git diff --check`: PASS.

Logs: `gates/fmt.log`, `gates/tests.log`, `gates/clippy.log` beside this report.

Coverage includes: a thread finishing through the ticker appears in context with zero inbox items; a sealed local done is delivered once and appears with sha/report without even allocating an inbox counter; actual courier import/replay produces exactly one delivery message per project; removed legacy kinds are ignored without rewriting their files; round verdict/retry/gone/divergence facts appear without messages; PR state and lineage facts appear directly; context receipt integration test checks peek, wrong pane, wrong generation, superseded events, current evidence and waiting-with-report.

No live install or project-memory edits. Only this lane branch is published; integration/main are untouched.
```

