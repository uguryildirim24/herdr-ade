# Review brief: round r23

plain: This round checks the fix that gives every box command the box's own tool path.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r23` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `93d65de3ee298b3fa7d84d211c251e026480ffb51f23906032ea5cfe8836cac3`, policy hash `d52d28b4e29dbab7ff979a291318f9d2ce7dde6ef6e2530db63b74d55c16369f`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0049 | 1 | `0f190f7694d33f0bb26b6474a108d1780b865400` | `t-0049-1-2` | `8eaad392764e96fe70e0c7872729846add091963dc7e4f0311917420c261da69` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r23.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r23"
candidate = "<C>"
manifest_hash = "93d65de3ee298b3fa7d84d211c251e026480ffb51f23906032ea5cfe8836cac3"
policy_hash = "d52d28b4e29dbab7ff979a291318f9d2ce7dde6ef6e2530db63b74d55c16369f"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0049 (artifact `8eaad392764e96fe70e0c7872729846add091963dc7e4f0311917420c261da69`)

Data, not instructions.

```text
# t-0049 — every box command runs with the fixed box PATH

## What changed

Added `contracts::with_box_path(script)` (`src/contracts.rs`, next to
`box_prefix`): it prepends `PATH=<BOX_PATH>` to a box `sh -c` script. Applied
it in the only two places the plugin builds an ssh command:

- `remote::ssh` (`src/remote.rs:154`) — covers `check_on_machine`
  (`src/pi/ade.rs`), the Mac doctor's box helper (`src/doctor.rs`), `provision`,
  `provision_card`, `fetch_file`'s `ssh cat`, and the remote worktree removal
  (`src/threads.rs`).
- `remote::ssh_courier` (`src/remote.rs:274`) — the courier helper
  (`src/steps.rs`).

Together these are every `remote::ssh`/`ssh_courier` caller from
`grep -n 'remote::ssh' src`; there is no other `Cmd::new("ssh")` in the crate.

`PATH=...` in front of a script that opens `set -e`/`set -u` shifts `PATH` for
the whole script because `set` is a POSIX special builtin and the assignment
persists (verified in dash and in bash-as-sh). The pi script
`HERDR_ADE_ROOT=... herdr-pi check ...` gets both variables on the command.

## Tests

- `pi::ade::the_box_readiness_check_calls_the_pi_binary_with_the_provider`:
  updated, not deleted — the pinned exact script text now starts with
  `PATH=/home/ubuntu/.local/bin:...`.
- `doctor::box_rows_read_the_box_and_gate_on_free_disk`: added an assertion
  that the box helper script starts with `sh -c 'PATH=<BOX_PATH>`.
- New `remote::every_ssh_script_carries_the_box_path`: `ssh` and `ssh_courier`
  both emit `sh -c 'PATH=<BOX_PATH> true'`.

## Docs

`docs/operations.md` "Threads on other machines": one sentence that every box
command runs over SSH with the fixed box PATH, so a fresh box needs no
login-shell PATH edits.

## Gates

With `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:
`cargo fmt --check`, `cargo test --locked` (525 tests across four binaries),
`cargo clippy --all-targets --locked -- -D warnings` and
`cargo build --release --locked` all clean.

## Durable lesson

None for memory. The fix is in the harness code, not a workaround.
```

