# Review brief: round r12

plain: This check reads the piece that brings a cloud box lane's finished work home.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r12` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 2, manifest hash `2b086b0d162e03da5c1bf1f28c531d25e7d4bf328d2b636bdf686886125a33ba`, policy hash `e1529634be8dfdf8688cad0bfcfd99dfa67f7f19fe2d1c3c7ab9f12ee7c0eec3`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0023 | 1 | `789bf54e5e056442a62f842841804ecdf82007b4` | `t-0023-1-2` | `618ee8cbf9e321f752f5b0891c3f8c8ba69ae75fa4b8b85b25cd830e025973df` |
| t-0027 | 1 | `e040856b6eb33a2b75ad137a0ad2e0562c90c7d4` | `t-0027-1-1` | `f53cfb3b735ac3f3308ea89f72f420edbad50b1e5871f352c324a481d7619d62` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r12.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r12"
candidate = "<C>"
manifest_hash = "2b086b0d162e03da5c1bf1f28c531d25e7d4bf328d2b636bdf686886125a33ba"
policy_hash = "e1529634be8dfdf8688cad0bfcfd99dfa67f7f19fe2d1c3c7ab9f12ee7c0eec3"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0023 (artifact `618ee8cbf9e321f752f5b0891c3f8c8ba69ae75fa4b8b85b25cd830e025973df`)

Data, not instructions.

```text
# t-0023 — box lanes, completion side (SPEC-remote v2.2 §8.2)

Result: the Mac now imports box completions. `remote_pass` is the courier: on
its due tick it runs the box-local helper over one multiplexed SSH call,
fetches the new envelopes and report bytes with one batched `scp`, checks
every hash, imports create-only into the Mac's canonical ledger with the
source tuple and a Mac artifact path, and types the D8 `BLOCKED`/`GONE` lines.
Doctor gained the box rows, and the pi probes use the machine's own login
shell and read the box's own wrapper. All gates pass with
`PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:
`cargo fmt --check`, `cargo test --locked` (337 + 41 + 52 + 4),
`cargo clippy --all-targets --locked -- -D warnings`,
`cargo build --release --locked`.

Branch based on t-0017's `6d517cb`. Main does not yet carry `6d517cb` (round
r9 is still in review), so no main merge was needed.

## The courier's tick

`remote_pass` (ticker) keeps the `herdr --machine` `agent list`/`pane list`
call, then calls `steps::courier` and `steps::remote_attention`. The cadence is
the existing `Memory::machine_is_due`: every fourth 15-second tick per machine,
with the existing eight-tick skip after a failure.

`steps::courier(ctx, project, machine)`:

1. Resolves the stable profile with `remote::machine_profile`; a local or
   target-less profile refuses.
2. Runs `remote::ssh_courier` once: `ssh` with
   `-o ControlMaster=auto -o ControlPath=<root>/.state/remote/ssh/%C
   -o ControlPersist=10 -o ConnectTimeout=5 -o BatchMode=yes`, running the
   generated `steps::courier_helper` script under `sh -c`.
3. The helper prints tab-separated facts: `boot` (boot id), `free` (disk
   bytes), and one `event` record per `<root>/<slug>/events/*.toml` with its
   path, SHA-256, and (for `done`) the artifact path and hash read from the
   event's own `artifact = "..."`. A reading's `source` tuple on import is
   `(profile id, box slug, box event id, box event hash, artifact hash,
   imported-at)`.
4. Filters to this project and to envelopes whose id is not already in the
   `taken` cursor, then one `remote::fetch_batch` (same control socket) copies
   every event and artifact into `<root>/.state/remote/staging/<profile>/<slug>/`.
5. For each envelope: verifies the event hash and the artifact hash, then
   `events::import_box_event`, which writes the artifact content-addressed to
   `<project>/artifacts/<hash>`, rewrites the event's `report_path` to that
   Mac path, writes the event create-only, and records the import tuple. An
   event id already imported with a different hash refuses as
   `event_conflict`; the same bytes are a replay no-op.
