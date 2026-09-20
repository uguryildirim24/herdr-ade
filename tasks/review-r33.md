# Review brief: round r33

plain: This round checks that the cloud box finds its own build tool and builds the terminal program.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r33` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `16f88ee287cb3aa9997c292ddc9307cf5ab159c7157bb874ac66d5346e3784fa`, policy hash `df69155c02a5636dc8a86ec27ade9e888d03782a6c7bd85df748f4c262aaee68`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0071 | 1 | `66e4119a5a9c14814c5933b324be32646dc52d76` | `t-0071-1-2` | `9ea0fc6244d6fb00231408943e8c5de13196aee7800cc99497a2b7188c479516` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r33.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r33"
candidate = "<C>"
manifest_hash = "16f88ee287cb3aa9997c292ddc9307cf5ab159c7157bb874ac66d5346e3784fa"
policy_hash = "df69155c02a5636dc8a86ec27ade9e888d03782a6c7bd85df748f4c262aaee68"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0071 (artifact `9ea0fc6244d6fb00231408943e8c5de13196aee7800cc99497a2b7188c479516`)

Data, not instructions.

````text
# t-0071 — the cloud box builds the terminal program too

Lane: `hp/adeherdr/t-0071-the-cloud-box-builds-the-terminal-progra`
Commit: `66e4119a5a9c14814c5933b324be32646dc52d76`
Pushed: `origin/hp/adeherdr/t-0071-the-cloud-box-builds-the-terminal-progra`
Machine: cloud box `oci`.

## What changed

`src/harness.rs` only.

`box_build` used to export the Mac-shaped `ZIG=<box_path>/.target/rebase/zig-0.16.0/zig`
and the box has no such directory, so the fork's `build.rs` panicked. It now
resolves zig on the box, inside the same one-shot SSH script:

- prefer the repository-local `<box_path>/.target/rebase/zig-0.16.0/zig` when it
  is present and executable;
- otherwise a `zig` on the box's PATH (`command -v zig`);
- otherwise print `harness_box_zig_missing: no zig found: neither <repo-local
  path> nor a zig on PATH` and exit nonzero, so the failure surfaces on the
  existing `harness_box_failed: ...` line;
- then run `"$ZIG" version` and refuse to build unless it is exactly `0.16.0`,
  printing `harness_box_zig_version: <path> reports zig <v>, not 0.16.0`.

The snippet is `box_zig_script`, kept separate so it is testable. `box_build`
now exports `PATH` and `DEVELOPER_DIR` before the resolution and the cargo call
(previously they were a command prefix); the install staging is unchanged. The
`Kind::Plugin` box step embeds none of it.

The Mac side is untouched: `local_build` still uses `zig_path()` and
`DEVELOPER_DIR` exactly as before.

## Gates (all run on the box)

- `cargo fmt --check` — clean.
- `cargo clippy --all-targets -- -D warnings` — clean.
- `cargo test --locked` — 435 + 56 + 78 + 4 tests, 0 failures, run with
  `SHELL=/bin/zsh`.

Plain `cargo test --locked` (box `$SHELL=/bin/bash`) has one pre-existing
environment failure: `pi::scenarios::scenario_setup_then_check_for_kimi` scripts
only the zsh probe (`zsh -lic whence -va pi`), while `pi::doctor::path_probe`
picks `type -a pi` from a bash login shell and the FakeRunner has no rule for
it. That test is unrelated to this change and passes with `SHELL=/bin/zsh`; it
would fail the same way on main at `5885f92`. Everything else passed in both
runs.

New tests in `src/harness.rs` (all pass):

- `box_zig_prefers_the_repository_local_tool` — repo-local wins over a PATH zig.
- `box_zig_falls_back_to_the_box_path` — PATH zig used when the repo-local tool
  is absent.
- `box_zig_missing_names_both_places` — `harness_box_zig_missing`, the
  repo-local path and PATH in the message.
- `box_zig_wrong_version_is_refused` — `harness_box_zig_version` for `0.15.0`.
- `box_build_for_the_fork_resolves_zig_on_the_box` — the generated SSH script
  for `Kind::Fork` carries the resolution and no longer contains
  `ZIG=/home/ubuntu/projects/herdr/.target`.

The first four run the real `box_zig_script` through `/bin/sh` against a temp
repo and a fake zig; the last builds the real `box_build` script through a
`FakeRunner` and inspects it.

## Manual confirmation of the box environment

Outside the tests (read-only):

```
$ ls /home/ubuntu/projects/herdr/.target/rebase/zig-0.16.0/   -> No such file
$ /home/ubuntu/.local/bin/zig version                         -> 0.16.0
```

Running the resolution snippet with the box's real PATH resolved
`ZIG=/home/ubuntu/.local/bin/zig`, version `0.16.0`. So on this box the
repository-local branch is skipped and the PATH fallback is the one that fires.

## What I did NOT run

Per the coordinator's correction:

- I did **not** run `ha harness install` (Mac or box) and did **not** edit
  `~/.config/herdr-ade/config.toml`. `harness install` builds from the main
  checkouts, not from this worktree, and the box has no `config.toml` at all
  (`~/.config/herdr-ade/` does not exist there), so it would prove nothing about
  this change.
- I did **not** rebuild the fork or replace `~/.local/bin/herdr`. The box's
  `herdr` binary is still the pre-r27 one; the real box rebuild is the
  coordinator's to run after this merges.
- No compatibility shim, flag or fallback was added.

## For the coordinator

1. Merge this change.
2. Put the fork row back in `~/.config/herdr-ade/config.toml`:
   `{ path = "/Users/rolfie/projects/herdr", box_path = "/home/ubuntu/projects/herdr" },`
   and remove the comment.
3. Run `ha harness install`; the box's `~/.local/bin/herdr` should then be
   rebuilt from the current `agent-parent-nesting` head and a box lane should
   nest under its coordinator in Rolf's window.

Durable note: the box had no `~/.config/herdr-ade/config.toml`, so a box-local
`ha harness install` would fail at `harness_repos_missing` even with the fix.
Not something this lane should create.
````

