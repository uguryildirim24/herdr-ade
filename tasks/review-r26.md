# Review brief: round r26

plain: This round checks the fresh-session pickup of box lanes and the spin-up of every project.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r26` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `3a59cb0ea225b0e3257c4eb41212d46c51e883439aa54e24c50987b1a638dd79`, policy hash `941e9ea13a16368ca1ee73b91e4a1dc137816aed5c9c2f6e7e6acea90d78dfbd`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0055 | 1 | `c41b270cf910e48771a9b5389f7eb2ce777a6231` | `t-0055-1-2` | `48974e4d41babf24fb40288216232622211f38a457a75265733584b073ce81c7` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r26.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r26"
candidate = "<C>"
manifest_hash = "3a59cb0ea225b0e3257c4eb41212d46c51e883439aa54e24c50987b1a638dd79"
policy_hash = "941e9ea13a16368ca1ee73b91e4a1dc137816aed5c9c2f6e7e6acea90d78dfbd"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0055 (artifact `48974e4d41babf24fb40288216232622211f38a457a75265733584b073ce81c7`)

Data, not instructions.

```text
# t-0055 report

Commit: c41b270

## What `ha pickup` does for a box lane now

- It reads each box lane's machine through the courier (`steps::courier`), the
  same one-SSH-per-machine, box-local `herdr agent list`/`pane list` path the
  ticker already uses, and decides live or gone with `thread::live_state`. A
  machine that fails the pass or does not answer is reported in a `note:` line
  and its lanes are left undecided; they are never invented as gone.
- A live box lane is re-linked: the record keeps its box pane and the parent
  token is written on the box server through `herdr --machine <route> pane
  report-metadata ... --token parent=<mac label>:<coord pane>`. The local label
  is `Local`, so a live box lane gets `parent=Local:w1:p1` and is compared
  against that value to count as already linked.
- A fresh box lane start (`place_box_worktree`) writes the same
  machine-qualified parent token on the new box pane right after the tab is
  created, before the ticker starts the agent, so the lane nests from its first
  second and survives `agent start` (which passes no `--parent` for a box lane).

**One caveat, one line:** fork lane t-0053 is still a live lane and its
machine-qualified `parent` parsing is not merged/installed yet, so the token is
written in the new form now but will only draw the lane under the Mac
coordinator once that fork change lands. I chose to write the new form rather
than the bare pane id because the brief's test names the machine-qualified
token; if you want the bare id until t-0053 merges, `threads::parent_token`
is the one place to flip.

## `--all` and `--start`

- `ha pickup --all` runs the pass for every non-archived project under the
  root, one output section per project (`project <slug>`), and one courier pass
  per machine across those projects.
- `--start` restarts gone lanes through the existing restart path
  (`threads::restart`) instead of printing start lines. It acts only for a
  project whose `[safety] start_threads = "auto"`; otherwise it prints, as
  today. Live relinked lanes are never in the gone list, so a `--start` pass
  does not restart them; `--dry-run` suppresses starting too.
- Gone box lanes print copy-ready lines with `--machine <label>`:
  `herdr --machine box tab create ...` and `herdr --machine box agent start ...
  --parent Local:<coord pane> ...`.

## Tests

All in `src/checkpoint.rs`, with the fake runner and the courier `ssh` manifest
fake:

- `pickup_relinks_a_live_box_lane_with_the_machine_qualified_parent`
- `pickup_prints_a_machine_start_line_for_a_gone_box_lane`
- `pickup_all_covers_two_projects`
- `pickup_start_starts_only_under_auto_and_skips_a_relinked_lane`
- updated `pickup_relinks_live_threads_and_prints_start_lines_but_never_starts`

Gates, with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:
`cargo fmt --check`, `cargo test --locked` (531 tests, 0 failed),
`cargo clippy --all-targets --locked -- -D warnings`, `cargo build --release
--locked` all pass.

Also updated `docs/operations.md` (pickup row plus three sentences in "Threads
on other machines") and `skill/PICKUP.md` step 4 for the new `--start`.
```