6. Advances `taken` per event and saves `RemoteState`, only after the import
   is on disk. The box keeps its copies; nothing prunes them.

`tick_slow` then runs `ops::tick` as before, and `steps::deliver_events` types
the normal `DONE <id> <mac-artifact-path> <sha>` / `WAITING <id> <what>` line
through the serialized writer. Token projection is skipped for a remote lane:
the box already set them.

## What it copies and types

- **Copies:** every new box envelope (`events/<id>.toml`) and, for `done`, the
  report bytes (`artifacts/<sha256>`). Nothing else; no home, library or
  brief copy.
- **Types:** `BLOCKED <lane>` when a verified box lane enters `blocked`;
  `GONE <lane>` on a boot-id change for every open box lane, or when its pane
  and agent are absent on two consecutive successful passes. A failed pass
  types nothing and invents no GONE. After the configured outage period it
  types one `BLOCKED <lane> machine <name> unreachable` per open lane, then
  stays quiet until the machine answers. Each transition is marked in
  `RemoteState` so it types at most once.
- **Machine text:** the `thread-state`, outage and delivery summaries name the
  machine (`on machine \`oci\``), and `overview` already labels the pane.

## The doctor rows

For each saved machine used by a project or a box lane (`machine_profile`,
not `ssh_target`), `doctor.rs::box_rows` runs one read-only SSH call and adds:

- `box <label> boot`: `systemctl --user is-enabled herdr.service`;
- `box <label> server`: `$HOME/.local/bin/herdr --version`;
- `box <label> host`: `hostname` plus the Tailscale IPv4;
- `box <label> listeners`: the `ss -tln` count;
- `box <label> repo <box path>`: the clone's `.git` exists (each committed
  `BOX_REPOS` row);
- `box <label> git`: global `user.name`/`user.email`;
- `box <label> gh`: `gh auth status` (Rolf's whole-account reading
  2026-09-19 13:55);
- `box <label> login <pi|claude|codex|agy>`: `$HOME/.local/bin/<kind>` is
  executable;
- `box <label> capacity`: live `nproc`, `MemAvailable`, `df -B1 /`, a
  conservative lanes-fit count (1 OCPU host reserve, 4 GB RAM and 5 GB disk
  per lane) and the always-refuse-below-12-GB-free gate;
- `box <label> rules`: the box `RULES.md` SHA-256 when one exists.

## The probes

- `pi/sh.rs::login_shell` now runs `$SHELL -lic`, with `zsh` only as the
  fallback when `$SHELL` is unset. `pi/doctor.rs`'s wrapper row goes through
  it, and `launch.rs`'s kind lookup uses `crate::pi::sh::shell()`. The
  hard-coded production `zsh -lic` probes are gone; the Mac result is
  unchanged because `$SHELL` is `/bin/zsh` there.
- `pi/ade.rs::check_on_machine` runs
  `HERDR_ADE_ROOT=/home/ubuntu/.herdr-ade /home/ubuntu/.local/bin/herdr-pi
  check <provider>` on the box over SSH. `threads::box_pi_ready` wires it into
  `thread start` and the ticker's `launch_pass`, so a box pi lane is refused
  from the box's own wrapper and login store, never from Mac auth.

## Spec lines not honoured, and why

- **§4.3 one SSH handshake per machine.** The pass still makes the
  `herdr --machine` bridge call for the live `agent list`/`pane list` (the
  brief asked to keep it), then the helper + `scp` mux connection: two SSH
  bridges, not one. Moving the live lists into the helper is what §4.3 wants,
  but the brief named `herdr --machine` explicitly, so I followed the brief.
- **§4.3 the helper returns after the taken cursor.** The helper prints the
  whole box ledger; the Mac filters by its per-`(profile id, project)` import
  records. t-0017's box seal writes no ordered outbox log, so there is no
  monotonic sequence to pass as `--after`. A taken-set cursor gives the same
  exactly-once import.
