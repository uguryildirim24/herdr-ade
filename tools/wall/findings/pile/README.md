# Review-pile reliability findings — t-0818

Scope: real ADE/Herdr/Git binaries in sandbox **instance 2**, local scripted
workers and reviewers. Fixture reviewers verify the exact scripted file and
its plumbing criterion; this is not real-model review or general semantic
acceptance. No production Rust changes.

## Confirmed findings (2)

### PILE-01 — A dropped, sealed task still lands from an in-flight review

**Priority:** highest finding here. Rolf can cancel unwanted work, see no open
task, and still receive its published code. Recovering requires noticing the
unwanted change and arranging another reviewed change to remove it.

**Build:** `herdr-ade 0.1.0+6051f43.1791098735`, base commit
`6051f43e94d790d1ec219410d0127a7aae4c86b2`; binary SHA-256
`d0ed70cbd394dc77d1cea9ae8ba02c17d567fc146cb17e5be14ede3a0ee4e981`.
The instance already ran this lane's base; it was not replaced by a newer build.

**Exact reproduction:** from this worktree on oci, with instance 2 available:

```sh
tools/wall/findings/pile/repro-01 --instance 2 \
  --evidence "$PWD/.herdr-project/adeherdr-t-0818/library/drop-review-new"
```

The executable resets the selected instance, prints EXPECTED/ACTUAL and returns
1 while this defect exists. Its `run`/`scenarios.py` support files are committed
alongside it. It uses ordinary commands, not edited task/review records:

1. Reset; open the sandbox project with the strict, unassisted helper. Route the
   reviewer to the existing scripted recipe.
2. Start a scripted fixture worker; hold at `before-seal`; invoke real `ha done`.
3. Start `ha review wall`; wait for its reviewer to receive the brief and reach
   `mid-review` (do not snapshot its pane identity before brief submission).
4. `ha task drop wall job-0001 --reason 'Fixture no longer wanted'`.
5. In the already-open reviewer checkout, merge the frozen member SHA, verify
   `scripted-t-0001.txt` exactly, write a fixture MERGE report with its exact
   criterion/seal, and invoke real `ha done`.
6. Wait for the real ticker. Inspect the review/task/thread records and run
   `git --git-dir "$HOME/remote.git" cat-file -e main:scripted-t-0001.txt`.

**Expected:** dropping the task before landing prevents its publication from
that review, or the drop is refused before recording success. Any required
review cancellation/rebuild must be durable, not merely a warning the coordinator
has to race against landing.

**Actual:** the command records the task as dropped and returns success, followed
by `t-0001 retirement incomplete (open): t-0001 is in review-1; cancel the review
first`. The review then completes and publishes the dropped member. The worker
is resolved as `merged`; overview has no open tasks or reviews.

**Three separate observations:**
- Injection succeeded: the real task-drop command persisted its timestamp/reason.
- Recovery did not implement the requested retirement: the active-review refusal
  left the member eligible, and the ticker finished publication.
- Semantic outcome is wrong: a task explicitly dropped before landing is on the
  bare remote. The fixture MERGE is only evidence for the original file contract,
  not authority to override the later task drop.

**Durable evidence:** lane library `drop-review-verified.log`, and
`drop-review-verified/local.tar` / `box.tar`, captured before resetting. Paths
inside `local.tar`:
- `.herdr-ade/wall/.state/tasks/job-0001.toml:12-14`: `[[dropped]]`,
  `at = "2026-10-04T07:42:18Z"`, `reason = "Fixture no longer wanted"`.
- `.herdr-ade/wall/.state/reviews/review-1.toml:9,17-18`:
  `phase = "complete"`, `fast_forward = true`, `push = true`.
- `.herdr-ade/wall/.state/threads/t-0001.toml:54,60-62`:
  `resolved_reason = "merged"`,
  `merged_sha = "3ac56caf35b85a7d1ca4918a2e87d004dcedc495"`,
  `merged_review = "review-1"` (install not required).
- `drop-review-verified.log:173-176` records drop/retirement refusal;
  `:258-259` records EXPECTED/ACTUAL, including
  `ACTUAL: dropped task file published = True`.
- `build.txt` records the instance version/hash independently.
- The final executable rerun also fails as expected: `repro-01.log:179,261-262`
  and `repro-01/{local,box}.tar` preserve its independent confirmation.

