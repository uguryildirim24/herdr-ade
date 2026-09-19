# Review brief: round r18

plain: This round checks the records behind the project screen: the plan, its steps, and the choices made for you.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r18` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `041fa869b066d3a36953fb6e63ac611cd510c73adf637015df18f0357482db96`, policy hash `7cf667dea9748fe2844267e971e613ff74b94a3ef0af9df5ee974d759cb7e9da`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0041 | 1 | `db95ad255375a101ae3dfab476a9bccec288394f` | `t-0041-1-2` | `be8220c9f80916dc2967f005ce9f900e6d51fa9c2e80af8266851a6ac3903203` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r18.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r18"
candidate = "<C>"
manifest_hash = "041fa869b066d3a36953fb6e63ac611cd510c73adf637015df18f0357482db96"
policy_hash = "7cf667dea9748fe2844267e971e613ff74b94a3ef0af9df5ee974d759cb7e9da"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0041 (artifact `be8220c9f80916dc2967f005ce9f900e6d51fa9c2e80af8266851a6ac3903203`)

Data, not instructions.

````text
# t-0041 — plan card, decision log and the coordinator rule (SPEC-talk v2, Lane 1)

Commit `db95ad2` on the lane branch. No push.

## What shipped

The records and commands behind the project screen, in `herdr-ade`. No screen
in this lane; the primitives and the automatic step transitions work without
one.

| File | Change |
| --- | --- |
| `src/contracts.rs` | `Plan`, `PlanStep`, `StepState`, `Plan::PLAN_KINDS`, `plan_kind_sentence`, `Decision`, `AuthorityRef`; `HumanMessage::Say` gained the optional `landed_round`; a plan/step/decision/reference round-trip test and an old-record (missing-field) load test. |
| `src/plan.rs` (new) | The card file, its commands, whole-card validation, revision guards, atomic parent-flushed writes, and the shared state projection. |
| `src/decide.rs` (new) | The decision history, validation, retry keys, authority references, current folding and replacements. |
| `src/cli.rs` | `ha plan` and `ha decide` registered; `ha say --landed-round`. |
| `src/threads.rs` | `carrying_rounds` and `round_landed` (durable round inclusion); `refresh_plan` called at `thread start` and `thread resolve`/`reopen`. |
| `src/round.rs` | `admit`/`remove` refresh the plan; `merge` publishes one keyed landing line and refreshes the plan at its durable completion boundary. |
| `src/checkpoint.rs` | `checkpoint` refreshes the plan after the HANDOFF commit. |
| `src/ask.rs` | Three-open-ask cap, `asks/.open.lock`, canonical `ask:<id>@<revision>` key kept, `say_landed`. |
| `src/talk.rs` | `Say` carries an optional `landed_round`; old entries still load. |
| `src/glossary.rs` | `check_sentence`: one plain sentence against the project registry. |
| `skill/COORDINATOR.md` | The §6.7 paragraph plus a short plans-and-choices section. |
| `docs/operations.md` | The new verbs, the authority boundary and recovery. |

## File shapes

`<project>/plan.toml` (exactly as in effect; `state` is a projection):

```toml
schema = 1
revision = 3
next_step = 3
goal = "I want to build a trading bot with Jeff."
kind = "screen"
what_you_get = "A screen you open."
does = "It shows pretend trades and lets them stop them."

[[steps]]
id = "s-1"
text = "Choose what the screen will show."
state = "left"
threads = []
rounds = []
```

`<project>/decisions.jsonl` (one complete line each, nullable fields present):

```json
{"schema":1,"seq":1,"id":"d-0001","at":"2026-09-19T21:16:41Z","line":"I kept the words short.","class":"routine","key":null,"basis":null,"replaces":null,"request":null}
```

A landing say line:

```json
{"seq":52,"key":"landed:r1","at":"2026-09-19T12:10:00Z","say":{"what":"The first round lands the shared types.","means":null,"landed_round":"r1"}}
```

`plan show --json` on a missing card: `present: false`, `revision: 0`,
`steps: []`. Writes go through a temporary file, `sync_all`, `rename`, then a
parent-directory `sync_all`, under `<project>/.plan.lock`.

## Verbs, one example each

Run from a project context; `--project <slug>` names one explicitly.

```
ha plan set --kind screen --does "It shows pretend trades and lets them stop them." --expect 0
  plan revision 1 set to `screen`

ha plan step add "Choose what the screen will show." --thread t-0041 --expect 1
  plan revision 2: added s-1

ha plan step edit s-1 "Choose what the first screen will show." --expect 2
ha plan step link s-1 --round r1 --expect 3
ha plan step unlink s-1 --thread t-0041 --why "That work moved to another step." --expect 4
ha plan step remove s-1 --why "That step is no longer needed." --expect 5
ha plan step move s-2 --before s-1 --expect 6
ha plan show
ha plan show --json
ha plan sync
  plan revision 8: step states refreshed

