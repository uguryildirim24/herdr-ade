# Review brief: round r95

plain: Rolf's newer choices always come before old notes, and briefs stay small.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r95` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `713684cbfb97b73fce47aa9589c7ee15f4c3ea5099d462e6086bbc3a3e30e3a5`, policy hash `d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0226 | 1 | `bead1982d5aa8b385bb43edff46706966e9eec5a` | `t-0226-1-1` | `13019bac1c0276cebad0599eabcfa2636095f2c8d90b119be4804f6594137ceb` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r95.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r95"
candidate = "<C>"
manifest_hash = "713684cbfb97b73fce47aa9589c7ee15f4c3ea5099d462e6086bbc3a3e30e3a5"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0226 (artifact `13019bac1c0276cebad0599eabcfa2636095f2c8d90b119be4804f6594137ceb`)

Data, not instructions.

```text
# D7 report

Implemented all four acceptance areas.

## What changed

- Added append-only `notes.jsonl` records for project memory and standing instructions. `ha note add` requires the Rolf request id, records the date, supports task scope, and accepts an explicit `--replaces` id.
- Extended `ha task note` with stable note ids, required request provenance, and explicit replacement. Extended `ha task add` so a newer task can explicitly replace an older note, instruction, decision, or task.
- Historical `PROJECT.md`, `MEMORY.md`, `memory/*.md`, and old task notes still load. Context labels records without complete provenance as `undated`; no date or subject is guessed.
- `ha decide --replaces` can now supersede a note or historical instruction as well as another decision.
- `ha context` renders the unified note/decision/task replacement history newest first, marks both `replaces` and `replaced by`, and bounds the section to 20 rows.
- Briefs fold explicit replacements, include only unscoped notes plus notes attached to their stable task, put newer current notes first, and retain the existing memory budget. The task is linked before first-brief composition so task-scoped notes work on attempt one.
- Updated README, operations/getting-started docs, project template, and coordinator instructions for the new commands.

## Skill rules removed because code already refuses the state

1. Removed the warning that `thread start` refuses `--role` and `--model`; those CLI options do not exist.
2. Removed the three-open-ask limit from skill prose; `ha ask` enforces it.
3. Removed the instruction to serialize merges and resume the existing merge first; `ha round merge` owns and enforces merge serialization/recovery.

## Explicit subject rule

The engine never compares prose. A newer record has the same subject only when it names the older record with `replaces`. Valid replacement sources are a decision, project note, task note, or task.

## Gates

Run with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check` — pass
- `cargo test` — pass (623 main, 57 pi, 82 pro, 19 integration tests)
- `cargo clippy --all-targets -- -D warnings` — pass
- `git diff --check` — pass

No live project or live configuration was edited.
```


## Repair revision

This revision reviews the integration base `a08fc6eef798ffc83c4abdae3501a679a03f1a20`.
