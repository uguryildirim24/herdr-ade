# Unattended loop findings

This historical report describes the builds below, not current test results. External evidence archives are not included. See [host requirements](../../README.md#host-requirements) before using the reproducers.

**1 confirmed defect; 0 additional production suspicions.** No production Rust
changes. Findings are ranked by babysitting cost (only one confirmed here).

The requested walk-away journey is **partially established**, not passed in
full: a scripted command driver completed two dependent, independently
fixture-reviewed piles and published both files. The stock scripted coordinator
could not receive its priming prompt, so real unattended notice consumption is
not established. Installation is also not established: this scratch repository
has no installation target. `install=true` on a review with
`install_required=false` is not installation evidence.

## Build and evidence

- Base commit: `05ddf72d057deda9cf707831ca1af7310881589c`.
- Both sandbox accounts installed from this lane's `cargo build --bins` before
  scenarios. Instance **2 only**, on the prepared Linux host.
- Version: `herdr-ade 0.1.0+05ddf72.1791114866`.
- Installed ADE SHA-256:
  `695cc780c1b8dabc4c311bc0afc8645256e15cf24ca2b989babef5ec22fa3f9c`.
- Evidence paths below are relative to
  `/path/to/evidence/` in the recorded lane checkout.
  Each capture directory contains the controller's `local.tar` and `box.tar`.
  All captures preceded the next reset. Runtime evidence is not committed.

Reproduction (use only an assigned, available instance with the build installed):

```sh
tools/wall/findings/loop/repro-01 --instance 2 --evidence /absolute/new/capture
# Expected exit 1 on the investigated build; 0 when the defect is absent.
```

`--evidence` is optional; its default is a fresh directory beneath
`~/.cache/herdr-wall-loop/`. The executable starts with the selected instance's
reset, sends the plain `guest-scenarios.py` file, runs it by path, prints EXPECTED
and ACTUAL, and captures evidence. It leaves the evidence-bearing state for
inspection. Reset your assigned instance when finished.

Other repeatable coverage, using the same reset/capture driver:

```sh
tools/wall/findings/loop/run-scenario journey --instance 2 --evidence /absolute/new/journey
tools/wall/findings/loop/run-scenario recovery --instance 2 --evidence /absolute/new/recovery
tools/wall/findings/loop/run-scenario notice-probe --instance 2 --evidence /absolute/new/notices
```

The last command is a coverage probe, **not a confirmed production regression**;
it times out with the present scripted coordinator (explained below).

## LOOP-01 — A fresh goal-only wait disappears before Rolf is asked

**Cost:** a scope question needed before tasks can be written silently becomes
“retired”; the project retains a wait disposition that prevents another goal
wake. Rolf can return to no current work, no open tasks and no visible question.

**Build:** the version, commit and binary hash above.

**Exact steps:** `repro-01` resets instance 2; `guest.open_project(strict=True)`
opens the scripted coordinator and records an ordinary request through the real
prompt hook; wait for the real ticker's first goal-check generation; run:

```sh
ha plan set wall --does 'Deliver the scope Rolf chooses'
ha plan check wall wait Rolf --condition 'Choose one or two fixture files' --evidence 'Scope choice needed before defining tasks'
ha context wall
ha overview wall
```

Read `goal-check.json`, observe another eight seconds, then exercise the answer:

```sh
ha plan check wall answer Rolf --evidence 'Choose two files'
```

- **Setup/injection:** no damage or lifecycle-record edits. The public wait
  command succeeds, with no `--task` (the advertised syntax; `--task` is optional).
- **Expected:** this new goal-level question remains visible with its party and
  wake condition until answered, superseded or closed. A task need not exist
  before Rolf chooses the goal's scope.
- **Actual:** context immediately says
  `Goal check wait retired; awaiting the next outcome judgment.` Overview shows
  no work and no tasks. Neither surface shows the question. Eight seconds later
  the generation and wait disposition are unchanged. The later explicit answer
  increments the generation but records `answers[0].waits=[]`, because the
  question was already excluded from the open-wait set.
- **Recovery:** an explicitly supplied answer re-owes a judgment. It requires
  knowing the hidden question; no surface tested asks Rolf to answer it.
- **Semantic result:** scope was not chosen and the goal was not achieved.
  No indefinite-time measurement is claimed: the suppression follows directly
  from the still-present disposition and the notice predicate.

**Durable evidence:** `repro-01-final.log:70-184` contains the exact commands,
missing question and EXPECTED/ACTUAL. Lines 187 onward contain the answer
record. `loop-01-final/local.tar` contains
`.herdr-ade/wall/.state/goal-check.json`: historical wait with `tasks=[]`,
`party="Rolf"`, the exact condition, `retired_waits=[]`, and, after answer,
`answers[0].waits=[]`. The pre-answer record is printed in the log. Initial
independent reproduction: `repro-01.log`, `loop-01/`.

**Suspected code path:**

- `src/steps/goal_check.rs:500`: `open_indices_with_evidence` drops every empty
  task scope, including a fresh, deliberately recorded goal-only question.
- `src/steps/goal_check.rs:614`: the filtered-out current wait renders as retired.
- `src/steps/goal_check.rs:290`: any retained disposition suppresses notices.
- `src/steps/goal_check.rs:561-578`: answer uses that same filtered open set.
- `src/steps/goal_check.rs:304`: the actual goal-check instruction advertises
  `wait <party> --condition ... --evidence ...` without a task.

**Smallest suggested fix:** preserve a current goal-only wait until an explicit
answer, superseding action/judgment or closure. Do not treat empty task scope as
proof that the goal question was answered. Keep D43's retirement of genuinely
finished task-scoped waits. The existing test at
`src/steps/goal_check/tests.rs:701-748` checks an empty *historical* scope but does
not cover this fresh pre-task CLI journey. This is the opposite path to D43,
not another report of its stale finished-task waits.

**Not tested for this finding:** real provider prompt delivery, Mac rendering,
Rundown pixels, a later real human prompt, or long-duration wake behavior.

## Covered behavior and limits

### Complete for these bounded scripted scenarios

- Two-step plan, second step `--after s-1`; premature second launch rejected
  with an explanation and `ha plan sync wall` guidance. Premature goal closure
  rejected for absent terminal acceptance. Evidence: `journey-4.log:86-91`.
- Task-scoped Rolf wait remains visible; `answer` removes it and re-owes the
  judgment. `action` works for the first and later dependent job. Evidence:
  `journey-4.log:92-212`.
- First seal queues a new goal obligation; disabled automatic reviews produce
  a concrete enabling command. Running that command lands the first pile.
- After that landing, dependent work starts with accepted prerequisite evidence.
  The second ready pile starts and lands automatically without a second
  `ha review`. Both are independent exact-fixture MERGE judgments, not arbitrary
  model acceptance. Evidence: `journey-4.log:213-343`, `journey-4/local.tar`.
- Plan goes left → running → done; repeated sync preserves both done steps at
  revision 7; accepted goal `close` succeeds. Both files reach local bare
  `main`, exactly equal to the local integration head. Evidence:
  `journey-4.log:344-521`, review-1/review-2 records and sealed reviewer artifacts.
- A scripted process exits at `working`; one configured automatic same-recipe
  retry runs, then stops at attempt 2 with `same_recipe_retries=1`, original
  recipe and an actionable exhaustion explanation. The literal retry command
  launches attempt 3 on that same recipe after removing the deliberate exit.
  Evidence: `recovery-1.log:81-290`, `recovery-1/local.tar`. Injection succeeded;
  process recovery succeeded; no semantic work-completion claim for this probe.

### Partial / unavailable

- **Actual unattended coordinator delivery:** `notice-probe` watched an empty
  plan for 200 seconds. Ticker was live, `generation=2`, but `queued=false`,
  `delivered_at=0`; coordinator remained `prime_pending=true`, `prime_sent=false`.
  Its pane was idle and contained only the scripted ready JSON, **no editor**.
  `coordinator-input-hold.json` recorded its pane. The prompt-clear check at
  `src/prompt.rs:465` conservatively rejects this non-TUI screen; priming is
  held at `src/coordinator.rs:859`, and `src/ticker.rs:3817` suppresses goal
  delivery while priming is pending. Evidence: `journey-3/`,
  `nudge-after-observe/`, `nudge-observe.log`.
  This is a sandbox stand-in limitation, not proof that real pi fails. Neither
  exactly-once delivery nor correct-pane consumption/acting on every notice is
  established. `start_notices.submitted=true` is not proof of agent consumption.
- The successful `journey` is an unattended **external scripted command driver**
  following `ha context`'s instructions and supplying fixture-specific judgments.
  It does not prove the stock scripted coordinator autonomously ran those
  commands, or that a real coordinator would. No human intervened during the
  final driver run, but this distinction matters to the requested walk-away bar.
- Installation unavailable for this scratch repo; no `ha harness` was run.
- Not exercised: 20-minute pile wait expiry, active-review member correction
  holds, failed-live-agent holds, provider/connection recovery budgets,
  work-failed retry routing, idle/stuck-but-alive escalation, ticker restart
  deduplication, two missed goal turns, or concurrent wait/answer races.
  Source inspected for these paths; not live coverage. No injectable clock
  or record timestamps were fabricated. D22/D28/D53/D59 are not re-reported.
- Mac, real pi/editor behavior and arbitrary semantic model review unavailable
  in this scripted campaign. No forbidden model or real provider was started.

### Every literal `next:` command encountered and tried

| Printed command (instance 2) | Result / evidence |
| --- | --- |
| `/home/wall-2/bin/herdr-ade --root /home/wall-2/.herdr-ade open wall` | Exit 0, existing coordinator reopened; `next-new-local.log`. |
| `/home/wall-2/box/bin/herdr-ade --root /home/wall-2/box/.herdr-ade open wall` | Exit 0, box coordinator opened; `next-new-box.log`. |
| `ha review wall --repo /home/wall-2/repo` | Exit 0, first pile reviewed/landed; `journey-4.log:279-282`. |
| `ha thread retry wall t-0001 --reason "retry failed startup"` | Exit 0, attempt 3 queued and brief submitted; `recovery-1.log:285-290`. |

Other encountered `next:` values were prose, not shell commands: `start an
attempt`, `review the repository pile`, `retry or cancel the current attempt`,
`none`, and `automatic same-recipe retry selected for attempt 2; wait for
startup`. Their corresponding behaviors were exercised above. Commands from
unvisited surfaces are **uncovered**, not assumed valid. The non-`next:`
prerequisite guidance `ha plan sync wall` was also executed as printed.

## Suspicions and campaign bookkeeping

No additional production defect is claimed. The undelivered nudge is kept as
an explicit coverage limitation, not promoted from the scripted stand-in.

Two earlier driver issues were not harness defects: the first host command was
cut off by a 120-second execution timeout; a later run completed both piles but
its logging `tee` failed because `runs/` did not exist at startup. The driver now
creates that directory; final `journey-4` and `recovery-1` exited 0. Earlier
captures remain in the library rather than being concealed.

Gates passed: `cargo fmt --check`, `cargo test -q`,
`cargo clippy -q --all-targets -- -D warnings`,
`python3 -m unittest discover -s tools/wall -p 'test_*.py'`, and
`git diff --check`. Logs: `fmt.log`, `test.log`, `clippy.log`,
`python-tests.log`, `diff-check.log`. **Full `tools/wall/gate` was not run**, per
the coordinator's explicit D63 instruction; it belongs to pile review.

Instance 2 was left at a clean reset of the named base build:
`final-reset.log:58-61` shows the reset, expected version, empty lane list,
clean repository and deterministic baseline commit
`9dbce763e1135f8ef4c796d272e32816c6e491c8`. No other wall instance was changed.
