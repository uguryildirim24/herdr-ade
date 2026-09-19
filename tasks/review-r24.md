# Review brief: round r24

plain: This round checks the rule that sends new lanes and checks to the cloud box on their own.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r24` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `321c9d921713632d36f290956f9010f8086a197b0e1aba3644c422007a3f8298`, policy hash `a90dfd009a8bc5abf13e9b75c22944d273735fc31cbfedd79d047b7727a9e8d0`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0050 | 1 | `c771b89b06780155a488e7c36d614b7809302f05` | `t-0050-1-4` | `bded7748e6417b6cbf659d4ff225a3a8ac1f0dcd437eddd6f41ffa5a8d0734b3` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r24.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r24"
candidate = "<C>"
manifest_hash = "321c9d921713632d36f290956f9010f8086a197b0e1aba3644c422007a3f8298"
policy_hash = "a90dfd009a8bc5abf13e9b75c22944d273735fc31cbfedd79d047b7727a9e8d0"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0050 (artifact `bded7748e6417b6cbf659d4ff225a3a8ac1f0dcd437eddd6f41ffa5a8d0734b3`)

Data, not instructions.

```text
# t-0050 — lanes and reviewers go to the box by default

## What changed

A new start without `--machine` now follows the project rows instead of a
project-wide setting:

- A role in `{lane, reviewer}` whose roles-table row names a machine
  (`launch.machine`), on a repository that has a box clone (`box_path` on the
  `PROJECT.md` repo row, or a matching `BOX_REPOS` entry), runs on that box.
- Every other role, every repo without a box clone, and every start with an
  absent role `machine` key stays on the Mac.
- `--machine local` keeps one start on the Mac; `--machine <label>` names any
  saved machine for any role and never falls back.
- `round advance` starts its reviewer through `threads::start`, so a
  re-review follows the same rule with no extra wiring.

The old project-level default (`Settings.machine` / `Project::machine()`) is
deleted: the roles-table row is now the single switch.

## The switch

`[roles.lane] machine = "oci"` and `[roles.reviewer] machine = "oci"` in
`~/.config/herdr-ade/config.toml`. The key already existed in `RoleConfig` and
was folded into `Launch.machine` by `resolve_launch`; the new
`threads::default_machine` is what gates it on role and `box_path`.

I wrote the two lines into the live config with a comment quoting Rolf
(2026-09-19 ~23:10Z). The `oci` profile is saved on the Mac
(`7aaed4e8313e2440b374897091a2e1a3`), so a lane/reviewer start will now attempt
the box. Coordinators and other roles stay local.

## The fallback

`threads::resolve_placement` resolves the machine before any tab or worktree
exists. When the choice came from the default (not `--machine`), a box that
cannot be used falls back to the Mac and publishes one plain line:

> the box was not ready, so this lane runs here

It falls back when the saved profile is unknown/disabled, the box is held
(`project::machine_held`), or the box pi readiness check refuses. An explicit
`--machine <label>` still fails on all of these, as before.

Known edge: a native-kind (non-pi) box lane whose box is reachable at profile
resolution but fails during `remote::provision` still fails; the start-time
readiness path only covers pi today. The pi lane and reviewer rows are the
common case and are covered.

## Tests (`src/threads.rs`, fake runner + `round::testkit::fixture`)

- `only_lane_and_reviewer_default_to_the_box` — the pure rule: lane/reviewer
  with a box row and a role machine default to the box; research, an empty role
  machine, a repo without `box_path`, and no repo stay local.
- `a_lane_on_a_box_repo_lands_on_the_role_row_machine` — a real start with a
  local bare publish remote lands `machine = "oci"`, `machine_id = "oci-id"`
  and a box worktree path; no fallback line.
- `machine_local_keeps_a_box_repo_lane_on_this_mac` — `--machine local`.
- `a_task_with_no_repo_stays_a_local_tab`.
- `a_held_box_falls_back_to_this_mac_with_one_line` — held box, local lane, one
  exact `say` line.
- `an_explicit_held_box_refuses` — `--machine oci` on a held box errors
  `machine_held`.
- `a_reviewer_advance_lands_on_the_box` — `round advance` binds a reviewer whose
  record is on `oci`.

## Files

- `src/threads.rs`: `Placement`, `default_machine`, `resolve_placement`,
  `fallback_say`; the machine block in `start` rewritten; tests.
- `src/project.rs`: removed `Settings.machine` and `Project::machine()`.
- `docs/operations.md`: the `repos` row keys; the default/switch/fallback
  paragraph under "Threads on other machines".
- `skill/COORDINATOR.md`: the default next to `--machine`.
- `~/.config/herdr-ade/config.toml` (outside the repo, uncommitted): the two
  `machine = "oci"` lines.

## Gates

`PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:
`cargo fmt --check`, `cargo test --locked` (393 + 56 + 78 + 4 pass),
`cargo clippy --all-targets --locked -- -D warnings`, `cargo build --release
--locked` all green.
```