**Likely cause:** `src/task.rs:482-507` persists `dropped` before attempting lane
retirement. `src/review.rs:2476-2493` refuses that retirement while the review is
active. `src/review.rs:1179-1199` validates criterion evidence without rejecting
a dropped task; `src/review.rs:2088-2100` rechecks the member seal but not task
retirement before moving integration. The unchanged seal passes.

**Smallest suggested fix:** serialize task drop with the active repository review;
for an unlanded review, cancel/rebuild it without the dropped task before the drop
can leave a publishable member. Revalidate task retirement at the landing boundary
so a concurrent drop cannot lose that race. Preserve the seal, report and unique
work; this lane deliberately does not implement the fix.

**Not tested for this finding:** remote reviewer/courier, multi-member exclusion,
a drop after integration already moved, a real provider reviewer, Mac install.
It is a new task-retirement path, not the withdrawn-criterion D29 case.

### PILE-02 — A cancelled sealed lane leaves its task pointing at an empty pile

**Priority:** below PILE-01. No unwanted publication, but the task sends the
coordinator back to a review action that cannot advance it; Rolf may have to
notice the stalled work and ask for another attempt.

**Build:** the same instance-2 base, version and binary hash as PILE-01 above.

**Exact reproduction:**

```sh
tools/wall/findings/pile/repro-02 --instance 2 \
  --evidence "$PWD/.herdr-project/adeherdr-t-0818/library/cancel-projection-new"
```

The executable resets first and returns 1 for the demonstrated wrong next action.
It opens a strict scripted project, starts/seals one worker, and starts a held
reviewer. It checks that cancelling or prompting the member during review is
explicitly refused. Then it pauses automatic allocation, runs `ha review cancel
wall`, runs `ha thread cancel wall t-0001 --reason 'Fixture cancellation'`, and
resumes the project. Finally it runs `ha review wall` and `ha task show wall
job-0001`. Pausing makes the two-command cancellation sequence deterministic;
it does not alter task, seal or review records directly.

**Expected:** preserve the historical seal but show that its attempt was
cancelled, with a next action that can actually advance or retire the task.
Do not instruct the coordinator to review a member excluded from all ready piles.

**Actual:** `ha review wall` says `no ready pile; automatic reviews enabled for
this project`; `ha task show` still says `job-0001 [finished] Scripted fault lane
— next: review the repository pile`. The member is resolved/cancelled, the only
review is cancelled, no review is active, and the remote is unchanged.

**Three separate observations:**
- Injection succeeded: normal review and thread cancellation commands completed;
  `status = "resolved"` and `cancellation_reason = "Fixture cancellation"` persist.
- Cancellation/publication recovery is safe: the remote stays at baseline, and
  `ha review` correctly finds no eligible member.
- Task guidance is wrong: the requested next action cannot use that sealed lane.
  This finding concerns the impossible next action, not erasing the genuine
  historical fact that the worker produced a done seal.

**Durable evidence:** `library/repro-02.log:183-194` records pause/cancel/resume
and unchanged remote; `:254-258` records the no-ready-pile response, cancelled
lane identity and contradictory task instruction. `library/repro-02/local.tar`
contains:
- `.herdr-ade/wall/.state/threads/t-0001.toml:3,55,59,61`:
  resolved status, cancellation reason, `changes_seal = "t-0001-1-1"`, empty
  `merged_sha`.
- `.herdr-ade/wall/.state/reviews/review-1.toml:9,17-18`:
  cancelled phase, `fast_forward = false`, `push = false`.
- `.herdr-ade/wall/.state/tasks/job-0001.toml:8`: sole attempt `t-0001`.
- `.herdr-ade/wall/.state/events/t-0001-1-1.toml:13`: retained sealed SHA
  `328b349c63d33bdc48460ad6560685e216c9ce8b`.

**Likely cause:** `src/task.rs:1209-1223` returns the finished/sealed projection
before reaching the resolved/cancelled handling at `src/task.rs:1248-1256`.
Meanwhile `src/review.rs:646-657` correctly filters resolved members out of the
ready pile. The two views therefore disagree about the available next action.

**Smallest suggested fix:** handle an explicitly cancelled, unlanded attempt
before selecting the task's sealed-work next action. Keep its seal/report as
history and retain separate merged/installed facts; direct the task to a valid
new attempt or explicit retirement instead of an unavailable review.

