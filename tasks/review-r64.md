# Review brief: round r64

plain: This round fixes your talk tab, keeps me reachable, starts checks the right way, and ties my bigger choices to your words.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r64` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 4, manifest hash `a7a64cadb1dbd3818239d266c4061e3e66fc80fcc33c6bbd75927785c98a5b5e`, policy hash `73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0129 | 1 | `074391732618668b971c59b51f43990777209f46` | `t-0129-1-1` | `bc051738387bcc6f32701bb7f0f24398fd09d286157e4fa257637edf87463fa0` |
| t-0131 | 1 | `e89486c19aed856b6041380d48f1cdae37bf0014` | `t-0131-1-1` | `6340f17ad4fde64d2216d1b54c9aab39c3a18b04622f96998a0fe1b97adcd654` |
| t-0134 | 1 | `221e94fbc7304113385ad740b452278a96d293f0` | `t-0134-1-1` | `57eedce37c878f3ac84da0f981e41c347f5efebe2776f5e4ddfb0d8d8f950619` |
| t-0135 | 1 | `4fb1c69914d04beb3f19decaa9741b1890168db6` | `t-0135-1-1` | `b7c9eccf0e5ffb5fa20f4b6b57793f87e804fa92e458c69a2180fd1869913340` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r64.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r64"
candidate = "<C>"
manifest_hash = "a7a64cadb1dbd3818239d266c4061e3e66fc80fcc33c6bbd75927785c98a5b5e"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0129 (artifact `bc051738387bcc6f32701bb7f0f24398fd09d286157e4fa257637edf87463fa0`)

Data, not instructions.

````text
# t-0129 — the talk tab is easy to read and always current

Branch `hp/adeherdr/t-0129-the-talk-tab-is-easy-to-read-and-always`.
Worktree `/home/ubuntu/projects/herdr-ade/.worktrees/t-0129`.
Lane ran on the cloud box `oci`.