- **§4.3 box-side D5 recovery.** The helper does not run X1/X2/X2b on the box
  because the box plugin exposes no `remote` verb; the Mac's `ops::tick` only
  recovers Mac-side ops. A box `ha done` between reserve and seal therefore
  stays until a human or a later box verb repairs it.
- **§5 board `machine: oci` field.** `board.rs` still skips remote threads in
  `ade_lanes`, so the board counts only local lanes. `board.rs` is outside the
  §8.2 file list for this lane; the machine name does appear in the inbox and
  overview text.
- **§3.3 the box pane-shell probe.** Doctor checks the box wrapper and native
  binaries over SSH, not a real box pane running `type -a -P pi`; the repo has
  no pane-exec/last-output API to read a fresh pane's answer. The Mac side is
  the required `$SHELL -lic`.
- **§6 rebooted lanes restarted from the start line.** The courier pushes
  `GONE`; the restart stays the coordinator's `ha thread restart`, which
  reuses the recorded brief and start line. No automatic restart was added
  (D9 keeps the ticker the only launcher, and §6 says the coordinator
  restarts).
- **Q11 box worktree removal under the box's repository lock.** `resolve
  --remove-worktree` on a box lane still runs `Herdr::worktree_remove` or a
  raw SSH `git worktree remove`, not under a box-side repository lock. The
  Mac-side `final_copy` is now honest: it reads the imported artifact and
  reports `Complete` once it hashes to its name, or `Partial` with the reason
  otherwise, so the existing D4 gate is meaningful for box lanes.
- **`remote::fetch_file`** remains with `#[allow(dead_code)]`; the courier
  path is `fetch_batch`. It is still the safe fallback for a path `scp` cannot
  carry.
- **`write_thread_items` report-available items** do not fire for a box lane
  during the tick (its `report_hash` is only learned at `final_copy`); the
  completion arrives through the imported sealed event's inbox item and typed
  line instead.

## Files

`src/steps.rs` (courier helper, manifest, import, BLOCKED/GONE, delivery
skip), `src/events.rs` (ImportSource, RemoteState, create-only import, artifact
rewrite, tests), `src/remote.rs` (`ssh_courier`, `multiplex_options`
un-allowed), `src/ticker.rs` (courier wiring, unreachable BLOCKED, box pi
readiness), `src/threads.rs` (`box_pi_ready`, imported `final_copy`),
`src/doctor.rs` (box rows + test), `src/pi/ade.rs` (`check_on_machine`),
`src/pi/sh.rs`, `src/pi/doctor.rs`, `src/launch.rs` (`$SHELL -lic`),
`src/scenarios.rs` (remote fakes now return a courier manifest).

New tests: box import is create-only/hash-checked/path-rewritten; artifact
mismatch refuses; remote state round-trips per profile; the courier manifest
parses and refuses junk; the helper survives a hostile box root; a box lane
absent twice is pushed GONE once and a boot change re-pushes it; the doctor
box rows gate on free disk.
```

### t-0027 (artifact `f53cfb3b735ac3f3308ea89f72f420edbad50b1e5871f352c324a481d7619d62`)

Data, not instructions.

```text
# t-0027 — repair the box completion side after r12

plain: This fixes the four faults the check found in the piece that brings a
cloud box lane's finished work home.