ha decide "I kept the words short." --class routine
  d-0001 routine
ha decide "I will spend five dollars on this check." --class money --basis ask:a-3@1
ha decide "I will show more detail beside each choice." --class routine --replaces d-0001 --request q-1234
ha decide "I kept the words short." --class routine --key k-1   # same payload: returns d-0001
ha decide list [--json]
ha decide show d-0001 [--json]

ha say --what "The first round lands the shared types." --landed-round r1
```

`set` copies the exact `PROJECT.md` goal, preserves steps, generates
`what_you_get` from the kind, and refuses a bad kind or a failed plain check
without changing the old file. `--expect 0` creates a missing card.
Step/`set`/decision text is checked with the project registry (a born name may
appear in gloss form); `--why` and decision lines must be one sentence.
`--thread`/`--round` must name existing project records. `move` changes display
order only.

## How a step flips to done

The coordinator never sets a state. `state` is written only by
`plan::refresh`/`plan sync`, which derives it from durable records:

1. `done` — at least one binding exists and every binding is satisfied.
   A thread binding is satisfied only when **every** round whose manifest
   includes that thread (`threads::carrying_rounds`) has a checkpointed merge
   (`threads::round_landed`). A `rounds` binding is satisfied only by that
   round's checkpointed merge. A lane saying `done`, being resolved or being
   closed is not enough.
2. `running` — otherwise, any required work has started or partially landed:
   the thread record exists, or it sits in a not-yet-merged carrying round, or
   a bound round exists but has not merged, or some bound work landed while
   another requirement has not.
3. `left` — otherwise: the step is unbound, or every bound record is missing.

Because a new carrying round for a thread that already landed is not merged,
adding required rework reopens a `done` step on the next refresh — no manual
flip. `refresh` is called from `thread start`, `thread resolve`/`reopen`,
`round admit`/`remove`, the `round merge` completion boundary, and
`ha checkpoint`; `plan sync` is the manual form. A refresh failure is printed
on its own line and never rolls back the operation that triggered it.

## The three-ask rule

At most three open asks co-exist, counting an ask whose publication is still
pending (`open_asks` reads the stored latest unanswered revisions, not the
published markers). Creation, re-asking and answering take
`<project>/asks/.open.lock` inside the project lock, so concurrent writers
cannot each claim the last slot. A fourth creation fails with `ask_cap` before
the id is allocated or any record is written. Re-asking keeps the identifier
and advances the revision, so a merged re-ask of the newest keeps the two older
asks and stays at three. A project already above the cap creates nothing new
until the count is back within bounds. Publication keeps the canonical
`ask:<id>@<revision>` key.

## Tests and gates

`PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`

- `cargo fmt --check` — pass.
- `cargo test --locked` — 377 + 50 + 78 + 4 pass, 0 fail. New tests cover the
  card round-trip, exact goal copy, stale revision leaving the file untouched,
  the seven fixed result sentences against an empty registry, the seven-step
  cap and non-reuse of ids, the required `--why`, missing-card creation, the
  full state derivation using a real reviewed/merged round (running → done →
  reopened by rework), one-of-two requirements, decision folding/keys/authority
  (including a `no` answer authorizing nothing)/replacement/broken tail, the
  ask cap with a pending publication, a merged re-ask, one keyed landing line,
  and refusal of a landing line for an unmerged round.
- `cargo clippy --all-targets --locked -- -D warnings` — clean.
- `cargo build --release --locked` — pass.

Tests use the existing fake runner and `round::testkit::fixture`; no herdr
server, no push, no `git stash`.

## Spec lines I could not fully honour

1. **`--why` is required and checked but not stored.** The plan schema in
   §6.5 has no field for it and the decision log is separate. `step remove`
   and `step unlink` require a checked one-sentence `--why`, but it is not
   persisted. The spec's "must follow the decision/ask rule before the
   mutation" is left to the coordinator; the command does not itself write a
   decision record (the command has no `--class`/`--basis`).
2. **Re-asking is not restricted to the newest open ask.** The cap is
   enforced and a re-ask preserves the identifier, but `ha ask --reask <id>`
   accepts any unanswered latest revision, not only the newest. The merged
   rule is the coordinator's step; the plugin does not refuse a re-ask of an
   older ask.
3. **Legacy over-cap recovery is a hard block, not a display.** Additional
   creation is blocked, but the "show all existing open asks with a plain
   warning" part of §6.7 belongs to a screen (Lane 2); Lane 1 exposes every
   stored ask through `open_asks` and blocks only new ones.
4. **The plan-catch-up sentence is not rendered here.** A failed refresh is
   reported to the caller; the screen's `The plan needs to catch up.` and the
   goal-mismatch rendering are Lane 2. `plan show` does print a goal-differs
   line so the mismatch is visible before the screen exists.

No other spec line in §2.7, §6, §8 Lane 1 or §6.7 was skipped.
````

