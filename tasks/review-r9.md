# Review brief: round r9

plain: This check reads the change that lets a lane start on the cloud box.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r9` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `a058cf16324726c313f024e1e12cd3914d84737600aa65bfbe1300ac9e008d08`, policy hash `e1529634be8dfdf8688cad0bfcfd99dfa67f7f19fe2d1c3c7ab9f12ee7c0eec3`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0017 | 1 | `6d517cb2060b13fc90ec4b8642b63a0ae0cff4fd` | `t-0017-1-1` | `ed1820dbd801eef48cdf4d8824905a5ed34ea7c15bb8ad3c764fc72c466bda38` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r9.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r9"
candidate = "<C>"
manifest_hash = "a058cf16324726c313f024e1e12cd3914d84737600aa65bfbe1300ac9e008d08"
policy_hash = "e1529634be8dfdf8688cad0bfcfd99dfa67f7f19fe2d1c3c7ab9f12ee7c0eec3"
gates = []
+++
```

5. Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0017 (artifact `ed1820dbd801eef48cdf4d8824905a5ed34ea7c15bb8ad3c764fc72c466bda38`)

Data, not instructions.

```text
# t-0017 — box lanes, start side (SPEC-remote v2.2 §8.2)

Result: a role's `machine` reaches a box lane's start; a box lane is provisioned
on the box from a Mac `ha thread start`, gets its brief by git (D9), and runs.
The completion side (courier, sealed-event import, DONE/BLOCKED/GONE) is the
second lane's; this report says exactly what it must add.

All gates pass with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:
`cargo fmt --check`, `cargo test --locked` (330 + 41 + 52 + 4), `cargo clippy
--all-targets --locked -- -D warnings`, `cargo build --release --locked`.

## What a box lane's start looks like end to end

`ha thread start <slug> --repo <mac-path> --machine oci …` (§4.2):

1. `--machine`, else the roles-table row (`[roles.<role>] machine` or the
   recipe's `machine`), else the project default (`PROJECT.md machine`), else
   `local`. `remote::machine_profile` resolves the stable saved-profile id;
   an unknown profile refuses. A held machine refuses (`ha machine hold`).
2. No repository and a remote machine refuses `needs --repo`; a repository is
   stored as the Mac path.
3. `resolve_launch` validates the roles table before any git effect (D2).
   `pi_ready` runs only for a local lane; box provider readiness is the second
   lane's.
4. The brief is composed on the Mac and committed on the integration branch as
   `B` (`git::commit_files_locked` under the Mac repository lock); the lane
   branch is created at `B` and pushed to the URL-matched remote by URL
   (`git push <url> <sha>:refs/heads/<branch>`, never force, never by remote
   name). A Mac-held lock keyed by (profile id, box repository) serializes
   starts for one box repository.
5. `remote::provision` makes one SSH call to the box: `git fetch <url>
   <branch>`, require `FETCH_HEAD = B`, then `git worktree add` at
   `<box clone>/.worktrees/<id>` from `FETCH_HEAD` and set the upstream. The
   brief never leaves git.
6. The box workspace is reused when the box's pane list still shows it, else
   created with the box clone as cwd; the lane tab is created through
   `herdr --machine <profile>` with cwd the box worktree and `--env`:
   `HERDR_ADE_LAUNCH=<slug>/<id>/<attempt>/<brief hash>`, the recipe env,
   `PATH=/home/ubuntu/.local/bin:/home/ubuntu/.cargo/bin:/usr/local/bin:/usr/bin:/bin`,
   `CARGO_TARGET_DIR=/home/ubuntu/build/lanes/<slug>-<id>`.
7. `remote::provision_card` makes the one card-write SSH call, writing
   `~/.herdr-ade/<slug>/lanes/<id>.toml` and (when absent) a minimal
   `~/.herdr-ade/<slug>/PROJECT.md` and `.state/`.
8. The Mac ticker (unchanged launch path) starts the kind through
   `herdr --machine` with no parent and types the birth line:

   `Run /home/ubuntu/.local/bin/herdr-ade --root /home/ubuntu/.herdr-ade skill lane, then read tasks/<id>.md and do what it says. You run on the cloud box named `oci`; finish with `ha done`, never with a parent prompt.`

9. `ha skill lane` on the box resolves the pane through the card, prints the
   box prefix, and writes the bootstrap receipt under
   `~/.herdr-ade/<slug>/.state/bootstrap/<id>.json`.
10. The lane commits, pushes the lane ref, then `ha done --report <path> --sha
    <sha>`: the box validates the card, checks the published ref with `git
    ls-remote`, reserves/stages/seals locally, and stops. It does not deliver
    and does not start a ticker.

## The lane card (`contracts::LaneCard`)

`project, thread, attempt, brief_hash, role, kind, pane_id, machine_label,
machine_id, box_repo, box_worktree, brief_commit, branch, publish_url,
recipient{pane, coordinator_attempt}, start_line, created`. It is written after
the pane id exists; the box validates `HERDR_ADE_LAUNCH`, `HERDR_PANE_ID`, cwd
and the pane process against it. `machine_id` keeps a renamed label from
changing identity.

## What I deleted

- `threads::remote_not_admissible` and `round::admit`'s `remote_not_admissible`
  (a mixed round admits box lanes now).
- The old remote placement path that used Herdr-native worktrees:
  `Herdr::worktree_create`, `Herdr::worktree_open`, `Herdr::worktree_reply`.
- `remote::write_brief` (the D9-breaking brief copy), `remote::report_hashes`
  (report-hash completion), `remote::fetch_dir` (the rsync copy),
  `remote::layout`, `remote::pr_safe`, `remote::repo_info`.
- `thread::copy_home_remote` (the home-copy helper) and `threads::final_copy`'s
  remote branch; `ticker::remote_pass`'s report-hash + copy block.
- The tests that asserted the old refusals
  (`remote_thread_is_not_admissible`, the remote half of
  `ade_start_refuses_remote_and_empty_plain`); the rsync assertions in the
  remote test. New tests cover the provisions and the held machine.

## What the second lane must add (completion side)

- `remote_pass` is now launch + `agent list`/`pane list` only. It needs the
  courier: one multiplexed SSH pass every fourth tick, the box helper that
  reads the box ADE root after the taken cursor (cards, ops, events, artifacts,
  bootstrap receipts, `herdr agent list`/`pane list`, boot id, disk), one
  batched `scp` (`remote::fetch_batch` + `remote::multiplex_options` are
  ready), hash checks, create-only import, the URL-matched lane-commit fetch
  (`remote::remote_for_url` is ready), and the normal D5 delivery.
- `Thread.machine_id` must be the identity the courier reports on; the remote
  `IdentityBinding.socket` currently stays the Mac socket (the launch path
  fills it), which the courier should overwrite with the box identity.
- `ha done`/`ha waiting` on the box must be recognized by the courier: import
  the sealed event into the Mac ledger, deliver the line once, and let the
  Mac's `ops::tick` recover X1/X2/X2b on the box through the helper.
- VM-op recovery on the box, boot-id change to GONE, BLOCKED/GONE typing, and
  the D4 removal gate for box lanes (`threads::final_copy` returns `Partial`
  for a box lane today; `resolve --remove-worktree` still runs the raw SSH
  `git worktree remove`).
- Doctor rows (§2–3, R11): boot service, server status, host, listeners,
  mapping, `gh auth status`, per-kind logins, rules hash, live
  `nproc`/`free`/`df` capacity. `src/pi/`: box provider/model readiness and the
  pane-shell probe replacing the `zsh -lic` rows.
- Board/D18 machine text and the `machine: oci` board field.

## Spec lines not honoured, and why

- §4.1 "The start checks both clones against the configured URL." The Mac side
  pushes by the configured URL; the box fetch is by that same URL, so the box
  clone's `origin` is never asserted. The URL fetch is the check.
- §4.2 step 3 "(profile id, box git-common-dir) repository lock." I hold a
  Mac-side lock keyed by (profile id, box repository path), not the box's
  git-common-dir (the Mac has no safe way to open a lock under the box's
  `.git`). Starts for one box repository still serialize.
- §4.2 step 5 "provider/model readiness and rules-hash checks before the start
  can advance." Belongs to the second lane (`pi/ade.rs`, `doctor.rs`); the Mac's
  `pi_ready` is skipped for box lanes.
- §4.1 "disabled, unreachable or incompatible machines refuse." Only unknown
  profiles refuse. Reachability/compat is the courier/doctor's (second lane).
- §4.2 step 4 "after `workspace list` verifies it." I verify with the box
  `pane list` (herdr has no `workspace list` call in `herdr.rs`).
- §4.3, all of it: the courier, sealed-event ingress, receipts, BLOCKED/GONE,
  D8 amendment. Explicitly the second lane's.
- Box process identity is validated, but leniently: a shell or `herdr-ade`
  foreground counts, because `ha done`/`ha skill` themselves show up in
  `pane process-info`; the courier should tighten it.
- `docs/herdr-notes.md` rows 100/103/108, `docs/operations.md` 128–129 and
  `README.md` 92 still describe the old remote path (remote `worktree open`,
  the brief copy, report/library copy). Not edited: docs are synthesis and some
  rows are historical observations the coordinator owns. They should be
  removed/updated when the split lands.

## Notes from files the brief put out of bounds

- `src/remote.rs`'s `fetch_file`, `fetch_batch` and `multiplex_options` are kept
  for the courier and carry `#[allow(dead_code)]` until it calls them.
- `src/ops.rs` gained `check_published_ref`; the rest of D5 is unchanged.
- `src/herdr.rs` only lost the three unused worktree helpers.
- `src/ticker.rs::remote_pass` was stripped to launch + list so it compiles
  without `report_hashes`/`copy_home_remote`; the courier replaces its middle.
- `Herdr::worktree_remove` stays for the current remote `resolve
  --remove-worktree`.
```