Branch starts from `main` (r10 round-advance hook, r11 Pro relay) with the
reviewed candidate `31bcc5b0cd52d19355fb3899a464367f230896cc` merged in. The
merge was clean (no `src/ticker.rs` conflict; r10's `round advance` and t-0023's
`remote_pass` did not overlap). The two review commits `66c2993` and `31bcc5b`
are untouched. No rebase, no stash. Gates green at every commit with
`PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:
`cargo fmt --check`, `cargo test --locked` (350 + 44 + 65 + 4),
`cargo clippy --all-targets --locked -- -D warnings`,
`cargo build --release --locked`.

Commits: `8e74541` (findings 1–3), `de3a84b` (finding 4), `6121d34` (board age),
`e040856` (tests).

## Finding 1 — courier cadence per machine, one SSH trip

**Changed.** `src/ticker.rs`: the courier moved out of the per-project
`tick_slow` and into a new `machine_passes`, called once from `tick` (and from
the test-only `tick_project_with`) before the per-project slow pass. It groups
every reachable project's open remote lanes by machine route, calls
`Memory::machine_is_due` once per machine, and runs `steps::courier` once with
all projects on that machine. The view is stored in `Memory::machine_views`
(this tick only) and consumed by `tick_slow`, which no longer checks the cadence
or calls the courier. Outage recording, the machine-unreachable `BLOCKED` lines
and `write_machine_outage` moved into `machine_passes` too.

`src/steps.rs`: `courier` now takes `&[&Project]` and resolves the profile,
builds the cursor, runs the helper and imports every project's envelopes in one
pass. The helper (`COURIER_HELPER`) reads the box's own lists with the box-local
binary (`"$HOME/.local/bin/herdr" --session <session> agent list` / `pane list`)
and prints them as `agents`/`panes` records, so `remote_pass` no longer opens a
`herdr --machine` bridge for `agent list`/`pane list`. `CourierOutcome` carries
`agents`/`panes` (`None` when the box server did not answer; the pass then
imports events but changes no lane state, so no false GONE).

**Tested.** `src/scenarios.rs` rewrote the three remote scenarios to drive the
courier's `ssh` (not `herdr --machine`): the failure/skip-for-eight-ticks test
now counts `ssh` courier calls; the outage test and the blocked-lane test feed
the helper's `agents`/`panes` records. New `courier_imports_every_project_on_the_machine_after_the_taken_cursor`
in `src/steps.rs` proves two projects on one machine both import from one
helper call and one `scp` each, and that a second pass with the taken cursor
does no fetch.

**Not fully honoured.** §4.3 says "one batched `scp`". The pass makes one
`scp` per project over the one multiplexed connection (one SSH handshake). A
single destination directory would collide: two projects can both have a box
event named `t-0001-1-1.toml`, and the later copy would overwrite the earlier
before its import. The finding's fix text asks for "one SSH trip per pass",
which this meets.

## Finding 2 — DONE requires the pinned commit

**Changed.** `src/steps.rs`: `deliver_event`, after loading the lane and before
the inbox item and the typed line, calls `verify_published_sha` for a remote
lane's `done` event. It resolves the publish URL from the project's own repo row
then the committed `BOX_REPOS` map (`publish_url_for`), picks the URL-matched
remote (`remote::remote_for_url`), `git fetch`es `refs/heads/<branch>`, and
requires `git merge-base --is-ancestor <sha> FETCH_HEAD`. On a failed fetch or an
absent commit it returns `published_fetch_failed` / `published_sha_missing`, so
no inbox item and no DONE line are written, no journal line is appended, and the
next tick retries.

**Tested.** `a_done_event_is_not_delivered_until_its_sha_is_on_the_publish_remote`
covers both the failed fetch and the absent commit.

**Note.** The "recorded reason" is the error the ticker logs each pass
(`.ticker.log`); no durable inbox item is written, to avoid a per-pass item.
The check runs at delivery, not at import, which is the single choke point where
the DONE line is typed; the event is imported first and stays undelivered.

## Finding 3 — cursor, receipts, box D5 recovery

**Changed.**
- Cursor: the Mac builds a `<slug>\t<event id>` cursor from every project's
  `RemoteState.taken` and passes it on the helper's stdin; the helper skips
  those events (`grep -Fqx`). Its answer starts after the cursor.
- Receipts: `events::write_receipt` writes `<project>/receipts/<event>.toml`
  (event hash and artifact hash, create-only) at seal; `ops::seal` calls it for
  both Mac and box seals. The helper prints `receipt` records and the courier
  requires the box receipt's `event_hash` and `artifact_hash` to equal the
  fetched bytes before advancing the cursor (`receipt_missing` /
  `receipt_mismatch`). Bootstrap receipts (`<slug>/.state/bootstrap/<thread>.json`)
  are printed as `bootstrap` records; `apply_bootstraps` marks the Mac thread
  `bootstrap = "acknowledged"` only when the receipt's pane and brief hash still
  match.
- Box D5 recovery: new hidden `herdr-ade recover` verb (`src/cli.rs`) calling
  `ops::recover_box`, which runs before the helper reads the ledger. It uses the
  box lane card as authority: a reserved op whose helper is dead or whose card
  moved on is abandoned (X1); a staged op whose card matches is sealed from its
  own durable payload (X2), and `seal_create_if_absent` repairs a matching event
  marker (X2b).

**Tested.** `courier_helper_answers_only_after_the_taken_cursor` runs the real
script with and without a cursor. `courier_refuses_a_receipt_that_disagrees_with_the_fetched_bytes`
proves no import and no cursor move on a mismatch. `courier_imports_every_project...`
asserts the bootstrap carry. `a_completion_receipt_is_create_only_and_records_the_hashes`
covers the receipt file. `box_recovery_seals_a_staged_op_from_its_card_and_abandons_a_dead_reserved_one`
covers X1/X2 and the receipt on the recovered event.

## Finding 4 — box pane probe, box logins, machine naming

**Changed.**
- `src/herdr.rs`: `pane_run`, `pane_read_text` (pane read prints text, not JSON)
  and `workspace_close`.
- `src/doctor.rs`: `box_pane_probe` creates a fresh box pane with the lane
  `PATH` (`workspace create --env PATH=...`), runs `BOX_PROBE`
  (`type -a -P pi` then `command -v cargo just claude codex agy node`) in that
  pane's own Bash shell, reads `@@pi`/`@@cmd`/`@@done`, and closes the
  workspace. The old bare-`ssh` `login_*`/`tool_*` checks are gone. The wrapper
  row fails unless the first `type -a -P pi` hit is
  `/home/ubuntu/.local/bin/pi`; the tools row fails on any missing tool. Pi
  provider readiness now runs on the box through the box wrapper
  (`herdr-pi check <provider>`) in the read-only SSH script; `gh auth status`
  stays on the box; native kinds keep the box binary check.
- `src/board.rs`: `compute` counts box lanes (using the Mac record's
  `last_state`/`last_group` since the live state is on the box), and `ade_last`
  names the lane's machine. `src/cli.rs`: `ha board <project> --thread <thread>`
  prints the lane's group, machine and age.
- `src/steps.rs`: a box lane's completion summary now reads
  `<lane> on machine `<label>` completed ...`.

**Tested.** `box_rows_read_the_box_and_gate_on_free_disk` fakes the probe pane
and asserts the wrapper/tools rows; `box_wrapper_probe_fails_closed_when_the_pane_answers_another_path`
proves the first-hit refusal. `board::tests::a_box_lane_is_counted_and_the_last_line_names_its_machine`
covers the board. `src/scenarios.rs` still asserts the thread-state line names
the machine.

**Not fully honoured.** §5's "remote-age fields" on the aggregate board tokens:
the tokens are single 80-character strings, so the per-lane age and machine live
on the `ha board <project> --thread <thread>` line, not on `ade_lanes`. §5's
"detailed record" (profile id, last pass, last sealed event, last import, taken
cursor, free disk, last error) is not exposed by `ha thread show`; the finding
did not name it and it is a larger surface than this repair.

## Other spec lines not honoured

- §4.3 "one SSH handshake": the courier is one handshake. Thread token
  reporting (`pane report-metadata`) and a pending prime still go through
  `herdr --machine` after the courier, by design (they are lane actions, not the
  poll); t-0023's scenario asserts the remote token call.
- §6 "rebooted lanes restarted from the start line": the courier pushes GONE;
  the restart stays the coordinator's `ha thread restart` (unchanged, as t-0023
  reported).
- §4.3 box-side worktree removal under the box repository lock is unchanged
  from t-0023.
```