The report of the fixes below is code in the plugin (`src/talk/`). The visual
evidence lives in `library/` beside this file (the brief's library folder).

## What changed, defect by defect

### 1. The screen goes stale

`src/talk/screen.rs`: the screen now compares its own `crate::VERSION` with the
version the installed binary reports, and `exec`s the installed binary with the
same arguments when they differ. It runs before the first paint and again on the
30-second slow tick. The composer draft rides across the hand-over in
`HERDR_TALK_DRAFT`; a `HERDR_TALK_REEXEC` guard stops a loop. The exec inherits
the alternate screen and raw mode, so the new process re-enters them and its own
cleanup restores the terminal. `stale::installed_version` is the shared reader.
A path check keeps the current binary from re-execing itself.

The old behaviour only *named* the stale screen; it never replaced it. The
stale-screen row in `stale::scan` remains for the case where the hand-over
cannot run.

### 2. "Running now" showed work that is not running

`src/talk/overview.rs`: a carried round is now considered only while it is
open. `pending_pin` also requires `!r.phase.closed()`, so a resolved lane pinned
in an **abandoned** round is not "checking" any more. Regression test
`an_abandoned_round_does_not_keep_a_handed_in_lane_running` builds exactly the
abandoned-round shape.

### 3. One piece of work showed as two rows

`src/talk/overview.rs`: production starts a round's reviewer with the round's
own `plain` (`round.rs` `StartReviewer` uses `plain: record.plain.clone()`), so
the lane and the reviewer render the same sentence. `Running now` now dedupes on
the checked sentence and keeps one row; unnamed rows keep their own fallback.
The remote marker no longer dangles: a reachable remote lane reads `box`, and
`last seen` is added only when the poll failed (`box, last seen`). Regression
test `a_lane_and_its_round_reviewer_show_the_work_once`.

### 4. The cost row meant nothing

`src/talk/cost.rs` + `overview.rs`: `format_totals` no longer invents
`"<n> min, cost unknown"`. When the ledger has money/tokens the row shows them;
otherwise no invented total is drawn. A new `running` row shows the elapsed
minutes of the work open right now (`Cost.running`, measured from each open
lane's own start). `the_cost_rows_show_money_or_running_time_not_an_invented_total`
covers both. The `after` capture shows `today 120.0k tokens, $3.75 …` and
`running 12 min so far`.

### 5. The chat repeated itself

`src/talk/view.rs`: a landing sentence keyed by `landed_round` reads once, and
an ordinary `say` repeated back-to-back with nothing between reads once.
Regression test `a_repeated_say_line_reads_once`.

### 6. The question card was squeezed

`src/talk/screen.rs`: the wide layout now gives the selected card as many rows
as it needs before sizing the overview (`body.height - card_h - 2`, floored at
6 and capped at 26). The trailing blank row after the last pinned card is gone.
The card is fully readable at the default sizes without scrolling;
`the_selected_question_fits_at_the_default_sizes` asserts
`app.questions.max() == 0` at 120×40, 134×40 and 60×40.

### 7. Header counts disagreed with the records

`src/talk/overview.rs`: `<n> open tasks` is now the count of open entries in
`TASKS.md` (the `Tasks` section's record), and `<n> need you` is the number of
open coordinator questions. Before, `open tasks` counted every active thread and
`need you` added `WaitingOnYou` lanes to the asks, which is how the header could
say 62/3 against 19 task lines and 2 questions. `task_rows` now returns its
count. The existing poll test was updated to write a task list.

### 8. Raw mouse escape bytes leaked

Reproduced on the box by sending the SGR report in two writes (`ESC`, then
`[<64;15;5M`): crossterm reads the lone ESC as `Esc` and the rest as text, and
the composer printed `[<64;`. `src/talk/screen.rs` now collects the characters
after an `Esc` and swallows a complete or partial SGR mouse report; anything
that is not a report is flushed as ordinary text, so a real clear still works.
Regression test `a_split_mouse_report_is_not_typed_into_the_composer`.

### v1 defect: the empty date rule

`src/talk/screen.rs`: the injected `request_waiting` notice has no time, so the
timeline drew `── ──`. A date divider is now drawn only when the date is
non-empty. Regression test `an_untimed_notice_does_not_draw_an_empty_date_rule`.

### Box staleness check (f-0094)

`src/talk/stale.rs`: `server_stale` sent the Mac's binary path over SSH to the
box, so the box check always died with `sh: 2: /Users/rolfie/.local/bin/herdr:
not found` (exit 127) and silently returned "current". It now asks the box for
its own `herdr` on the box `PATH` (the `with_box_path` path already contains
`/home/ubuntu/.local/bin`). `server_stale` returns `Option<bool>`; an unreadable
check is `None`, which sets `Stale.unknown` and shows `Some running programs
could not be checked.` in the overview instead of staying silent. Tests
`the_box_server_check_asks_the_box_for_its_own_herdr` (asserts no
`/Users/rolfie` crosses to the box) and
`an_unreadable_server_check_is_unknown_not_current`.

On the box the fixed command resolves and reports current:

```
$ PATH=/home/ubuntu/.local/bin:/home/ubuntu/.cargo/bin:/usr/local/bin:/usr/bin:/bin \
    herdr status server --json
{"status":"running","running":true,"version":"0.9.1",…,"restart_needed":false,"server_binary_stale":false}
```

## Visual evidence

Scratch root: `.herdr-project/adeherdr-t-0129/scratch` (built by a temporary
test that was removed before the commit; its shape is a plan with two done
steps, two open asks, a resolved lane pinned in an abandoned round, an open
lane admitted to an open round, a reviewer thread with the open lane's sentence,
a `TASKS.md`, and pi usage for money).

The **before** captures were taken from the stashed pre-change build; the
**after** captures from the final build. Wide is 120×40 and narrow is 60×40 (the
box's outer terminal is 120 columns; the pane pty was set to 60 with
`stty cols 60`). The visual checks ran in an isolated named herdr session
(`herdr --session scratch-t0129 …`), which was stopped and deleted afterwards;
no scratch workspace or tab remains in the watched session.

Captures:

- `library/before-wide.txt`, `library/after-wide.txt`
- `library/before-narrow.txt`, `library/after-narrow.txt`

Before (wide), the defects in one frame:

```
 Running now
 checking Check the abandoned path. last seen
 working Show pretend trades on the screen. box last seen
 working Show pretend trades on the screen. last seen
…
── ──
       ! Your message waits until the coordinator is ready.
…
0 I did not understand the       │
question                more below
```

After (wide):

```
 Cost
 today 120.0k tokens, $3.75, 6 min, some cost unknown
 round r3 120.0k tokens, $3.75, 0 min
 running 12 min so far
 Running now
 working Show pretend trades on the screen. box, last seen
…
       ╰─────────────────────────╯
       ! Your message waits until the coordinator is ready.
…
│ 0 I did not understand the       │
│ question                         │
│ asked 13:54                      │
╰──────────────────────────────────╯
```

The empty date rule is gone, the panel shows the full card, the abandoned lane
and the duplicate are gone, and the marker is plain.

## Gates (on the box)

All run in the worktree, no `PATH`/`DEVELOPER_DIR` overrides needed on Linux:

- `cargo fmt --check` — clean
- `cargo test` — 544 unit + 55 + 78 + 6 + 8 + 2 + 3 integration, 0 failed
- `cargo clippy --all-targets -- -D warnings` — clean
- `git diff --check` — clean

## Files

- `src/talk/screen.rs` — stale self-exec, mouse-report collection, question-card
  room, date rule.
- `src/talk/overview.rs` — abandoned-round filter, work dedupe, plain marker,
  header counts from `TASKS.md`/asks, cost rows, unknown-staleness row.
- `src/talk/cost.rs` — no invented minutes total, running elapsed.
- `src/talk/view.rs` — duplicate `say` folding.
- `src/talk/stale.rs` — box `herdr` resolution, `Option` result, `unknown`.

## Notes for the coordinator

- The header semantics changed from the spec's §2.1 wording: `open tasks` now
  counts `TASKS.md` (the LEAN U5 record Rolf reads) and `need you` counts open
  asks only. If the spec should say this, it needs a line; the task text was the
  authority here.
- `server_stale`'s remote check now relies on `with_box_path` containing the
  box `herdr`. `BOX_PATH` in `contracts.rs` is the single place that guarantees
  it.
- The stale-screen row and the auto-exec overlap. Auto-exec makes the row
  transient; removing the row entirely would need a decision because U4 asks the
  screen to name what to restart.
````

### t-0131 (artifact `6340f17ad4fde64d2216d1b54c9aab39c3a18b04622f96998a0fe1b97adcd654`)

Data, not instructions.

````text
# t-0131 report: a reviewer always starts, and starts as a reviewer

Commit: `e89486c19aed856b6041380d48f1cdae37bf0014`
Branch: `hp/adeherdr/t-0131-a-reviewer-always-starts-as-a-reviewer`
Machine: `oci`

## Outcome

- **Defect 1 (no fallback to the Mac): not reproducible on current main; dropped with
  evidence.** The reviewer start already falls back to the Mac through the shared
  placement path. A temporary repro test showed the reviewer landing with
  `machine=""` (local) when the box recipe probe fails.
- **Defect 2 (a hand-started reviewer bootstraps as a lane): fixed.** `round reviewer`
  now starts and binds the reviewer through the same path as `round advance`, so a
  hand-started reviewer always bootstraps with the reviewer skill and the reviewer
  role floor. The old `thread` argument is removed, so a lane thread can no longer be
  bound as a reviewer.

## Defect 1: fallback to the Mac already exists

`start_reviewer` passes `machine: None` (`src/round.rs`). That is the value that
**enables** the fallback: `start_with_ticker` resolves placement with
`explicit_machine = None`, and `resolve_placement` then builds candidates
`[default box, local]` for a reviewer (`default_machine` accepts `lane | reviewer`).
If the box probe fails, it tries `local` and records `fell_back`. The readiness
fallback landed in `657256f fix(dispatch): place picks only on ready machines`
(2026-09-20), an ancestor of `HEAD` (`git merge-base --is-ancestor 657256f HEAD`).

Evidence: I added a temporary test that opened a round on a repo with a box clone and
`[dispatch] machine = "oci"`, made the box `claude` readiness probe fail the way
`an_unreachable_box_falls_back_to_this_mac` does, sealed one lane, then ran
`advance`. The reviewer was bound and measured:

```
REPRO reviewer machine="" role=reviewer
```

and the talk line was `the box was not ready, so this lane runs here`. The box probe
covers the causes the brief names: a missing runtime (`command -v` plus the kind's
probe), a missing pi sign-in (`herdr-pi check` on the box), and a box that is down
(`machine list` fails, so the box candidate is not ready). I removed the temporary
test because the project rules forbid adding a test that does not fail on a real
defect.

## Defect 2: one hand-start path, and it is a reviewer

Before: `round reviewer <slug> <round> <thread>` only recorded an existing thread.
The coordinator had to start that thread itself; the natural `thread start` has
`workflow = "lane"`, so `skill reviewer` refused with `bootstrap_mismatch`
(`src/lane.rs`) and the routing floor for the reviewer role did not apply. The
recovery text ("start it by hand with `round reviewer`") did not even name the
missing `thread start --workflow reviewer` step.

After: `round reviewer <slug> <round>` starts the reviewer itself and binds it. It
reuses the exact `round review` -> `start_reviewer` path that `round advance` uses, and
`start_reviewer` sets `workflow = "reviewer"`, so the thread gets `skill/REVIEWER.md`
and the `role_floors` reviewer floor (`pi_codex_sol_high` in the live policy). There is
no CLI way left to bind an arbitrary thread, so the wrong move is impossible rather
than merely discouraged.

Design choices:
- `start_reviewer_by_hand` refreshes pins, refuses a live bound reviewer, requires
  every lane pinned and at least one pin not yet landed, then reuses
  `reviewer_branch`/`start_reviewer`/`bind_reviewer`.
- I extracted `reviewer_branch` out of `advance` (the review-is-current test) and both
  paths call it, so hand and automatic starts can never disagree about which review
  branch to start from. `advance`'s diff is 8 lines replaced by 1 call; no round
  phase was restructured (t-0128 works in the same file).
- `--workflow reviewer` still exists as a generic `thread start` flag, but it is no
  longer part of starting a round reviewer, so the skill and docs name `round reviewer`
  only.

Files changed:
- `src/round.rs`: new `pub fn start_reviewer_by_hand`, extracted `reviewer_branch`,
  recovery/attention messages name `round reviewer <slug> <round>`, new test
  `round_reviewer_starts_a_reviewer_by_hand`.
- `src/cli.rs`: `RoundCommand::Reviewer` is now `{ slug, round }` and calls
  `start_reviewer_by_hand`; `round review` names `round reviewer <slug> <round>`.
- `skill/COORDINATOR.md`, `skill/REVIEWER.md`: name the one way and nothing else.
- `docs/operations.md`: same.

Reproduction of the fixed behavior: `round_reviewer_starts_a_reviewer_by_hand` seals a
lane with no `advance`, calls `start_reviewer_by_hand`, and asserts the new thread has
`role == "reviewer"`, a non-empty base, the committed brief and `skill reviewer` in its
task, the round record binds it, and a second call refuses with
`reviewer_already_bound`. On the old code the reviewer thread was whatever the
coordinator started, so the role assertion is the defect.

## Gates

Run on `oci` with `PATH=/bin:$PATH`:

- `cargo fmt --check` — clean.
- `cargo test` — pass: 535 main, 55 pi, 78 pro, 6 cli, 8 context_actionable,
  2 context_records, 3 routing_cli; 0 failed. Main gained the one new test (534 -> 535).
- `cargo clippy --all-targets -- -D warnings` — clean.
- `git diff --check` — clean.

## Notes for the coordinator

- Durable lesson: the brief's premise that `machine: None` means "box only" was
  outdated. `657256f` had already made reviewer placement try the box then fall back
  to the Mac; the next brief about reviewer placement should check
  `resolve_placement`/`tried`/`ready` first.
- The automatic reviewer path is unchanged; only the manual repair path changed
  shape. A coordinator that had a manual 3-argument `round reviewer` line cached will
  get a clap usage error and then the correct two-argument form from `--help`.
````

### t-0134 (artifact `57eedce37c878f3ac84da0f981e41c347f5efebe2776f5e4ddfb0d8d8f950619`)

Data, not instructions.

```text
# t-0134 — The coordinator can always be woken

## What was wrong

Two independent paths leave a coordinator unreachable.

**1. The name is dropped while the agent keeps running.** `herdr agent start`
clears the pending agent name when interactive readiness times out
(`src/cli/agent.rs` `wait_for_named_agent` → server `reconcile_managed_agent_at`),
and a live server handoff clears it on respawn
(`clear_agent_runtime_identity_after_respawn`). The process is still in the pane.
The plugin's launch path only printed "the coordinator agent is not ready yet"
and relied on `tick_slow` to relaunch — but that retry is gated on
`!pane_has_agent`, and the pane *does* host an agent. So the name never came
back, `coordinator::agent_matches` never matched, and every finished lane and
inbox item went unreported. `ha open` made it worse: with a nameless agent in
the bound pane, `reusable` was false and `open` created a second coordinator
tab.

**2. Nobody was told.** `nudge` defaulted to `false`, so the ticker showed one
desktop notification and never prompted. A coordinator that did not read its
inbox stayed unread.

## What changed

- `src/coordinator.rs`
  - `agent_on_pane(record, agent)` is `agent_matches` without the name; the
    restore path needs the pane identity when the name is gone.
  - `restore_agent_name(project, herdr, record, agents)` puts the recorded name
    back on the agent in the bound pane, counts it in the coordinator record,
    and returns the agent. It is used by `open` and by the ticker.
  - `open` now calls it before deciding the coordinator is gone, so a dropped
    name reuses the pane instead of opening a second coordinator.
- `src/herdr.rs`: `Herdr::agent_rename(target, name)` (`herdr agent rename`).
- `src/ticker.rs`: the cheap pass calls `restore_agent_name` every tick, before
  priming, tokens, delivery and nudge. The ticker state is saved whenever
  anything changed, not only `nudged`.
- `src/project.rs`: `Coordinator.name_restored` counts restores (serde default,
  so old `coordinator.json` loads). `Settings::default().nudge` is now `true`;
  an explicit `nudge = false` in `PROJECT.md` still turns prompting off.
  Template wording updated.
- `src/steps.rs`: `State.unread_passes` counts ticker passes where the announced
  set stayed unread (serde default, old `ticker.json` loads).
  `announced_unread(project)` returns the pass count only while the live unseen
  set still hashes to the announced set, so a stale counter cannot fail a
  project whose items are now read. `UNREAD_NUDGE_PASSES = 3`.
- `src/doctor.rs`: the project row now reads `agent list` and fails when the
  recorded name does not resolve to the bound pane, and fails when
  `announced_unread` reports several unread passes. A matching row also prints
  `name restored Nx`.

## Tests added (each fails on the named defect)

- `ticker::tests::a_name_dropped_while_the_agent_runs_is_restored_on_the_bound_pane`
- `coordinator::tests::identity_needs_ids_cwd_and_name` (extended: `agent_on_pane`)
- `project::tests::front_matter_parsing` (extended: default is `true`)
- `scenarios::an_announcement_that_stays_unread_is_counted_by_ticker_pass`
- `doctor::tests::a_coordinator_whose_name_does_not_resolve_fails_the_project_row`
- `doctor::tests::announced_items_unread_across_passes_fail_the_project_row`

`scenarios::with_nudge_off_...` now sets `nudge = false` explicitly, since the
default changed.

## Gates

Run in `/home/ubuntu/projects/herdr-ade/.worktrees/t-0134` (cloud box `oci`;
the Mac `DEVELOPER_DIR`/`ZIG` prefix does not apply here).

- `cargo fmt --check` — clean.
- `cargo clippy --all-targets -- -D warnings` — clean.
- `git diff --check` — clean.
- `cargo test` — all green:
  - 538 passed (bin `herdr-ade`)
  - 55 passed (bin `herdr-pi`)
  - 78 passed (bin `herdr-pro`)
  - 6 + 8 + 2 + 3 passed (integration tests)

## Published

- Branch `hp/adeherdr/t-0134-the-coordinator-can-always-be-woken` pushed to
  `origin` (`uguryildirim24/herdr-ade`), `29d2244..221e94f`.
- Commit `221e94f`.
```

### t-0135 (artifact `b7c9eccf0e5ffb5fa20f4b6b57793f87e804fa92e458c69a2180fd1869913340`)

Data, not instructions.

````text
# A choice that changes what Rolf gets points at what he said

## Result

Every message Rolf sends the coordinator now has a request id he can cite, whether it
arrived in the talk tab or was typed straight into the coordinator pane, and a
`what-you-get` / `money` / `undo` decision is still refused unless its `--basis`
references one that exists.

The talk tab already recorded `Entry::Rolf { request, text }` for its own messages and
used `q-...` ids. The missing half was the coordinator pane: prompts typed there never
reached `talk/journal.jsonl`, so `--basis request:<id>` could never resolve and
consequential choices were being logged as `routine`.

No second journal was added. Pane messages join the same journal and are folded by the
same replay/context readers.

## What changed

- `src/hook.rs`
  - A Claude Code coordinator now gets a `UserPromptSubmit` hook next to its `Stop`
    hook. It runs the same `ha plain hook` command with `--phase prompt`. Install,
    verification and removal all share one `owned_events` table, so only `claude` gets
    the new event (`codex`/`cursor` keep their existing shapes).
  - `run` handles `phase == "prompt"` before the end-of-turn check: it classifies the
    prompt, records Rolf's words verbatim under a fresh `q-...` id when they are his,
    prints `request <id>` (Claude Code adds UserPromptSubmit stdout to the turn
    context), and returns without running the plain check or marking requests accepted.
  - `captures(project, pane)` reports whether a prompt-submit hook is bound to the
    pane, so the talk layer does not litter marker files for kinds without one.
- `src/talk/mod.rs`
  - `record_pane_request` writes one `Entry::Rolf { request, text, answer: None }` for a
    pane prompt; `fresh_request` is now shared with `submit_with_answer`.
  - A prompt can be marked before it is typed: `mark_talk_delivery` (a talk delivery
    that `submit` already recorded) or `mark_automated_prompt` (a harness line).
    `take_pending_prompt` reads and removes the marker for the exact pane + prompt and
    ignores a stale one after two minutes. Markers live under
    `talk/prompts/<sha256(pane + text)>.json`.
  - `deliver_queued` marks each talk delivery, so the prompt hook cites the request the
    talk tab already recorded instead of appending a duplicate.
  - `recent_requests` lists the latest Rolf messages with their ids.
- `src/steps.rs`, `src/ticker.rs`, `src/coordinator.rs`
  - Every automated line typed into the coordinator pane (priming, nudge, `DONE` /
    `WAITING` / `FAILED` events, remote `BLOCKED` / `GONE` lines) is marked first, so
    the prompt hook never records it as Rolf's.
  - `digest_snapshot` prints the latest five messages under
    `## Latest messages from Rolf — cite one with --basis request:<id>`.
- `skill/COORDINATOR.md` and `docs/operations.md` say where the id comes from (the
  hook line, or the context section) and that `--basis request:<id>` points at it.

`decide.rs` did not change: the three consequential classes already refuse with no
`--basis`, and `validate_basis` already refuses a `request:<id>` that no human message
carries. The defect was that no id existed to satisfy it. `ask:<id>@<revision>` support
is unchanged.

## Tests

- `hook::tests::a_pane_prompt_gets_a_request_id_but_a_harness_line_does_not` — a marked
  harness line is not recorded; an unmarked prompt gets a `q-...` id, keeps its text
  verbatim, and satisfies `ha decide --class money --basis request:<id>`.
- `hook::tests::a_talk_delivery_reuses_the_request_the_talk_tab_recorded` — a marked
  delivery returns the existing id and appends nothing.
- The existing Claude install test now also checks the `UserPromptSubmit` entry is
  installed once with `--phase prompt` and removed cleanly.

## Gates

```
cargo fmt --check                                          clean
cargo clippy --all-targets -- -D warnings                  clean
cargo test                                                 536 + 55 + 78 + 6 + 8 + 2 + 3 passed, 0 failed
git diff --check                                           clean
```

## Real run (scratch root)

Built binary, `--root /tmp/t0135/root`, no live `~/.herdr-ade` or `~/.config/herdr-ade`
touched:

```
$ echo '{"session_id":"s1","prompt":"Spend five dollars on the check."}' \
    | HERDR_PANE_ID=w1:p1 herdr-ade --root /tmp/t0135/root plain hook \
      --kind claude --project demo --binding w1:p1 --phase prompt
request q-1789999277937-716651

$ tail -1 demo/talk/journal.jsonl
{"seq":1,"at":"2026-09-21T14:01:18Z","rolf":{"request":"q-1789999277937-716651","text":"Spend five dollars on the check."}}

$ herdr-ade --root /tmp/t0135/root decide "I will spend five dollars on the check." \
    --class money --basis request:q-1789999277937-716651 --project demo
d-0001 money

$ ... same line, --class money, no --basis        -> decision_authority, refused
$ ... same line, --basis request:q-bogus          -> decision_basis, refused

$ herdr-ade --root /tmp/t0135/root context demo --peek | grep -A1 "Latest messages"
## Latest messages from Rolf — cite one with --basis request:<id>
- q-1789999277937-716651: Spend five dollars on the check.
```

## Notes for the coordinator

- The new hook is installed on the next `ha open` for a `claude` coordinator; existing
  panes keep the old `.claude/settings.local.json` until they are reopened. No
  migration shim was added.
- The prompt hook is Claude-only on purpose. Codex/Cursor hook shapes were left
  untouched rather than guessing a prompt-submit event for them; their pane messages are
  still not recorded. The context section does surface talk-tab ids for every kind.
- The two-minute marker TTL is the only heuristic: a marker whose hook never fired is
  ignored rather than allowed to claim a later identical prompt.
- The working tree is clean except for this report and `library/` under
  `.herdr-project/`, which are not committed.

Pushed lane branch `hp/adeherdr/t-0135-a-choice-that-changes-what-rolf-gets-poi` at
`4fb1c69914d04beb3f19decaa9741b1890168db6`.
````