**Not tested:** a cancelled remote worker, an unsealed cancellation, a multi-attempt
task, or a cancellation after integration moved. No claim that the seal was lost.

## Suspicions (1; not confirmed findings)

**S-01 — Failed lane with a still-live agent may bypass the hold deadline.**
`src/review.rs:2855-2872` releases starting/open non-members after twenty minutes,
but `src/review.rs:2874-2899` appends `failed lane has a live agent` blockers
without applying that deadline. Unknown: whether live recovery can leave such a
lane there indefinitely. This is a potential *failed-status* path related to
D28, not a new report of D28's already-known working-lane issue. No deterministic
live reproduction or claim of failure; ordinary working-lane deadline coverage
is reported separately below.

## Coverage and limitations

Run individual reset-first observations with:

```sh
tools/wall/findings/pile/run --instance 2 --evidence /absolute/new/directory SCENARIO
```

Evidence for each run is `<scenario>.log` and `<scenario>/{local,box}.tar` in
`.herdr-project/adeherdr-t-0818/library/`. A nonzero scenario needs inspection:
it can be a confirmed assertion failure or an incomplete fixture/environment
run. Only PILE-01 and PILE-02 above are promoted to confirmed defects.

| Requested scenario | Scope / evidence |
|---|---|
| Several seals at once | `batch`: two real concurrent sealing helpers, one frozen pile; both land. |
| Seal arriving during review | `batch`: third seal excluded from first candidate, retained for second review, then published. |
| Unchanged reseal | `batch`: same seal identity retained; no duplicate member. |
| Changed reseal | `changed-reseal`: real commit creates seal `t-0001-1-2`; the old candidate stays unlanded with `t-0001 changed during review` in attention. |
| Follow-ups before and after seal | Partial: `cancel-member` checks explicit refusal of a post-seal member correction during active review. Pre-seal delivery and reviewer corrections not covered; scripted agents cannot establish D26/pi idle-delivery behavior. |
| REJECT then retry | Partial: `reject` checks a merits rejection never publishes and does not silently retry the same failed member seal. Corrected member/reseal after that rejection not covered. |
| Evidence-only REJECT | `evidence-reject`: preserves the exact member seal for a fresh reviewer; fixture judgment then lands. |
| Withdrawn conditions | `withdrawals`: criterion 2 withdrawn during review; its stale `established=false` row is ignored, required criterion 1 remains evidenced, and the candidate lands. |
| Dropped task with sealed lane | Complete for the single-member in-flight-review path: PILE-01. |
| Lane cancelled mid-review | `cancel-member` and `repro-02`: direct cancellation is refused with the cancel-review path; cancel review then member leaves remote unchanged. PILE-02 exposes the resulting wrong task guidance. Process loss is a different scenario, not inferred from this. |
| Twenty-minute bound, starts/stops nearby | `hold-bound`: no review at minute 19; third lane starts at 08:25:16Z and is cancelled at 08:25:56Z. Ready seal from 08:06:16Z enters review after 1211.52 observed seconds, excluding the still-running older lane. Durable notice says `20-minute wait bound reached`; exact fixture then lands. No timestamps/clocks edited. |
| Bare remote rejects push | `push-rejection`: genuine divergent bare-remote main; non-fast-forward error recorded in attention; merged=true, push=false, install=false. Repair remote, restart ticker, exact reviewed candidate completes. |
| Disk fills mid-merge | Not covered at the merge boundary in instance 2. The declared wall gate's generic full-disk round is not a substitute. |
| Ticker dies between merge/install | Partial: `push-rejection` stops/restarts the ticker after local merge while publication is blocked. This tests durable landing resumption, not abrupt SIGKILL or an actual install-required repository. |
| Install half-fails | Not covered. Fixture repository has no harness install requirement; mixed-build fault/generic gate runs do not establish partial-install recovery. No production install was attempted. |

The requested invariant is **not established universally**: these runs do not
prove absence of all unreviewed publication, lost seals or unbounded holds. They
establish the specific frozen-membership/publication/recovery checks above and
expose unwanted publication and impossible task guidance. Waiting explanations are checked in the
push, changed-member and working-lane hold observations.

## Gates / final state

See the lane report and `library/gates.log` for exact gate exits, wall-gate
evidence directory and final instance-2 reset. The permanent regression gate
is not weakened or changed; the new failing finding is kept here because this
is a discovery lane, not its repair lane.
