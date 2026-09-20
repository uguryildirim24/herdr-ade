# Review brief: round r48

plain: This round checks that the rundown shows only what still needs you, not everything that ever happened.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r48` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `1a45301352b8f3555dfaf772069c2e706299b01876e49eca27b944c725c9c33e`, policy hash `518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0102 | 1 | `76b2ba29877997cfa1e0cd84be977853a4203f71` | `t-0102-1-1` | `ec1b7caa9a0de49bcc9fbd779b528ddaaf92bf0c196df481cc100c81423477e4` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r48.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r48"
candidate = "<C>"
manifest_hash = "1a45301352b8f3555dfaf772069c2e706299b01876e49eca27b944c725c9c33e"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0102 (artifact `ec1b7caa9a0de49bcc9fbd779b528ddaaf92bf0c196df481cc100c81423477e4`)

Data, not instructions.

````text
# t-0102 — E5: actionable rundown

Commit: `76b2ba29877997cfa1e0cd84be977853a4203f71`
Branch: `hp/adeherdr/t-0102-e5-the-rundown-shows-only-what-needs-a-d`
Machine: `oci`

## Result

`context` now counts merged/abandoned rounds and resolved threads without listing their names. It lists only non-closed rounds, including interrupted merges and divergence, in numeric round order. No durable records are removed or rewritten.

Abandoned preparations are terminal and require no separate action, whether superseded or not: current thread state and sealed evidence carry outstanding work. None is printed. Reserved/staged operations remain visible unless their recorded thread is resolved or on another attempt; orphan preparations remain visible for diagnosis. An empty preparation section disappears entirely.

Completed tasks, empty task groups, disabled routine rows, replaced decisions and the historical question-misunderstanding counter do not appear. Tasks use the same existing parser as the talk screen. No compatibility path, flag or fallback was added.

## Every digest section

- **Project/header/settings/safety:** current command/root/identity, capability, goal, limits, permissions and configuration errors guide safe work; repository list capped at 20, overflow points to PROJECT.md.
- **Overturned decisions:** unreplaced overturned choices prevent repeating a rejected action; at most 20, overflow points to decisions.jsonl; absent when empty.
- **Memory index:** reference paths needed to find standing constraints, not memory bodies; first 20 lines plus the existing budget warning, overflow points to MEMORY.md.
- **Tasks:** only unchecked tasks under named lists, retaining owner/delegation, so the coordinator can choose unfinished work; 20 tasks total across groups, overflow points to TASKS.md; unreadable is an error rather than an empty list.
- **Failures:** existing policy retained: open repeats or failures since the last context read, worst first, at most 5; these need diagnosis or ledger closure.
- **Open threads:** unresolved lanes with current state, current-attempt completion/wait/failure, unsealed report differences, copy/lineage/PR problems and pane location support review, recovery and resolution; 20 lanes and 20 copy notes per lane, overflow points to thread records; resolved lanes only counted.
- **Rounds:** only non-closed phases with pins, reviewer, attention, start failures and merge recovery detail support advancing/recovering the round; 20 rounds and 20 members per round, overflow points to round records; merged and abandoned totals are unnamed counts.
- **Completion preparation:** current reserved/staged work or orphaned operations can need recovery; 20 operations, overflow points to ops/; sealed/abandoned/obsolete preparation rows omitted and no empty heading.
- **Open questions:** outstanding asks, current revision and numbered choices identify decisions still waiting; renderer limit 20 (normal writer already limits open asks to 3), overflow points to asks/; no historical misunderstanding count.
- **Inbox:** independently unhandled messages/firings still need handling; 20 items including shown routine output, overflow points to inbox/.
- **Routines:** active schedules and their command-permission/approval state support planning and detecting blocked runs; 20 enabled routines plus 20 broken-file diagnostics, overflow points to routines/; disabled routines counted only.

Limits are record/line limits, not a global byte cap on individual prose fields. Overflow is explicit and names the full source. Only shown inbox IDs are marked seen/acknowledged; only shown thread evidence can receive event acknowledgements. Hidden records are not silently receipted.

## Actual byte measurements

Both are real CLI stdout captures from `context adeherdr --peek`, not string-length estimates of a renderer fixture. Before is a build of the starting commit `017ee9cf0441fbfb2e149904d366fced0f12b71e`; after is this change. Each pair uses one frozen project, isolated HOME and exactly the same executable/root paths. No output normalization; no live project writes.

| Project | Before | After | Reduction |
|---|---:|---:|---:|
| Actual cloud project snapshot | 2,321 bytes / 51 lines | 1,424 bytes / 30 lines | 897 bytes (38.6%) |
| History-shaped reconstruction | 18,269 bytes / 208 lines | 1,672 bytes / 35 lines | 16,597 bytes (90.8%) |

The cloud snapshot is copied from `/home/ubuntu/.herdr-ade/adeherdr`, including its actual operations, events and failure ledger. It has 16 abandoned preparations but **no canonical Mac thread/round records**. This separately measures real, non-synthetic state; it cannot honestly reproduce the brief's 18,692-byte Mac snapshot.

The history reconstruction uses all 45 committed review briefs across the cloud fork and plugin checkouts, their real plain descriptions, branch names, policy hashes and 47 pinned attempts, plus committed verdict candidates and actual task descriptions. It models **44 merged rounds, one open round, 79 abandoned preparations, 46 resolved threads, one open thread and three unhandled courier messages**. Round r44 is deliberately held under review for this replay; this is not a claim about its present Mac phase. The 79 abandoned revisions and sealed successors are modeled over the real admitted attempts, not claimed copies of the Mac's precise operations. Checkpoint transactions are inert modeled fields sufficient to validate the replay; they are not a restoration source. Courier text is derived from three actual cloud sealed events. It omits unavailable Mac memory/tasks/asks rather than inventing them.

Before lists exactly 44 merged rows, 79 abandonment rows and 46 resolved-thread rows. After lists none of those, but preserves the open r44, its pin, open thread completion and all three messages. The extra saving beyond the two brief examples comes from removing resolved-thread history too.

Reproduction script: `scripts/context-history-size.py`. Arguments:

```
python3 scripts/context-history-size.py BEFORE_BINARY AFTER_BINARY \
  /home/ubuntu/.herdr-ade/adeherdr /home/ubuntu/projects/herdr \
  /home/ubuntu/projects/herdr-ade/.worktrees/t-0102 FRESH_OUTPUT_DIR
```

Evidence under this report's `library/measurement/`: raw before/after stdout and stderr in `cloud/` and `history/`, frozen input roots, `results.json`, and `provenance.json` with source paths and SHA-256 hashes. Path lengths affect absolute byte totals on a rerun elsewhere; each pair still uses identical paths.

## Gates

All cargo commands used `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools` and the lane's dedicated CARGO_TARGET_DIR.

- `cargo fmt --check`: PASS.
- `cargo test`: PASS, 641 tests (491 + 56 + 78 + 6 + 6 + 2 + 2).
- `cargo clippy --all-targets -- -D warnings`: PASS.
- `git diff --check`: PASS.

Six new CLI tests cover closed/open round rendering, superseded abandonment, current versus obsolete preparation, task filtering and cross-group bounds, round/preparation bounds, inactive routines plus error/memory bounds, and receipt behavior for hidden thread/inbox rows. Existing automatic/merged resolution tests now require counts rather than historical names. Gate logs are in `library/gates/`.

## Durable lesson

Bounding a digest is also a receipt change: acknowledgment must follow the displayed slice, not the original unbounded collection. Tests now enforce that for both inbox records and sealed lane events. No project memory was edited.
````

