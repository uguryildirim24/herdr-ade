# Code review + fix — plugin round r1 (herdr-ade, six lanes) on branch `review/plugin-r1`

You are the one reviewer of the first build round of `herdr-ade`, Rolf's coordination
plugin for his herdr fork (SPEC-ADE v3.1). Six lanes built it as packages
A0 `ade-contracts` (`lane/ade-contracts` 20fd9e9c), A1 `ade-core` (`lane/ade-core`
1a947922), A2 `ade-outbox` (`lane/ade-outbox` 2d8ff3a1), A3 `ade-rounds`
(`lane/ade-rounds` 99e1c0e9), A4 `ade-picker` (`lane/ade-picker` 0d3e706e) and A5 `ade-pi`
(`lane/ade-pi` d6f71088). A1 to A5 all branch from A0's pin 20fd9e9c. The surface is about
26,000 added lines, so this round has two reviewers: **you own the candidate, every seam, the
wiring, the gates, the acceptance and the verdict**; a second reviewer (`reviewer-b`, brief
`tasks/review-plugin-r1-b.md`) inspects and fixes A4 and A5 on their own lane branches in
parallel and hands them to you through the coordinator. You merge A0 to A3 now and A4, A5
when the coordinator prompts you `MERGE A4 <sha> A5 <sha>`. Nothing reaches `main` before
your verdict.

## Setup

- Repo `/Users/rolfie/projects/herdr-ade` (fork of eliasstravik/herdr-projects; `origin` =
  uguryildirim24/herdr-ade). Worktree `/Users/rolfie/projects/herdr-ade/.worktrees/review`,
  branch `review/plugin-r1` from `main` 52194f2 (base a4cdb0a plus the six briefs).
- Build: `export CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/review` and
  `export DEVELOPER_DIR=/Library/Developer/CommandLineTools`; Rust 1.89 (`cargo +1.89.0`),
  edition 2024. GNU sed is first in PATH: edit with python or your editor, never `sed -i ''`.
- The herdr binary that is C for §4.3 (SPEC-ADE §6 item 72): the r2 candidate
  `/Users/rolfie/projects/herdr/.target/review/release/herdr` (prints `herdr 0.9.1`). Reach it
  by absolute path (`HERDR_BIN_PATH`, as A3's doctor run did); never put it first on your
  PATH, because your closing pushes below must reach Rolf's live server through
  `~/.local/bin/herdr` (0.9.0, session `default`).
- Throwaway everything: `HERDR_ADE_ROOT=/var/tmp/ade-review/root`,
  `XDG_CONFIG_HOME=/var/tmp/ade-review/xdg`, `XDG_STATE_HOME=/var/tmp/ade-review/state`; a
  throwaway herdr session is started with the candidate binary,
  `env -u CLAUDE_CODE_CHILD_SESSION`, the isolated XDG dirs, and stopped with that same
  binary's `session stop <name>`. pi runs only inside a throwaway prefix under
  `/var/tmp/ade-review/` the way A5's gate 5 shows.
- Never push. Never merge into `main`. Never `git stash`. Never touch the root checkout or
  another worktree (the lane worktrees `a0` to `a5` stay as they are). Never `herdr plugin
  link`, `herdr plugin install`, `cargo install`. Never write under `~/.config`,
  `~/.herdr-ade`, `~/.herdr-projects`, `~/.pi`, `~/.local/bin`, `~/.claude`, `~/.codex`,
  `~/.cursor`. Never type a `/login` anywhere. Rolf's live server (four builds in it):
  never `herdr server stop`, `herdr server restart`, `herdr update`. Real `claude` panes in
  your throwaway session are fine (they run as Rolf, nothing is copied), and **every one of
  them runs Haiku**: `-- --model claude-haiku-4-5-20251001 --dangerously-skip-permissions`,
  and the throwaway project's roles table pins that model for every Claude role; never Opus
  or Sonnet in a throwaway pane (Rolf, 19:15). Hooks go only into the throwaway project's own
  `.claude/settings.json`, never Rolf's home. No Cursor pane
  (quota gone, Cursor is retired after the port) and no Codex pane for acceptance rows:
  those rows are NOT-RUN with that reason.
- Start line (for a restart after GONE; Rolf names the helper, the coordinator keeps this
  line current):
  `herdr agent start reviewer --kind claude --pane <the review tab's pane> --parent w1F:p1 -- --model claude-opus-5 --effort high --dangerously-skip-permissions`

## Steps

1. Read `/Users/rolfie/projects/herdr/tasks/SPEC-ADE.md` v3.1 whole, with §6 items 36 to 76
   (the lanes' questions with the coordinator's readings: several assign you work, listed in
   step 5), `tasks/jev/SPEC-jev-picker.md` and `tasks/pi/SPEC-pi.md` in that same repo, the
   six briefs `tasks/ade-{contracts,core,outbox,rounds,picker,pi}.md` here, and the six
   reports pasted below. Base line references with the `hp:` prefix mean this repository at
   a4cdb0a.
2. Merge in this order, resolving each seam yourself, one merge commit each:
   `git merge lane/ade-contracts`, then `lane/ade-core`, `lane/ade-outbox`, `lane/ade-rounds`
   now; `lane/ade-picker` and `lane/ade-pi` only after the coordinator's prompt
   `MERGE A4 <sha> A5 <sha>` (reviewer-b's fixes land on those branches first, so their heads
   move past 0d3e706e and d6f71088; merge the shas the prompt names). Do everything for A0 to
   A3 first (steps 3 to 6 as far as they reach without A4 and A5); if you get there before the
   prompt, push `WAITING review-plugin-r1 A4 and A5 from reviewer-b` with the closing lines
   below and stop; after the prompt, merge, wire A4 and A5, and rerun every gate and the
   acceptance on the full candidate. Known seams: the marked `ade-<pkg> begin/end` blocks in
   `src/main.rs` and `src/cli.rs` (keep every block; A5's is five lines with a
   `#[path = "pi/ade.rs"]` module); the additive `contracts(<pkg>)` commits on
   `src/contracts.rs` (adjacent-line conflicts, keep every field, one `RoundRecord`, one
   `Launch`; A4 wants `ThreadRecord.launch: Launch` in place of `LaunchRecipe`); `Cargo.toml`
   (A1 dropped the `too_many_arguments` allow, A5 added the `[[bin]] herdr-pi` entry);
   `skill/THREAD.md` versus `skill/LANE.md` (item 59: the include moves to `LANE.md` and
   `THREAD.md` goes, in one commit); `scripts/acceptance` (A0 skeleton, A3 assembled it; A5
   gives §7 pi rows).
3. Wire what the lanes left for you, each report's "Left for the reviewer" has the exact
   lines: A1's ticker takes A2's `crate::ops::tick(ctx, project)` in the slow pass outside
   the project lock (item 57), A3's `crate::round::tick(ctx, &project)` and A4's tick entry;
   A1's `thread start` calls A4's `resolve_launch` before the worktree and tab exist, not
   holding the project lock, and fills `brief_hash` after the brief commit (item 48), with
   `--role` and `--recipe` on the verb; A1's doctor prints A4's `doctor_rows` (then delete
   A4's hidden `picker-doctor` verb, item 50) and A5's pi rows (`crate::pi_ade::doctor_rows_with`
   for scripted tests); `crate::pi::check(provider)` plus the process-prefix check before a
   `kind = "pi"` launch; A2's adapter table takes A5's priming row; A4's picker table takes
   A5's `pi_recipes()` and the withheld list; A1's `Herdr::call` treats silent success as
   success (item 71, A3 line 2); A2's coordinator open, outbox writer and correction hook take
   A3's `talk`/`ask` calls (A3 lines 7 to 11); A3's `dialogue start` takes A4's picker as the
   pair filter; the three pi actions go into `herdr-plugin.toml` (A5's TOML); A1's
   `.git/info/exclude` gets `.worktrees/` on `thread start` (A3 line 6); `.state/capabilities/`
   qualified markers only after a real authenticated row (A2 line 7).
4. Apply the coordinator's readings that name you (SPEC-ADE §6): 36 format the whole crate
   once on the candidate in its own `review(fmt):` commit so `cargo fmt --check` exits 0; 37
   clippy allows dropped where the code is fixed; 38, 39 doctor D1 tuple and `XDG_CONFIG_HOME`
   (A1 says done, verify); 47 an inline role without `plain` gets a doctor warning, not a
   refusal; 55 new-lane briefs commit on the integration branch through
   `commit_file_on_branch` (D9), merges keep the checkout-dirty path; 61 the pi probe is
   `whence -va pi`; 68, 69 add `i`, `merged`, `reviewed`, `unfinished`, `skipped` to
   `plain/vocabulary.txt` and drop A3's fixed-text bypass; 73 the checker strips a trailing
   `'s` before the lookup, with a test; 76 `round merge` never fast-forwards a worktree whose
   branch has commits past its admitted pin (add the guard if small, else say so). Item 58
   (a real Claude login for the hook row) and item 65 (the Muse model id: my reading is the
   OpenCode Go id `muse-spark-1.3-contributor`) are Rolf's; take the reading unless the
   coordinator tells you his answer, and say which you took.
5. Gates, from the worktree with the env above, all green before the verdict:
   ```
   cargo +1.89.0 fmt --check
   cargo +1.89.0 clippy --all-targets --locked -- -D warnings
   cargo +1.89.0 test --locked
   cargo +1.89.0 build --release --locked
   HERDR_ADE_ROOT=/var/tmp/ade-review/root XDG_CONFIG_HOME=/var/tmp/ade-review/xdg XDG_STATE_HOME=/var/tmp/ade-review/state HERDR_BIN_PATH=/Users/rolfie/projects/herdr/.target/review/release/herdr $CARGO_TARGET_DIR/release/herdr-ade doctor
   ACC_DIR=/var/tmp/ade-review/acc sh scripts/acceptance/run
   ACC_LIVE=1 ACC_DIR=/var/tmp/ade-review/acc sh scripts/acceptance/run
   ```
   Doctor: everything ok except the four pi provider logins (fail-closed, item 63) and
   `gh auth`. Acceptance, deterministic mode: every `det`, `cli` and `mod` part PASS, with
   the `mod` test-name patterns adjusted to the real names on the candidate (A3 line 14).
   Live mode against your throwaway session with real `claude` panes: every row you can run,
   runs; the pi rows T4 to T14 and any row that needs a Cursor or Codex pane or a `/login`
   are NOT-RUN with the reason in the verdict, never marked passed. `$CARGO_TARGET_DIR/release/herdr-pi doctor`
   and `check` on the throwaway prefix as A5's gate 5. After every acceptance run, stop your
   throwaway session with the candidate binary; never the real server.
6. Adversarial review, per package, and fix what you find in separate `review(<pkg>):`
   commits. Attack hardest: **A0** serde defaults and round-trips on every record; the plain
   checker R1 to R7 on 25-word, hyphen, number, possessive and empty inputs. **A1** the
   project and repository locks (path `<git-common-dir>/herdr-ade.lock`), `HERDR_ADE_LAUNCH`
   against process identity, `worktree add` never `--force`, restart increments `attempt`
   before `tab create` (item 54), every swap-binary refusal, the 0.9.0 special case (item 56)
   against the 0.9.1 candidate. **A2** ops durability: fixed event id, once-by-id delivery,
   sealed events never rewritten, receipts binding a replacement coordinator, hooks enforced
   without trusting the skill text, no native prose forwarded, no screen reads. **A3**
   brief-before-worktree order and the launch record, admission manifest revision, checkpoint
   composed without stopping a landed merge, talk journal request states and replay, ask
   binding frozen at a revision, `Herdr::call` on silent verbs, the ff guard. **A4**
   `resolver = "off"` default, shadow mode never launches, the daily cap file and counter,
   unknown recipe fields refused, no live Jev call in any test, a model named in the task
   warns, `ade_last` from the stored sentence. **A5** trust never, no `--approve`, the guard
   only ever `ha waiting`, wrapper first on PATH and prefix isolation, no Cursor route, the r2
   strip tokens, `whence -va`. Contract conformance everywhere: names, error codes
   (`plain_missing`, `role_args_missing`, `gate_duplicate`, `recipe_kind_unknown`,
   `pi_cursor_forbidden`, `credentials_not_configured`), exit codes, TOML shapes; nothing
   sensitive written to disk; no `unwrap()` in production code; dependency pins.
7. Write `tasks/reviews/code-plugin-r1.md`: a 3-line verdict (MERGE /
   MERGE-AFTER-DECISION / REJECT), the gate table with counts, the acceptance table (every
   §4.3 row: PASS, FAIL or NOT-RUN with the reason), the defects table (severity, file:line,
   what was wrong, what you changed, commit), the wiring you did (one line per item of step
   3), which reading you took on items 58 and 65, the install-day list for Rolf (what he
   types once: `herdr-pi setup`, the four logins, the swap script, the hook install, in
   order), and "Needs a decision" numbered from 77 (SPEC-ADE §6 continues there; answer each
   with your reading). Commit it on `review/plugin-r1` as `docs(review): plugin round r1 verdict`.

## Report and closing steps (verbatim, every turn ends with one of these, also on failure)

Write `/Users/rolfie/projects/herdr-ade/.worktrees/review/.reports/review-plugin-r1-report.md`
(git-ignored, not committed): what you ran, what you did not run and why, the final commit
sha. Then:

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=review-plugin-r1 --token done=1
herdr notification show "plugin review done" --body "reviewer" --sound done
herdr agent prompt hcoord "DONE review-plugin-r1 .reports/review-plugin-r1-report.md <final commit sha>" || herdr agent prompt hcoord "DONE review-plugin-r1 .reports/review-plugin-r1-report.md <final commit sha>"
```

If you must stop for something outside the lane:

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=review-plugin-r1 --token waiting="<what>"
herdr agent prompt hcoord "WAITING review-plugin-r1 <what>"
```

A turn that ends without one of these pushes is the one failure nothing catches.

## The lanes' reports, verbatim

### ade-contracts (lane a0, `lane/ade-contracts` 20fd9e9c)

````markdown
# Lane a0 report: ade-contracts

Final commit sha: `20fd9e9c615c4d92ba3d00d5173b49ab7a6fe62b`
Branch: `lane/ade-contracts`
Worktree: `/Users/rolfie/projects/herdr-ade/.worktrees/a0`
Toolchain used for gates: `rustc 1.89.0` (`cargo +1.89.0`). Default PATH cargo is 1.97.1.

## What I read

- `tasks/ade-contracts.md` (this brief)
- SPEC-ADE.md v3.1: §1.2 D1, D2, D5, D6, D14, D17, D18; §1.3; §4.2 A0; §4.3; §6 items 29 and 32–35
- `tasks/ade/turns/04-pro.md` adversarial fixtures (lines 52–60)
- Base plugin at a4cdb0a: `src/actions.rs`, `src/paths.rs`, `src/herdr.rs`, `src/runner.rs`, `src/scenarios.rs`, `src/thread.rs`, `src/cli.rs`, `herdr-plugin.toml`, `Cargo.toml`

## Commits

1. `5b63284d38ea2c026ea1d4a2ba6c737815e28787` chore(ade): rename herdr-projects to herdr-ade
2. `8c7e692f8a8f66b9271c9e8b5c59693884af59a6` feat(contracts): shared types (SPEC-ADE D2, D5, D6, D18, items 32-35)
3. `5faaa8d4e5f45a22125561cab4f4e2880782f14c` feat(plain): pure checker R1-R7 with word list and fixtures (D17)
4. `e4d709eafc8d207d422174564c204e9cfe13e711` test(scenarios): FakeRunner wiring for the new verbs (§1.3)
5. `20fd9e9c615c4d92ba3d00d5173b49ab7a6fe62b` chore(acceptance): required-row skeleton (§4.3)

(The brief commit `579fe257dac8c844dba2d479e16ead18aa3bb7fc` was already on the branch.)

## Renamed literals

Plugin id, crate, binary, root, config, env:

- `Cargo.toml` package `herdr-projects` → `herdr-ade`
- `Cargo.lock` package name
- `herdr-plugin.toml`: `id = "herdr-ade"`, `name = "ADE"`, every `target/release/herdr-ade` command; `min_herdr_version = "0.9.1"` kept
- `src/actions.rs`: `PLUGIN_ID`, log hint `--plugin herdr-ade`, notification title `herdr-ade doctor`
- `src/cli.rs`: command name `herdr-ade`; help text `$HERDR_ADE_ROOT` and `~/.herdr-ade`
- `src/paths.rs`: config `~/.config/herdr-ade`; env `HERDR_ADE_ROOT`; default root `~/.herdr-ade`
- `src/main.rs`: error prefix `herdr-ade:`
- `src/herdr.rs`: `SOURCE = "herdr-ade"`; socket request id `herdr-ade`
- `src/overview.rs`: view source `herdr-ade`
- `src/steps.rs`: nudge prefix `[herdr-ade ticker:…]`; `HERDR_ADE_OUTAGE_SECS`; notification `herdr-ade: {slug}`
- `src/routine.rs` / `src/pr.rs`: ticker-shaped hostile fixtures use `herdr-ade`
- `skill/COORDINATOR.md`: ticker prefix; `~/.config/herdr-ade/`
- `scripts/dev-hp`, `dev-herdr`, `dev-server`: `HERDR_ADE_ROOT`; binary `herdr-ade`
- `README.md`: install id, binary commands, plugin log id, `~/.herdr-ade`, `~/.config/herdr-ade`
- `tests/cli.rs`: `CARGO_BIN_EXE_herdr-ade`; default root `~/.herdr-ade` (needed so the binary rename compiles; not listed in owned files)

Left as project-slug examples, not plugin id: `src/project.rs` humanize tests and `src/scenarios.rs` workspace label `herdr-projects`.

`docs/` was not edited.

## Types (`src/contracts.rs`) and spec lines

All serde JSON round-trip tested; TOML too except jsonl-only journal lines.

| Type | Spec |
|---|---|
| `LaunchRecipe` | D2 lines 298–299 |
| `ProcessIdentity` | D3 lines 323–324 |
| `IdentityBinding` | D3 lines 322–324 (`terminal_id` omitted) |
| `ThreadRecord` (`role`, `launch`, `attempt`, `partial`, `bootstrap`, `plain`, `identity`) | D2 298–309; D3 322–330; D4 357; D14 924–926; D17 item 6 731–737; A0 1257–1258 |
| `RoleSpec` (deny unknown fields), `RolesTable`, `DAY_ONE_ROLES` | D2 290–309 |
| `Op`, `OpKind`, `OpState`, `Requested`, `Recipient` | D5 376–400; item 32 1531–1538 |
| `Event`, `EventPayload`, `DonePayload`, `WaitingPayload` | D5 395–400 |
| `DeliveryLine`, `DeliveryState` (`submitted`, `acknowledged`, `handled`) | D5 377, 406–411 |
| `Ask` | D17 item 4 697–703 |
| `HumanMessage` (`Say`, `Ask`, `Notice`) | D17 item 3 680–682; item 35 1552–1558 |
| `RoundRecord`, `AdmissionManifest`, `ManifestMember`, `CompletionPin` | D6 447–469; item 33 1539–1544 |
| `MergeIntent`, `MergePhase` | D6 482–500; item 34 1545–1551 |
| `CheckpointIntent` | D6 488–490; item 34 1545–1551 |
| `TalkJournalRecord`, `TalkInbound`, `TalkRequestState` (`queued`, `submitted`, `uncertain`, `accepted`) | D18 item 2 756–762; item 35 1552–1558 |

## Word list

- File: `plain/words.txt` (5000 lines), `plain/vocabulary.txt` (102 plugin nouns)
- Source: SCOWL 2020.12.07 (`rel-2020.12.07`, commit `5ef55f9c4273`), <https://github.com/en-wl/wordlist>, <http://wordlist.aspell.net/>
- Cut: `final/english-words.10` (most common band) plus enough of `final/english-words.20` to reach 5000 ASCII alphabetic words
- Licence: SCOWL collective-work grant (Copyright 2000–2018 Kevin Atkinson) permits copy, modify, distribute and sell; size 10 and 20 come from public-domain Moby Words II and Brian Kelk frequency classes. Notice recorded in `plain/README.md`
- Refused: first20hours/google-10000-english (GitHub licence `NOASSERTION`)

Checker: `src/plain.rs` `check` / `check_ask` / `check_message`. R1–R7 with exact fix texts. Fixtures: one pass and one fail per rule, plus D17 adversarial cases. No hooks, no file writes, no glossary persistence.

## FakeRunner (§1.3)

`FakeRunner::on_ade_new_verbs` and `ADE_NEW_VERB_SCENARIOS` in `src/runner.rs`. Test `ade_new_verb_scenarios_have_canned_herdr_replies` in `src/scenarios.rs`. Canned replies only. No verb behaviour.

## Acceptance (§4.3)

`scripts/acceptance/run` and `scripts/acceptance/rows.md`. Rows 1–11 present, all NOT-RUN. Report format `STATUS<TAB>id<TAB>title` then `REQUIRED`. Exit 1 because required rows are not PASS.

## Gates

Toolchain: `cargo +1.89.0`, `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a0`, `DEVELOPER_DIR=/Library/Developer/CommandLineTools`.

### `cargo fmt --check`

Exit 1. Same pre-existing wrapping on unowned files as main at `579fe25` (for example `src/ticker.rs`). A0 did not reformat unowned modules. Owned files were formatted when edited.

Last lines:

```
-        let log = Log { path: dir.path().join("log") };
+        let log = Log {
+            path: dir.path().join("log"),
+        };
```

### `cargo clippy --all-targets --locked -- -D warnings`

Exit 0.

```
   Compiling herdr-ade v0.1.0 (/Users/rolfie/projects/herdr-ade/.worktrees/a0)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.11s
```

`Cargo.toml` allows `clippy::collapsible_if`, `too_many_arguments`, `type_complexity`. Those three already fail this gate on main at a4cdb0a in unowned `ticker.rs` / `lifecycle.rs` / FakeRunner.

### `cargo test --locked`

Exit 0. 169 unit tests + 4 `tests/cli.rs`.

```
test result: ok. 169 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.20s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.17s
```

### `cargo build --release --locked`

Exit 0. Binary `/Users/rolfie/projects/herdr-ade/.target/a0/release/herdr-ade`.

```
    Finished `release` profile [optimized] target(s) in 8.09s
```

### doctor

```
HERDR_ADE_ROOT=/var/tmp/ade-a0/root XDG_CONFIG_HOME=/var/tmp/ade-a0/xdg $CARGO_TARGET_DIR/release/herdr-ade doctor
```

Exit 1 (herdr 0.9.0 < 0.9.1). Ran as `herdr-ade`. Root `/var/tmp/ade-a0/root`. Did not create `~/.herdr-ade` or `~/.config/herdr-ade`. Did not write outside `/var/tmp/ade-a0/` (the two dirs there were created empty by the test setup). `XDG_CONFIG_HOME` is unused: `paths.rs` still uses `$HOME/.config/herdr-ade`.

```
binary:     /Users/rolfie/projects/herdr-ade/.target/a0/release/herdr-ade
version:    0.1.0+20fd9e9.1789765037
root:       /var/tmp/ade-a0/root
config dir: /Users/rolfie/.config/herdr-ade

[FAIL] herdr: 0.9.0 (/Users/rolfie/.local/bin/herdr); 0.9.1 or later is required
[ok  ] session: /Users/rolfie/.config/herdr/herdr.sock (name: -)
[ok  ] git: git version 2.50.1 (Apple Git-155)
[ok  ] ssh: OpenSSH_10.3p1, LibreSSL 3.3.6
[ok  ] rsync: openrsync: protocol version 29
[ok  ] gh: gh version 2.97.0 (2026-07-31)
[warn] gh auth: You are not logged into any GitHub hosts. To log in, run: gh auth login; pull request follow-up will not work
[ok  ] root: 0 project(s)
[warn] ticker: not running
herdr-ade: some checks failed
```

## What I could not do

1. Print the full D1 tuple from `ha doctor` (plugin id, crate, binary, prefix). `src/doctor.rs` is not owned. After the rename it already prints binary path, root, and config dir.
2. Honour `XDG_CONFIG_HOME` for the config dir. Resolution lives in unowned-for-this-purpose `paths.rs` (owned for the rename of the default path and env name only).
3. Make package-wide `cargo fmt --check` exit 0 without rewriting unowned modules. Main already fails that command.
4. Implement `ha done`, `ha waiting`, rounds, ask, say, talk. A0 only wires FakeRunner replies.
5. Avoid `tests/cli.rs`. The binary rename requires `CARGO_BIN_EXE_herdr-ade`.

## Open questions (numbered from 1 for SPEC-ADE §6)

1. `cargo fmt --check` fails on main at a4cdb0a. Should A0 keep unowned wrapping, or should a later lane format the crate once on the integration branch?
2. `clippy -D warnings` fails on main (`too_many_arguments` on `ticker.rs`, `collapsible_if` on `lifecycle.rs`). A0 allows those three lints in `Cargo.toml`. Should a later owner of those functions fix them and drop the allows?
3. Doctor does not print plugin id / crate / command prefix as D1 states. A1 owns `src/doctor.rs`. Should A1 add that line?
4. `paths.rs` ignores `XDG_CONFIG_HOME`. The throwaway isolation in the brief sets it. Should config resolution read `XDG_CONFIG_HOME`?
5. Item 24: does D18 ship in the first plugin round? Still Rolf's.
6. Item 29: SCOWL 2020.12.07 cut at 5000, licence recorded. Confirm this source, or name a different redistributable list.
7. Default `herdr` on this machine is 0.9.0; plugin `min_herdr_version` is 0.9.1. Doctor fails that check. Is the live binary the r2 fork that still reports 0.9.0?
````

### ade-core (lane a1, `lane/ade-core` 1a947922)

````markdown
# ade-core (lane a1)

Final commit: `1a947922218d5b7c7960c213bcca3fe4f71129fc`

Commits on `lane/ade-core`:
- `5c16f9c` feat(ade-core): roles, git worktrees, parent launch and doctor D1
- `e494b8d` feat(ade-core): add the state-dependent herdr binary swap
- `1a94792` docs(ade-core): describe roles, --plain and git worktree tabs

## Read

- `tasks/ade-core.md`
- A0 report at `/Users/rolfie/projects/herdr-ade/.worktrees/a0/.reports/ade-contracts-report.md`
- SPEC-ADE v3.1 D1–D4, D6, D9, D11, D13, D17 item 6, §1.3, §3.3 step 6, §4.2
- A0 types in `src/contracts.rs` (no fields added)

## Built (owned files)

- `src/main.rs`: `mod git` in the ade-core marked block.
- `src/cli.rs`: `--plain`, `--role`, `--passive`; CLI calls `start_with_ade` / `adopt_with_ade`. Missing `--plain` is `plain_missing` (not a clap required-arg).
- `src/paths.rs`: `$XDG_CONFIG_HOME/herdr-ade` else `~/.config/herdr-ade` (item 39).
- `src/project.rs`: roles table (replace-not-merge, `role_args_missing`, unknown fields refused, picker keys skipped). `talk`. `coordinator_agent` / `thread_agent` skip-serialize. Launch recipe, `policy_hash`, `tab_env` with `HERDR_ADE_LAUNCH`. A4 hook comment: `crate::picker::resolve_launch(...)`.
- `src/herdr.rs`: `Agent.tokens`, `--parent`, `ready_timeout_ms`, `tab create --env`, `pane get`, `pane process-info`, pane parent token with no TTL. Source `herdr-ade`.
- `src/git.rs`: repo lock on `git-common-dir`, `worktree add` / `remove` without `--force`, `commit_file_on_branch` (D9 three checkout cases), `commit_file_from_parent` (ADE brief, does not move the integration branch), `update-ref`, ancestry.
- `src/thread.rs`: ADE record fields, `is_ade()`, `identity_verifies`, `bind_identity` (no `terminal_id`; adopted has no `agent_name`).
- `src/threads.rs`: dual path (`start` legacy for unowned tests; CLI ADE path `start_with_ade`). Birth sentence. Remote ADE `remote_not_admissible`. Git worktree + tab `--env`. Restart reopens a tab, never `worktree open`. Remove uses `git worktree remove` + `tab close`. `pub fn tick` lineage repair.
- `src/adopt.rs`: `adopt_with_ade`, `--passive` sends no prompt, parent token set.
- `src/ticker.rs`: `Ticker` for other lanes. `LaunchPass` packed so `too_many_arguments` is gone from `Cargo.toml`. ADE launch uses the stored recipe + `--parent` + identity bind. Calls `crate::threads::tick`.
- `src/doctor.rs`: D1 tuple (plugin/crate/binary/prefix). Fork `0.9.0` accepted only with `--parent` on CLI until install day `0.9.1`. Legacy PROJECT.md agent keys FAIL. Role permission-arg warnings.
- `scripts/migration/swap-binary.sh` + `.test.sh`: exclusive-create backup, already-target reuse, failure cases. `HERDR_ADE_SWAP_DIR` so tests never write `~/.local/bin`.
- `README.md`, `docs/operations.md`: roles, `--plain`, git worktree tabs.

Library `start()` / `adopt()` stay legacy so unowned `src/scenarios.rs` still compiles. No fields added to `StartArgs`.

## Tick entry point

```rust
pub fn tick(t: &mut crate::ticker::Ticker<'_>) -> anyhow::Result<()>
```

in `src/threads.rs`. Wired from A1 `tick_cheap`. Lineage repair only when pid and argv0 verify; else `lineage-mismatch`. Resolved threads are not repaired. Adopted threads without a stored process may set the parent token.

## Contract fields

None added. Used A0 `RoleSpec`, `LaunchRecipe`, `IdentityBinding`, `ProcessIdentity`.

## Gates

Commands used `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a1` and `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, `cargo +1.89.0`.

### rustfmt

Owned files only, with `--config skip_children=true` (formatting `src/main.rs` without that option rewrites unowned modules). Exit 0.

`cargo fmt --check` on the crate still fails on pre-existing wrapping in unowned files (`coordinator.rs`, `inbox.rs`, `lifecycle.rs`, `remote.rs`). A0 open question 1: formatted what we own; did not reformat other lanes.

### clippy

`cargo +1.89.0 clippy --all-targets --locked --offline -- -D warnings`

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.02s
```

Dropped `too_many_arguments` from `Cargo.toml`. Kept `collapsible_if` (unowned `lifecycle.rs`) and `type_complexity` (FakeRunner).

### test

`cargo +1.89.0 test --locked --offline`

```
test result: ok. 191 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.42s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.19s
```

§1.3 owned rows covered: `--parent` on launch argv, tab `--env HERDR_ADE_LAUNCH`, git `worktree add` without `--force`, restart opens a tab not a workspace, persisted launch recipe, `plain_missing` / unknown-word refusal, remote ADE refused, passive adopt, lineage-mismatch does not repair, D9 dirty checkout and from-parent commit.

### swap-binary (this Mac)

`sh scripts/migration/swap-binary.test.sh`

```
ok   first run
ok   already-target with source backup (no write)
ok   already-target missing backup (refused)
ok   installed hash matches neither (refused)
ok   backup wrong hash (refused)
ok   backup already source is reused
ok   installed is a symlink (refused)
all swap-binary tests passed
```

M1 is no longer blocked on this script for this Mac.

### release

`cargo +1.89.0 build --release --locked --offline`

```
    Finished `release` profile [optimized] target(s) in 3.59s
```

### throwaway doctor

Stub `herdr` under `/var/tmp/ade-a1/bin` (no live session). `HERDR_ADE_ROOT=/var/tmp/ade-a1/root`, `XDG_CONFIG_HOME=/var/tmp/ade-a1/xdg`, `--socket /var/tmp/ade-a1/no.sock`, `env -u HERDR_SOCKET_PATH -u HERDR_SESSION`. Did not write `~/.config` or `~/.herdr-ade`.

```
plugin:     herdr-ade
crate:      herdr-ade
binary:     /Users/rolfie/projects/herdr-ade/.target/a1/release/herdr-ade
prefix:     /Users/rolfie/projects/herdr-ade/.target/a1/release/herdr-ade --root /var/tmp/ade-a1/root
[ok  ] herdr: 0.9.1 (/var/tmp/ade-a1/bin/herdr)
[ok  ] parent: CLI `agent start --parent` (fork)
[ok  ] root: 0 project(s)
[warn] role coordinator: kind claude has no args; a permission flag is usually required
```

Exit 0.

## Left for the reviewer

Exact lines already in `src/ticker.rs` `tick_cheap`, after `crate::threads::tick`:

```rust
    // crate::ops::tick(&mut ticker)?;
    // crate::round::tick(&mut ticker)?;
    // crate::picker::tick(&mut ticker)?;
```

Exact line already in `src/project.rs` `load_roles`:

```rust
// `let spec = crate::picker::resolve_launch(project, role, task, sibling)?;`
```

A2/A3/A4 should expose `pub fn tick(t: &mut crate::ticker::Ticker) -> anyhow::Result<()>`.

## Could not do

- Whole-crate `cargo fmt --check` (unowned wrapping). Owned files are clean with `skip_children`.
- Throwaway herdr session `ade-a1`: doctor did not need a live server; a stub answered `--version`, `session list`, and `agent start --help`. Did not start or stop session `default`.
- `herdr plugin link` / `install`, `cargo install`, writes under `~/.config` / `~/.herdr-ade`: not done.
- Hash-mismatch / wrong-tab removal-gate rows as full FakeRunner scenarios: dirty tree and working-agent refusals exist; remaining D4 gate rows are unit-level.

## Open questions (SPEC-ADE §6)

1. Missing `--plain` now returns `plain_missing` from the mechanic. Confirm the reviewer row wants that string, not clap's "required arguments were not provided".
2. ADE restart increments `attempt` before `tab create`. Confirm `HERDR_ADE_LAUNCH` on restart should carry the new attempt.
3. Brief commits for a new lane use `commit_file_from_parent` and do not move `main`. Merge (D6/D9 checkout-dirty) still uses `commit_file_on_branch`. Confirm that split.
4. Doctor accepts a 0.9.0 version string only when `agent start --help` names `--parent`. Confirm that is enough until install day 0.9.1.
````

### ade-outbox (lane a2, `lane/ade-outbox` 2d8ff3a1)

````markdown
# Lane a2 report: ade-outbox

Final commit sha: `2d8ff3a114687e995d802db321df0b2d7e3161df`
Branch: `lane/ade-outbox`
Worktree: `/Users/rolfie/projects/herdr-ade/.worktrees/a2`
Toolchain: Rust 1.89.0, edition 2024

## What I read

- `tasks/ade-outbox.md`
- A0 report `/Users/rolfie/projects/herdr-ade/.worktrees/a0/.reports/ade-contracts-report.md`
- `SPEC-ADE.md` v3.1: D5 lines 368-445; D12 lines 579-586; D14-D16 lines 915-959; D17 lines 594-742; D18 lines 744-830; §1.3 lines 832-897; A2 ownership lines 1282-1292; acceptance rows 3, 4, 6, 7, 9 and 10; items 28 and 32-35
- `tasks/ade/decisions.md`
- A0 shared contracts and checker, and the A0/base interfaces in `project.rs`, `thread.rs`, `herdr.rs`, `runner.rs`, `ticker.rs`, `inbox.rs`, `steps.rs`, `coordinator.rs`, `cli.rs`
- Installed CLI evidence: Claude Code 2.1.277, Cursor Agent `2026.09.15-d2fe57e`, Codex CLI 0.155.1/0.155.0 doctor; official Cursor hooks page for `.cursor/hooks.json`, `afterAgentResponse`, `stop`, and `followup_message`; installed Codex binary hook schema strings and isolated `codex doctor`

## Commits

1. `da8c67d` `feat(outbox): add durable operations and sealed events`
2. `4995ea4` `feat(outbox): add lane completion delivery and receipts`
3. `64c6588` `feat(outbox): enforce coordinator correction hooks`
4. `64fd621` `fix(outbox): use installed per-kind hook formats`
5. `2d8ff3a` `fix(outbox): bind replacement coordinator receipts`

## What I built

### `src/ops.rs` — D5 lines 376-443; item 32

- Reserve stores the complete tagged request, round, recipient, helper pid, revision and fixed event id under the project lock.
- Same-payload retries resume; changed sha/report/text abandons and allocates `n+1`.
- `done` staging reads twice, requires stable bytes, an empty `git status --short`, exact `HEAD`, and writes a content-addressed fsynced artifact.
- `waiting` bounds and reconstructs text without a clean-tree or sha requirement.
- Seal checks state/revision and binding, creates the event once, accepts only byte equality, and repairs X2b to revision 3.
- Recovery abandons dead reserved helpers, seals staged ops from their durable payload, and exposes preparation state.
- Fixtures cover dirty tree, wrong sha, unstable report, helper death after staging, helper/ticker race, X2b repair, same-payload resume, changed-payload supersession, and waiting from a dirty tree contract.

Ticker entry point: `ops::tick(ctx: &Ctx, project: &Project) -> Result<()>` at `src/ops.rs:281`. A0 has no `crate::ticker::Ticker` type, so the A1 call-site line is listed under reviewer work.

### `src/events.rs` — D5 lines 376-411, 425-431

- Immutable TOML events use create-new plus exact byte comparison.
- Delivery facts are fsynced JSONL lines; acknowledgement and handling are idempotent, while submitted may repeat across X4.
- Typed `DONE`/`WAITING` lines derive only from sealed events.

### `src/lane.rs`, `skill/LANE.md`, compatibility `skill/THREAD.md` — D5, D12, D14

- Added `ha done`, `ha waiting`, and role-aware `ha skill`.
- Lane lookup is pane/socket/cwd-bound, remote lanes refuse, and seal rechecks current attempt and coordinator recipient.
- Bootstrap receipt consumes `HERDR_ADE_LAUNCH`; repeated identical receipt says already accepted; mismatch refuses.
- `RULES.md` is appended at runtime only and refuses over 64 KiB.
- `LANE.md` says repeated skill calls continue the attempt and finish only through the durable verbs.
- A short `THREAD.md` compatibility file remains because A0's unowned `thread.rs` still has `include_str!("../skill/THREAD.md")`.

### `src/steps.rs` — D5 lines 403-445

- Delivery verifies current recipient/attempt, writes an event-linked inbox item, projects/clears lane tokens, types only to a matching ready coordinator, then journals submitted.
- Recipient replacement writes a separately keyed event-linked `recipient-changed` item for the new binding.
- Exposes `deliver_events`, `deliver_event`, `config_changed`, and `report_available`; the latter explicitly says report bytes are not completion.

### `src/inbox.rs` — D5 lines 405-411, 432-435

- Items carry an optional sealed event id.
- Event projection ids are stable and idempotent.
- `ha context` acknowledgement requires the exact recipient pane/attempt; `--peek` calls no acknowledgement path.
- A replacement coordinator may acknowledge only the `recipient-changed` projection under its new binding.
- Handling an event item requires its valid binding and appends `handled`.

### `src/coordinator.rs` — D12, D14, D17 item 2

- `open` installs and verifies the owned hook before `agent_start`; rebind and `ha close` remove only the owned entry.
- Transport submission no longer represents receipt in this module; a matching `ha context` receipt clears pending.
- Context prints the honest capability label, completion preparations/abandonments, and acknowledges shown event items only from the bound coordinator.
- Priming names `skill coordinator`; `close` retires the binding.

### `src/adapters.rs` — D14-D15

- Seven-kind table records receipt path, positive resend evidence, required use, correction mechanism and honest label.
- Empty composer is never positive evidence. Only explicit rejection for Claude/Codex and the observed unsubmitted pasted block for Cursor allow retry.
- Capability remains `unqualified; chat: shown only through say and ask` until a project-local qualification marker exists.

### `src/hook.rs` — D17 item 2, item 3; D18 item 3; item 28; item 35

- Idempotent per-project install/remove preserving unrelated entries:
  - Claude: `.claude/settings.local.json`, `hooks.Stop`.
  - Codex: `.codex/hooks.json`, Claude-compatible `hooks.Stop` confirmed by installed schema/config load.
  - Cursor: version-1 `.cursor/hooks.json`; `afterAgentResponse` observes `text`, persists the check result, and `stop` returns the bounded `followup_message`.
- Hook scope requires inherited pane plus stored project/kind/pane/session binding.
- Budget persists by native turn: three corrections, one optional translator, ten minutes, 64 KiB input, 60-second subprocess timeout. Translator is absent/off unless configured.
- Exhaustion queues only notice id `plain-budget-exhausted`, whose fixed text is `The coordinator could not say this plainly. Open its pane to read it.`
- Parser forwards only typed `ade-say` and `ade-ask` blocks. Prose, including prose questions, is never forwarded. Ask blocks resolve the stored id/revision and exact checked question.
- Typed publications dedupe by session, turn and message.
- Fixtures cover raw-question bypass, prose privacy, duplicate publication, budget continuation/exhaustion, duplicate install/removal, unrelated-entry preservation, and all three installed project config shapes.

### `src/cli.rs`, `src/main.rs`

- Registered A2 modules only in `// ade-outbox begin/end`.
- Registered `done`, `waiting`, role `skill`, `close`, `plain check`, hidden `plain hook`, and `dialogue start`'s `plain_missing` refusal in A2 blocks.
- `inbox done` supplies the current coordinator binding.

## Contract fields added

None. A0's `Op`, `Event`, `DeliveryLine`, `Recipient`, `HumanMessage`, and `Ask` were sufficient.

## Installed CLI qualification probes

- Claude Code 2.1.277: isolated HOME/XDG/root and throwaway Herdr session `ade-a2`; `ha open` installed the owned Stop entry before `agent.start`; kind `claude` launched with `--model claude-haiku-4-5-20251001 --dangerously-bypass-hook-trust`. The pane stopped at `OAuth error: Invalid code` because the isolated HOME had no credentials. No hook publication was claimed. I did not use or edit `~/.claude`.
- Cursor Agent `2026.09.15-d2fe57e`: official installed-version help exposes no hook diagnostic. The current official hooks schema was matched and unit-tested, but no isolated authenticated response ran. Row remains unqualified. I did not use or edit `~/.cursor`.
- Codex CLI 0.155.1 (doctor runtime 0.155.0): installed binary includes Stop input fields `last_assistant_message` and `stop_hook_active`; isolated `CODEX_HOME=/var/tmp/ade-a2/codex-home codex doctor --json` loaded config with hooks enabled and accepted the project hook shape, but auth failed because the isolated home had no credentials. Row remains unqualified. I did not use or edit `~/.codex`.
- The throwaway Herdr session and plugin ticker were stopped. The default/live session was never addressed.

## Gates

All commands used `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a2` and `DEVELOPER_DIR=/Library/Developer/CommandLineTools`.

### `cargo +1.89.0 fmt --check`

Exit 1, the pre-existing A0/A1-unowned formatting failure documented in the A0 report. Tail:

```text
Diff in /Users/rolfie/projects/herdr-ade/.worktrees/a2/src/ticker.rs:859:
     fn log_is_capped() {
         let dir = tempfile::tempdir().unwrap();
-        let log = Log { path: dir.path().join("log") };
+        let log = Log {
+            path: dir.path().join("log"),
+        };
```

Owned-file proof:

```text
rustfmt +1.89.0 --edition 2024 --check --config skip_children=true src/main.rs src/cli.rs src/ops.rs src/events.rs src/lane.rs src/steps.rs src/inbox.rs src/coordinator.rs src/adapters.rs src/hook.rs
owned rustfmt exit=0
```

### `cargo +1.89.0 clippy --all-targets --locked -- -D warnings`

Exit 0.

```text
   Compiling herdr-ade v0.1.0 (/Users/rolfie/projects/herdr-ade/.worktrees/a2)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.36s
```

### `cargo +1.89.0 test --locked`

Exit 0.

```text
test result: ok. 191 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.70s

running 4 tests
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.19s
```

### `cargo +1.89.0 build --release --locked`

Exit 0. Final binary carries `0.1.0+2d8ff3a`.

```text
   Compiling herdr-ade v0.1.0 (/Users/rolfie/projects/herdr-ade/.worktrees/a2)
    Finished `release` profile [optimized] target(s) in 3.83s
```

### throwaway doctor

Command:

```text
HOME=/var/tmp/ade-a2/home XDG_CONFIG_HOME=/var/tmp/ade-a2/xdg XDG_STATE_HOME=/var/tmp/ade-a2/state HERDR_ADE_ROOT=/var/tmp/ade-a2/doctor-root HERDR_BIN_PATH=/Users/rolfie/projects/herdr/.target/install/release/herdr /Users/rolfie/projects/herdr-ade/.target/a2/release/herdr-ade doctor --session ade-a2
```

Exit 1 solely on the already-recorded fork version string. Tail:

```text
version:    0.1.0+2d8ff3a.1789769585
root:       /var/tmp/ade-a2/doctor-root
config dir: /var/tmp/ade-a2/home/.config/herdr-ade

[FAIL] herdr: 0.9.0 (/Users/rolfie/projects/herdr/.target/install/release/herdr); 0.9.1 or later is required
[ok  ] session: /var/tmp/ade-a2/xdg/herdr/sessions/ade-a2/herdr.sock (name: ade-a2)
[ok  ] git: git version 2.54.0 (Apple Git-157)
[ok  ] ssh: OpenSSH_10.3p1, LibreSSL 3.3.6
[ok  ] rsync: rsync  version 3.5.0  protocol version 32
[ok  ] gh: gh version 2.97.0 (2026-07-31)
[warn] gh auth: You are not logged into any GitHub hosts.
[ok  ] root: 0 project(s)
[warn] ticker: not running
herdr-ade: some checks failed
```

The session was stopped with:

```text
/Users/rolfie/projects/herdr/.target/install/release/herdr --session ade-a2 session stop ade-a2
stopped session ade-a2
```

## Left for the reviewer

1. A1 owns `ticker.rs`. Wire the A2 pass once per active project, outside the project lock, with this exact line in the slow-pass error vector:

   ```rust
   errors.extend(crate::ops::tick(ctx, project).err());
   ```

2. A1's branch supplies the extended thread record and identity helper. Replace A2's A0 compatibility reads with these exact field reads after merge:

   ```rust
   let attempt = binding.thread.attempt;
   ```

   and call A1's process-identity verifier in `lane::current_lane` and the seal validator before accepting the pane. A0 only exposes pane/socket/cwd/name and `launch_attempts`.

3. After A1 changes its unowned brief include, use this exact line in `thread.rs`, then delete the compatibility `skill/THREAD.md`:

   ```rust
   include_str!("../skill/LANE.md")
   ```

4. A1/reviewer should update the legacy report-hash scenario and replace A0's compatibility writer in `steps::write_thread_items` with:

   ```rust
   inbox::write(project, "report-available", &t.id, &summary, "")?;
   ```

   `report_available()` already exposes the non-completion template.

5. A3 owns the final talk/board publisher. Consume each checked `hook::Publication` through A3's one typed publisher with the equivalent exact call:

   ```rust
   crate::talk::publish(project, publication.message)?;
   ```

   Do not forward native prose or read the pane screen. The current A2 queue is `.state/plain/publications.jsonl` and is deduplicated.

6. A3 owns `skill/COORDINATOR.md`; add the exact `ade-say` / `ade-ask` envelope instructions there. A2 enforces them without trusting that instruction.

7. The reviewer/acceptance owner must create `.state/capabilities/<kind>.qualified` only after that installed CLI's authenticated row passes. Until then context remains explicitly unqualified.

## What I could not do

1. I could not complete an authenticated Claude, Cursor, or Codex hook turn inside the isolated HOME because it contained no credentials. Using the real homes was prohibited. None of those adapter rows is claimed qualified.
2. Package-wide format check cannot pass without formatting unowned base files; owned files pass rustfmt.
3. Doctor cannot pass while the fork release binary reports `herdr 0.9.0` against the plugin's minimum 0.9.1. Session and all other local tool checks were reached.
4. I could not expose the brief's literal `ops::tick(t: &mut crate::ticker::Ticker)` signature because A0 has no `Ticker` type and A1 owns that file. The buildable A0 seam is `ops::tick(ctx, project)` and its exact integration line is above.
5. I did not run the reviewer-owned assembled acceptance script; the brief says to make its rows runnable, not run them in this lane.

## Open questions (numbered from 1 for SPEC-ADE §6)

1. Should the reviewer keep A2's buildable `ops::tick(ctx, project)` seam, or adapt it to an A1 `Ticker` receiver after the A1 merge?
2. Who provides isolated authenticated CLI homes for the required Claude and Cursor acceptance rows without touching real user config? Until then they correctly remain unqualified.
3. Should A1's integration commit remove the compatibility `skill/THREAD.md` immediately when it changes the include to `LANE.md`?
4. The installed fork release still prints 0.9.0 although it contains the 0.9.1-based `--parent` work. Should A1's doctor special-case the fork version string as decision item 42 says, or should the release version be corrected before acceptance?
````

### ade-rounds (lane a3, `lane/ade-rounds` 99e1c0e9)

````markdown
# ade-rounds report (lane a3)

Branch `lane/ade-rounds`, worktree `.worktrees/a3`, cut from A0's pin `20fd9e9c`.
Final commit: `99e1c0e909c92a4d2858dc74f0759e24ab22c530`.

## What I read

- `tasks/ade-rounds.md` (the brief) and A0's report `.worktrees/a0/.reports/ade-contracts-report.md`.
- SPEC-ADE v3.1: §0.3 (O1 to O8), §0.4 (r2), §1.2 D6, D7, D8, D9, D10, D13, D17 items 1 to 7, D18 items 1 to 7, §1.3 scenarios, §4.2 lane split, §4.3 acceptance rows, §6 items 14, 24, 32 to 35, and `tasks/ade/turns/06-pro.md` (the recovery table).
- `hp:` sources I build on: `src/project.rs`, `src/herdr.rs`, `src/thread.rs`, `src/scenarios.rs`, `src/runner.rs`, `src/overview.rs`, `src/coordinator.rs`, `skill/COORDINATOR.md`, `docs/herdr-notes.md`; A0's `src/contracts.rs`, `src/plain.rs`, `plain/words.txt`, `plain/vocabulary.txt`, `scripts/acceptance`.
- `save-state/state.py` (read only) for the checkpoint port.
- The fork CLI's `--help` for every call I make (tab create, pane run, pane read, pane send-keys, agent send-keys, workspace/pane report-metadata, api snapshot, pane process-info, notification show).

## What I built, per owned file

- `src/round.rs` (D6, items 32 to 34, D9). Round records under `.state/rounds/r<n>/`: `round open` (with `--plain`, birth check, `plain_missing`, `branch_missing`, `round_exists`; stamps workspace tokens `round`/`branch`), `admit` (`remote_not_admissible`, `round_closed`), `remove`, `reviewer`, `review` (brief commit B through D9 mechanics, then `git worktree add` of `review/r<n>` from B; revision and manifest hash frozen; reports pasted in fences longer than any inner run, labelled data), `merge` (verdict checks: V's only parent is C, C..V touches only `tasks/reviews/code-r<n>.md`, candidate, round, MERGE only, manifest and policy hash, B ancestor of C, every pin ancestor of C; then intent, ref under the repository lock (ff-only in a clean checkout, `update-ref <V> <old>` when not checked out, `integration_checkout_dirty` otherwise), merged, checkpoint intent with the staged payload bytes and `payload_hash`, H, checkpointed; recovery exactly as the table; everything else `merge_diverged` plus an inbox item). Pins come only from sealed done events of the current attempt; a changed pin or an unpin bumps the revision (item 33: a missing or corrupt record fails closed with `round_manifest_unavailable`; a manifest changed after B makes merge stale). Test-only `--stop-after ref|merged|intent|commit`. Lane worktrees fast-forwarded after checkpointed, a dirty one is reported, not touched. `pub mod repo` is a stand-in for A1's `git.rs` (repository lock at `<git-common-dir>/herdr-ade.lock`, commit on branch, ancestry). `herdr_quiet` is a stand-in for calls that succeed silently (see Left for the reviewer).
- `src/ask.rs` (D17 items 3 and 4, item 35). The one publisher `publish(HumanMessage)` for `Say`, `Ask{id,revision}` (resolved from the stored record; unknown, closed, stale refused), `Notice{id}` (fixed ids only). `ha ask`: structured check (2 to 4 choices, R6, envelope), immutable record `asks/<id>/r<rev>.toml` written first under the lock, then the compact line (at most 60 characters, checked), the journal entry, the board row, the notification, then a `published` marker; `ask::tick` resumes a crash between record and publication. `ha ask answer` (`ask_revision_stale`, `ask_closed`, `ask_choice_out_of_range`); choice 0 records not understood and counts it. `ha say`.
- `src/board.rs` (item 14, gate B). Five workspace tokens with TTL 300 s from binary-owned templates; every value at most 80 characters and checked; a failing value is refused and the previous value is re-sent. `board::stage` is also the overview's stage line.
- `src/glossary.rs` (D17 item 6). Registry built from threads, rounds, dialogues and `terms.toml`, passed to A0's checker as a value; birth check; `GLOSSARY.md` rewritten atomically, newest last; `ha explain`, `ha term add` (with the `--plain` refusal, `term_exists`).
- `src/talk.rs` (D18). Journal with one append owner (`journal.lock`, fsync, 64 KiB bound, keyed dedup, a cut tail terminated and reported once as `journal_tail`); requests queued, submitted, uncertain, accepted (uncertain never re-sent; a changed recipient never retargeted); the serialized writer lock; `!native`, `!back` (re-reads readiness), `!stop`; `/` passes through; a number answers only the ask frozen when the prompt was drawn, else `ask_redrawn`; `--replay` renders and never sends; the talk tab (`tab create --label talk`, then `pane run`); ticker pass (a `needs_you_in_pane` notice once per blocked episode, queued requests delivered when ready). Item 24 decision: on by default for a `claude` coordinator, off for other kinds, `talk = true|false` in front matter overrides.
- `src/dialogue.rs` (D7). `dialogue start` (`--plain`, birth check, `PairFilter` for A4), `critic` (Pro must be kind `chatgpt` named `pro`), `turn` (pinned before the line is typed; `turn_outstanding`, `--resend`), `commit` (`turn_mismatch`, `turn_file_missing`, `turn_file_empty`, `turn_file_exists`, commit through D9 under the repository lock, then advance).
- `src/checkpoint.rs` (D9, the `state.py` port). `collect` (api snapshot), `render` (`## Herdr`), `splice_herdr`, `check_document` (ids, uuids, paths, branches, placeholders, workers mentioned, required sections, one Next), `ha checkpoint` (check, then HANDOFF.md and HANDOFF.json as one commit H), `ha pickup` (re-parents live threads, prints start lines from the launch recipe, never starts or prompts).
- `src/overview.rs`: the round stage and the questions waiting (count and newest compact line).
- `skill/COORDINATOR.md`: talk, say, ask, the `ade-say`/`ade-ask` envelope, rounds, and "never merge except through `round merge`". New `skill/REVIEWER.md` (verdict front matter), `CRITIC.md`, `DRAFTER.md`, `PICKUP.md`.
- `docs/herdr-notes.md`: O5/O6 stage, the r2 facts and what they mean for the plugin, the CLI shapes rounds uses.
- `scripts/acceptance/run` and `rows.md`: every §4.3 row wired from parts (det, mod, cli, live); see Gates.
- `src/main.rs` and `src/cli.rs`: only the `ade-rounds` marked blocks (mods; verbs `round`, `dialogue`, `checkpoint`, `pickup`, `ask`, `say`, `explain`, `term`, `talk`, `board`).

Tests: 56 of mine (round 22, ask/board/glossary 11, talk 14, dialogue 4, checkpoint 5; plus overview assertions in an existing test), all FakeRunner with git through RealRunner on temp repos.

Live smoke on the throwaway session `ade-a3` (fork build, isolated XDG, root `/var/tmp/ade-a3/root`): `round open` stamped `round`/`branch`; `board` published five rows and `api snapshot` read them back (`ade_needs_you` equalled the checked compact line); `say` and `ask` published; `talk --open-tab` created the tab and the surface rendered the header, the say line and the numbered question in a real pane (`pane read`); typing `2` there recorded the answer and queued the ANSWER line (no coordinator agent, so `request_waiting`). Session stopped with `session stop`; nothing under `~/.config` or `~/.herdr-ade` was created.

## Tick entry point

`round::tick(ctx: &Ctx, project: &Project) -> anyhow::Result<()>` in `src/round.rs:1607`. The brief's `crate::ticker::Ticker` type does not exist on my base. The pass refreshes pins, forwards lanes after checkpointed, reports a pending merge once, then runs `ask::tick`, `talk::tick` and `board::refresh`.

## Contract fields added (each its own commit, additive, `#[serde(default)]`)

`RoundRecord.opened` (5158ffe), `RoundRecord.repo` (b8a9664), `RoundRecord.frozen_revision` (ff06790), `RoundRecord.review_branch` (6fae744), `RoundRecord.reviewer` (72eac1e); the roundtrip test covers them (5cee1fa).

## Gates

`CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a3 DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo +1.89.0 ...` at `99e1c0e`:

```
$ cargo fmt --check        -> exit 1
diffs only in unowned files: src/adopt.rs src/coordinator.rs src/doctor.rs src/inbox.rs src/lifecycle.rs
src/project.rs src/remote.rs src/thread.rs src/threads.rs src/ticker.rs
(all my owned files, src/main.rs and src/cli.rs are clean; in cli.rs I applied only the hunks inside the
ade-rounds markers)

$ cargo clippy --all-targets --locked -- -D warnings   -> exit 0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 1.16s

$ cargo test --locked      -> exit 0
test result: ok. 225 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 11.33s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.21s

$ cargo build --release --locked   -> exit 0
    Finished `release` profile [optimized] target(s) in 5.11s
```

Throwaway doctor (`HERDR_ADE_ROOT=/var/tmp/ade-a3/root XDG_CONFIG_HOME=/var/tmp/ade-a3/xdg XDG_STATE_HOME=/var/tmp/ade-a3/state HERDR_BIN_PATH=<fork build> herdr-ade doctor`, session `ade-a3` running) -> exit 1:

```
binary:     /Users/rolfie/projects/herdr-ade/.target/a3/release/herdr-ade
version:    0.1.0+9789926.1789769844
root:       /var/tmp/ade-a3/root
config dir: /Users/rolfie/.config/herdr-ade

[FAIL] herdr: 0.9.0 (/Users/rolfie/projects/herdr/.target/install/release/herdr); 0.9.1 or later is required
[warn] session: /var/tmp/ade-a3/xdg/herdr/herdr.sock (name: -); not reachable
[ok  ] git: git version 2.54.0 (Apple Git-157)
[ok  ] ssh: OpenSSH_10.3p1, LibreSSL 3.3.6
[ok  ] rsync: rsync  version 3.5.0  protocol version 32
[ok  ] gh: gh version 2.97.0 (2026-07-31)
[warn] gh auth: You are not logged into any GitHub hosts. To log in, run: gh auth login; pull request follow-up will not work
[ok  ] root: 1 project(s)
[warn] ticker: not running
[ok  ] project demo: active; socket /var/tmp/ade-a3/xdg/herdr/sessions/ade-a3/herdr.sock; workspace w1 exists; coordinator pane w1:p1 exists
herdr-ade: some checks failed```

The FAIL is the herdr version: `/Users/rolfie/projects/herdr/.target/install/release/herdr --version` prints `herdr 0.9.0` (question 6). The session line probes the default socket, not the named session (base doctor, A1). The project line is ok.

Acceptance, deterministic mode (`ACC_DIR=/var/tmp/ade-a3/acc sh scripts/acceptance/run`, no `ACC_LIVE`) -> exit 1, 25 parts PASS, 9 FAIL, 12 NOT-RUN. Every det and cli part passes; every FAIL is another lane's test that does not exist on this branch; every NOT-RUN is a live part:

```
FAIL	1	ha new and open with claude coordinator: hook before launch, bootstrap acknowledged
  NOT-RUN	live	ha new and open, hook before launch, bootstrap acknowledged within 120 s (ACC_LIVE is not 1)
  FAIL	mod	coordinator receipt and hook install (no test matches /(coordinator|hook)::[a-z_:]*(bootstrap|receipt|hook_before)/ (lane A2))
FAIL	2	round open r1; claude and cursor lanes admitted: tabs, parent, briefs committed first
  PASS	cli	round open without --plain is refused
  PASS	cli	round open with --plain records the round
  PASS	cli	the round's sentence is in GLOSSARY.md
  NOT-RUN	live	claude and cursor lanes admitted: tabs, parent, briefs committed first (ACC_LIVE is not 1)
  FAIL	mod	brief committed before the worktree, launch record (no test matches /git::|threads::[a-z_:]*(brief_committed|brief_before|launch_record)/ (lane A1))
FAIL	3	each lane ha done seals events; peek does not acknowledge; unadmitted ha waiting from a dirty tree
  NOT-RUN	live	each lane's ha done seals an event; peek does not acknowledge (ACC_LIVE is not 1)
  FAIL	mod	done, waiting, seal and acknowledgement (no test matches /(lane|events|ops)::/ (lane A2))
  PASS	det	admission pins only sealed done events of the current attempt (2 tests)
FAIL	4	representative-topology handoff continuity and blocked-then-submitted delivery
  NOT-RUN	live	representative-topology continuity across a restart (ACC_LIVE is not 1)
  PASS	det	delivery: blocked keeps the request queued, then submitted in order (3 tests)
  FAIL	mod	outbox delivery, duplicate typed line consumed once by id (no test matches /(steps|events)::[a-z_:]*(duplicate|once_by_id|consumed_once)/ (lane A2))
FAIL	5	ha round review: brief commit B precedes review worktree; verdict V; ha done --sha V
  PASS	det	B committed before review/r1; brief pastes pins, event ids, artifacts, gates (2 tests)
  NOT-RUN	live	round review on the live project; reviewer produces C and V (ACC_LIVE is not 1)
FAIL	6	ha round merge stop after merged; resume to H; item 32-34 fixtures
  PASS	det	stop after merged shows V, resume commits H, second merge is a no-op (3 tests)
  PASS	det	item 34: crashes at ref, before H and after H recover without a second merge (6 tests)
  PASS	det	item 33: a removed or stale manifest fails closed (2 tests)
  PASS	det	D9 commit mechanics under the repository lock (2 tests)
  FAIL	mod	item 32: helper death after staging is sealed by the ticker, a changed sha supersedes (no test matches /(ops|events)::[a-z_:]*(item32|supersede)/ (lane A2))
FAIL	7	negative rows: dirty done, wrong sha, unstable report, incomplete review, stale merge verdicts
  FAIL	mod	ha done with a dirty tree, a wrong sha, an unstable report (no test matches /lane::[a-z_:]*(dirty|wrong_sha|unstable)/ (lane A2))
  PASS	det	round review with an admitted lane not done (1 tests)
  PASS	det	merge: MERGE-AFTER-DECISION, moved head, earlier round, parent not C, C..V touches code, lane not in C (2 tests)
FAIL	8	pro-mcp start, passive adopt, one TURN, one artifact event, ha dialogue commit
  PASS	det	dialogue: pinned turns, Pro recipient check, turn commit (4 tests)
  NOT-RUN	live	pro-mcp start, passive adopt, one TURN, one artifact event, dialogue commit (ACC_LIVE is not 1)
FAIL	9	kind rows: claude and cursor required; others NOT-RUN when unavailable
  NOT-RUN	live	claude as a lane, receipt observed (ACC_LIVE is not 1)
  NOT-RUN	live	cursor as a lane, receipt observed (ACC_LIVE is not 1)
  NOT-RUN	live	codex, opencode, agy, dsh: capability unqualified (ACC_LIVE is not 1)
  FAIL	mod	adapter table and capability labels (no test matches /adapters::/ (lane A2))
  PASS	det	talk header labels per kind (1 tests)
FAIL	10	plain-language rows: --plain, hook rewrite, talk captures, ha ask, board tokens, GLOSSARY.md
  PASS	det	the checker: R1 to R7 and the adversarial fixtures (6 tests)
  PASS	det	ask records first, compact line at most 60, notification; stale revision refused (4 tests)
  PASS	det	board: a failing value is not published, the old one stays; stage and lanes carry no name (2 tests)
  PASS	det	talk: journal, frozen ask binding, replay sends nothing, unchecked say not appended (4 tests)
  PASS	det	GLOSSARY.md and names at birth (1 tests)
  PASS	cli	ha ask with a term as a choice is refused
  PASS	cli	ha ask writes its record first (publication needs herdr)
  PASS	cli	the record exists
  PASS	cli	ha ask answer with a stale revision is refused
  PASS	cli	term add without --plain is refused
  PASS	cli	board values print and pass the check
  FAIL	mod	thread start without --plain refused; brief header plain:, no RULES.md (no test matches /(threads|thread)::[a-z_:]*plain/ (lane A1))
  FAIL	mod	the correction hook: rewrite on R1, budget, fixed notice, install and removal (no test matches /hook::/ (lane A2))
  NOT-RUN	live	hook rewrite captured before, during and after; talk never shows the bare text (ACC_LIVE is not 1)
  NOT-RUN	live	board tokens read back from the live workspace (ACC_LIVE is not 1)
FAIL	11	throwaway server stop; ha doctor reports unreachable; records intact
  NOT-RUN	live	throwaway server stop; ha doctor reports unreachable; records intact (ACC_LIVE is not 1)

REQUIRED	FAIL	0/11 pass
```

## Left for the reviewer (exact lines)

1. A1 `src/ticker.rs`, in the per-project pass: `if let Err(e) = crate::round::tick(ctx, &project) { log(&format!("rounds: {e:#}")); }` (or, with the brief's type, `pub fn tick(t: &mut crate::ticker::Ticker) -> anyhow::Result<()> { crate::round::tick(t.ctx, &t.project) }`).
2. A1 `src/herdr.rs`, `Herdr::call`: after the `if let Some(reply)` block, `if out.success() && out.stdout.trim().is_empty() && out.stderr.trim().is_empty() { return Ok(serde_json::Value::Null); }`. The fork prints nothing on success for `workspace/pane report-metadata`, `pane run` and `pane send-keys` (observed on `ade-a3`); the base `pane_report_tokens` fails there today. Then replace `crate::round::herdr_quiet(ctx, &h, ...)` with `h.call(...)` in round.rs, board.rs, checkpoint.rs, talk.rs.
3. A1 `src/git.rs`: replace `crate::round::repo::{Git, repo_lock, commit_files_on_branch}` with A1's `git::{..., repo_lock, commit_file_on_branch, update_ref}`; the lock path must stay `<git-common-dir>/herdr-ade.lock`.
4. A1 `thread start`: for a reviewer, `thread start <slug> --role reviewer --cwd <review worktree> --plain "..." --task-file tasks/review-r<n>.md`, then `round::bind_reviewer(ctx, slug, round, &thread.id)`; `round review` prints that line today.
5. A1 thread record: `attempt`, `plain` and `[launch]` are read tolerantly from the TOML by `round::thread_attempt`, `round::thread_plain`, `checkpoint::launch_of`; point them at A1's typed fields.
6. A1 `thread start` on a repo: `info/exclude` gets `.worktrees/` (my test fixture sets it by hand).
7. A2 `coordinator::open`, after the binding is recorded: `let _ = crate::talk::ensure_tab(ctx, &project);`
8. A2 outbox writer, around its read-and-prompt: `let _w = crate::talk::writer_lock(&project)?; if crate::talk::writer_suspended(&project) { return Ok(()); }`
9. A2 correction hook, after a turn ends: `crate::talk::mark_accepted(&project)?;` and per envelope block `crate::ask::publish_keyed(ctx, &project, &message, Some(&format!("hook:{session}:{turn}:{n}")))`; on budget exhaustion `crate::ask::publish(ctx, &project, &HumanMessage::Notice { id: "plain_exhausted".into() })`.
10. A2 `adapters.rs`: move `talk::chat_label`/`talk::surface_label` to the capability table and call it from `talk::header`.
11. A2 `ha context`: list open asks with `crate::ask::numbered(&ask)` and the not-understood count `crate::ask::not_understood_count(&project)`.
12. A2 `dialogue start` `--plain` refusal is in my `dialogue::start` already (the spec gives it to A2); keep one.
13. A4: pass its picker as `&dyn dialogue::PairFilter` in the `dialogue start` arm of `run_rounds` (today `&dialogue::AnyPair`).
14. `scripts/acceptance`: the `mod` parts name test patterns I expect from A1/A2 (e.g. `lane::...(dirty|wrong_sha|unstable)`, `hook::`, `adapters::`); adjust the patterns to the real test names on the merged candidate. The live parts check their verbs first and were never executed on this lane.

## What I could not do, and why

- No live part of §4.3 ran: A1's `thread start --role`, A2's `open` hook, receipt, `done`, `waiting`, `plain hook` are not on this branch, and I must not start agents against real credentials. The script reports them NOT-RUN with the reason.
- Readiness for talk delivery is the detector state `idle`/`done`, not an empty-composer read (A2's adapters own composer reads).
- `ha checkpoint` inside `round merge` composes the handoff without running `check_document` (an automatic checkpoint must not stop a merge that already landed V); `ha checkpoint` by hand does check.
- Row 4's representative topology (21 panes, restart drill) needs `herdr server restart` from lane/restart-core in C.

## Open questions for SPEC-ADE §6

1. The spec's exhausted-budget notice ("could not say this plainly") fails R4: `plainly` is not in `plain/words.txt`. I ship "The coordinator could not say this in plain words. Open its pane to read it." Add the word, or change the spec text?
2. The standing choice "I did not understand the question" fails R4 on `I`. I append it after the check as fixed text (a test pins the failure so a word-list fix shows). Add `i` to `words.txt`?
3. `merged`, `reviewed`, `unfinished`, `skipped` are not in the word lists; the board says "has landed" and "is in review". Add them to `vocabulary.txt`?
4. D18's header is "talking to <coordinator plain sentence>", but no record gives the coordinator a birth sentence. I print "the coordinator of <project name>". Should `ha new`/`ha open` take `--plain` for the coordinator?
5. The fork CLI prints nothing on success for report-metadata, `pane run` and `pane send-keys`, and the base `Herdr::call` reads that as a failure (Left for the reviewer, line 2). Is empty output the fork's intended contract?
6. The fork install build at `/Users/rolfie/projects/herdr/.target/install/release/herdr` reports `herdr 0.9.0`, and doctor fails it. Which binary is C for §4.3?
7. `talk_uncertain` reads "The own pane of the coordinator shows whether it did": the possessive `coordinator's` is not a word-list entry. Should the checker admit `'s` on known words?
8. When Rolf answers in talk, the surface records the answer and types `ANSWER a-<id>@<rev> <n>: Rolf chose "<choice>"` to the coordinator as a request. The spec does not say how the coordinator learns the answer. Is that line right, or should only `ha context` carry answers?
9. Notifications on the throwaway session returned `shown: false, reason: disabled`. The acceptance row counts the send. Should it require `shown: true`?
10. `round merge` fast-forwards released lane worktrees after checkpointed. Should that wait for Rolf when a lane's branch has commits past its pin?
````

### ade-picker (lane a4, `lane/ade-picker` 0d3e706e)

````markdown
# Lane a4 report: ade-picker

Final commit sha: `0d3e706e302c7120d8bd94533579c888637bfb94`
Branch: `lane/ade-picker`
Worktree: `/Users/rolfie/projects/herdr-ade/.worktrees/a4`
Toolchain: `cargo +1.89.0`, `DEVELOPER_DIR=/Library/Developer/CommandLineTools`,
`CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a4`.

## What I read

- `tasks/ade-picker.md` (this brief), `tasks/SPEC-ADE.md` v3.1 §1.2 D1–D18, §1.3, §2, §4.2
  A1–A3, §4.3, §6 items 32–35 and 36–42
- `/Users/rolfie/projects/herdr/tasks/jev/SPEC-jev-picker.md` v2 in full (especially §2, §3,
  "Design (normative)", "Plugin touch points by file", §5)
- `tasks/ade/decisions.md`
- A0's report `.worktrees/a0/.reports/ade-contracts-report.md`
- `src/contracts.rs`, `src/plain.rs`, `src/runner.rs`, `src/scenarios.rs`, `src/cli.rs`,
  `src/main.rs`, `src/project.rs`, `src/paths.rs`, `src/herdr.rs`, `src/doctor.rs`,
  `src/tests/cli.rs`
- The check b scripts (`tasks/jev/turns/02-opus-b/rerun_scrubbed.py`) and one cached TypeSafe
  response from jevtest for the exact request/response shape (`answers.<id>.noul`, `model`)
- `herdr agent start --help` on the fork release build: `[possible values: pi, claude, codex,
  gemini, cursor, devin, agy, cline, omp, mastracode, opencode, copilot, kimi, kiro, droid, amp,
  grok, hermes, kilo, qodercli, qwen, maki, muse]` (no `chatgpt`, no `dsh`)

## Commits

1. `cac0215` `contracts(ade-picker): picker types and launch fields`
2. `8f12947` `feat(picker): Jev client and launch resolver (SPEC-jev-picker v2)`
3. `9a139fd` `contracts(ade-picker): bool assert in the picker round-trip test`
4. `6c89c34` `test(picker): plain fixtures for the reason sentences (SPEC-jev-picker §3)`
5. `df34e03` `fix(picker): refuse unknown recipe fields and bad PROJECT.md pins`
6. `0d3e706` `fix(picker): warn when the task names a model, record the ade_last sentence`

## What I built, per owned file

### `src/contracts.rs` (additive, 199 lines added, 0 removed at `cac0215`; one line changed at `9a139fd`; `df34e03` adds `deny_unknown_fields`)

Added, with `#[serde(default)]` on every new field:

- `ResolverMode` (`off|shadow|jev|pin`) — SPEC-jev-picker v2 §2 Config, §2 Output
- `CostClass` (`default|sideways|upgrade`) — §2 Design C
- `Recipe { kind, args, env, ready_timeout_ms, provider, cost, enabled, plain }` —
  "Plugin touch points by file" row 1; `provider` reserved for the pi move (question 25)
- `GateCriteria { true, false }` and `Gate { recipe, cost, threshold, instructions, criteria }`
  — §2 Design C; `threshold: Option<f64>` so a gate can use its class floor
- `Launch`: the D2 row (`kind, args, env, ready_timeout_ms, policy_hash, attempt, brief_hash`)
  plus `recipe_id`, `resolver`, `gate`, `gate_p`, `jev_pick`, `jev_confidence`,
  `jev_probabilities`, `reason`, `compact_reason`, `fallback`, `jev_model`,
  `jev_input_tokens`, `jev_prompt_hash`, `excerpt_version` — §2 Output, D17 item 14
- JSON+TOML round-trip tests for all of them; `Recipe`/`Gate`/`GateCriteria` refuse unknown
  fields (D2 "unknown fields are rejected"), asserted in
  `contracts::tests::resolver_mode_and_cost_class_parse_from_lowercase`.

`compact_reason` is not in the spec's field list; I added it because §3 Publication says the
ticker sets `ade_last` to "<job> runs on <plain>" while D2 forbids rebuilding a launch from
live settings. The sentence is stored on the record.

### `src/jev.rs` (new, 1105 lines)

- Excerpt transform and scrub (§2 Input): drops `herdr agent start` lines, `Start line` lines and
  fenced code; collapses blank runs; cuts at 2,200 characters on a word boundary; case-insensitive
  whole-token replacement with `[agent]` and collapse of runs. Scrub list = every recipe kind,
  every `--model`/`model=` value and its word pieces, every codex `key=value` arg, every recipe
  `plain` phrase, the fixed list `grok opus fable sonnet astra sol gemini antigravity agy kimi
  muse claude codex cursor opencode dsh pi xhigh extra-high`, effort words (`HIGH`, `xhigh`,
  `max`, `medium`, `minimal`, `extra high`, …) and permission flags. `EXCERPT_VERSION = 1`.
- `names_a_model`: the longest recipe kind or model word the raw task names, effort words and
  permission flags excluded (question 16 warning).
- `languages`: extensions in the task, at most 8, sorted.
- `build_state` and `questions`: one Noul per gate keyed by recipe id; state carries only
  `task, title, sentence, role, round, project{name,goal}, repo, languages, policy`.
- `request_body`, `prompt_hash` (instructions + criteria + excerpt version), pin `jev-1.13.0`.
- `call`: `/usr/bin/curl --silent --show-error --max-time <s> --config -` through `Runner`; the
  key reaches the child only on stdin; total budget 3 s including one retry; retry only on
  429/529 and curl exit 6/7/35; never retry 401/402/422 or other statuses; the body is stdout
  minus its last line, which is the status from `write-out`.
- `parse_body`: response `model` must equal the pin (`ModelMismatch`), every answer must be a
  Noul object with a number (`Malformed`), `usage.input_tokens` optional.
- `fired_gate`: highest Noul at or over its threshold; a tie returns `None` (default).
- Key: `TYPESAFE_API_KEY`, else `~/.config/typesafe/api_key`; `key_report` carries source, path,
  mode and group/other readability for the doctor; the key value is never logged.
- `models_probe`: the doctor's `GET /v1/models`, the only live call.
- 15 unit tests with the FakeRunner: 200, 429→200, 401 (one attempt), timeout (one attempt),
  connect error → retry, spawn failure (`NoCurl`), malformed, model mismatch, Noul tie, curl
  stdin escaping, key report, transform, languages, state, prompt hash.

### `src/launch.rs` (new, ~1990 lines)

- `parse_picker_config(config_dir, opted_in)` — §2 Config: `[roles] resolver`, `jev_model`,
  `jev_timeout_ms`, `jev_daily_cap`, `floor{sideways,upgrade}`; `[recipes.<id>]`; per role
  `default`, `allowed`, `escalate`, `gates`. Round-one inline D2 roles become `<role>_inline`
  with `default == allowed == ["<role>_inline"]`; an inline row plus a reference is
  `role_form_mixed`; a role without a default is `role_default_missing`. Defaults: `off`,
  `jev-1.13.0`, 3000 ms, 200/day, floors 0.50/0.70. `policy_hash` = SHA-256 over the resolver
  settings, recipes, role lists, gates and thresholds, `jev_model`, the excerpt version and the
  scrub fingerprint (Design norm 8, D11).
- `validate_config` — §2 Validation: kind is in `herdr agent start --help`
  (`recipe_kind_unknown`); claude Opus/Fable must have `--effort high`
  (`recipe_effort_forbidden`); claude/agy need `--dangerously-skip-permissions` (or the bypass
  flag), cursor needs `--force`/`--yolo` (`recipe_permission_missing`); default in allowed,
  every allowed id exists and is enabled, every gate is in allowed, not the default, has a cost
  class, a threshold in (0,1], both criteria and instructions non-empty and only one gate per
  recipe; every recipe `plain` phrase and every rendered reason template passes `plain::check`
  as a birth sentence (`recipe_reason_not_plain`). Roles `pro` and `coordinator` never resolve
  and are skipped; a `[recipes.*]` row of kind `chatgpt` is still refused on the r2 fork.
- `resolve_launch(ctx, project, input)` — §3 steps 1–8, in order: `--recipe` pin
  (`recipe_unknown`, `recipe_not_allowed`; `resolver = pin`, `fallback = override`); PROJECT.md
  pin (`role_args_missing` when the kind changes without args, `recipe_kind_unknown` for an
  unknown kind; `fallback = project`); validation; the model-name warning; the pair filter;
  `off|not_opted_in|single_row|no_gate`; key; the daily cap; one POST; acceptance; the config
  re-read (`config_changed`). No project lock is held across the HTTP call; the caller must not
  hold it either (documented at the module top).
- Pair filter (§2 Pairs): drops every allowed recipe whose `--model`/`model=` value equals the
  sibling's; if it removes the default, the first remaining allowed row in file order is the
  default; none left is `dialogue_same_model`.
- Daily cap: `<project>/.state/jev-calls/<utc-day>.count`, read and incremented under the
  project lock; a spent cap returns `daily_cap` without a POST.
- Acceptance: `http_<code>`, `timeout`, `no_curl`, `no_key`, `malformed`, `model_mismatch`,
  `error` (a gate id missing from `answers`), `not_allowed`, `no_gate`, `daily_cap`,
  `config_changed`; shadow records `jev_pick` and launches the default with `fallback = shadow`.
- Reason templates (fixed, plain-checked at load): picked, default by choice, pinned, fallback,
  shadow; job nouns (lane/reviewer/critic/drafter/research); clauses for the five shipped
  non-default recipes; `compact_reason` for `ade_last` (≤80 characters).
- `doctor_rows` — §5: mode; key source; key file mode; curl `--version`; the live
  `GET /v1/models` only when the resolver is not `off`; table validation; `zsh -lic
  'command -v <exe>'` per enabled kind; the Codex-quota reminder when a `codex_*` recipe is in a
  list; the Astra-enabled note. `picker_doctor` prints them with doctor's three marks.
- 21 unit tests: off, 200 pick, 200 low Noul, 200 other model, 429→200, timeout/401/malformed,
  daily cap, shadow, `--recipe` pin, PROJECT.md pin (including an empty plain and an unknown
  kind), `recipe_not_allowed`, pair filter, validation refusals, inline D2 role, plain templates
  and the compact form, policy hash moves on a threshold change, doctor rows.

### `src/main.rs` and `src/cli.rs` (marked blocks only)

```rust
// ade-picker begin
#[allow(dead_code)]
mod jev;
#[allow(dead_code)]
mod launch;
// ade-picker end
```

and in `cli.rs` a hidden `picker-doctor` verb that calls `crate::launch::picker_doctor(&ctx)`.
The `#[allow(dead_code)]` matches A0's pattern for not-yet-wired modules; the reviewer can drop
it once `threads.rs` and `doctor.rs` call in. The hidden verb can be deleted once A1's `doctor`
prints the rows.

### `tests/picker_plain.rs` (new) and `plain/vocabulary.txt`

`picker`, `web`, `spec`, `engine`, `draft`, `lookup`, `strongest` were added to
`plain/vocabulary.txt` (sorted, lowercase, unique) because the spec's recipe phrases and reason
clauses need them. `tests/picker_plain.rs` compiles A0's real `src/plain.rs` and
`src/contracts.rs` with `#[path]`, checks every day-one recipe phrase, every rendered reason
template and every compact form as a birth sentence, asserts the templates are the ones
`src/launch.rs` writes (drift guard), and proves a bad fixture still fails.

### `skill/PICKER.md` (new)

How a coordinator pins `--recipe` (with examples), what the record stores, the modes, the daily
ceiling and what the picker never does.

## Tick entry point

None. The picker runs at launch time (`resolve_launch`); nothing runs from the ticker.

## Gates

All on `0d3e706` unless stated. Toolchain `cargo +1.89.0`.

### `cargo fmt --check`

Exit 1, only the ten unowned files A0 named (pre-existing wrapping on main; I formatted only my
own files). My files (`src/jev.rs`, `src/launch.rs`, `src/contracts.rs`, `tests/picker_plain.rs`,
my blocks in `main.rs`/`cli.rs`) are clean:

```
Diff in .../src/adopt.rs
Diff in .../src/coordinator.rs
Diff in .../src/doctor.rs
Diff in .../src/inbox.rs
Diff in .../src/lifecycle.rs
Diff in .../src/project.rs
Diff in .../src/remote.rs
Diff in .../src/thread.rs
Diff in .../src/threads.rs
Diff in .../src/ticker.rs
```

Note for the coordinator: while running this lane, one `cargo +1.89.0 fmt -- src/contracts.rs`
invocation reformatted the whole crate in my working tree at 17:17. I restored those ten files
to the branch tip with `git checkout --` and never committed the crate-wide format.

### `cargo clippy --all-targets --locked -- -D warnings`

Exit 0.

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.04s
```

### `cargo test --locked`

Exit 0. Unit 209, `tests/cli.rs` 4, `tests/picker_plain.rs` 29 (including A0's plain and
contracts tests recompiled by `#[path]`).

```
test result: ok. 209 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.59s
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.18s
test result: ok. 29 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.22s
```

The picker's §1.3 scenario tests are covered: 200 pick, 200 low confidence, 200 other model,
429 then 200, timeout, 401, malformed, daily cap.

### `cargo build --release --locked`

Exit 0. Binary `0.1.0+0d3e706.1789768891` at
`/Users/rolfie/projects/herdr-ade/.target/a4/release/herdr-ade`.

```
    Finished `release` profile [optimized] target(s) in 3.71s
```

### Throwaway `doctor`

Throwaway tree `/var/tmp/ade-a4` (`HERDR_ADE_ROOT=/var/tmp/ade-a4/root`,
`XDG_CONFIG_HOME=/var/tmp/ade-a4/xdg`, `XDG_STATE_HOME=/var/tmp/ade-a4/state`,
`HOME=/var/tmp/ade-a4/home`, `HERDR_BIN_PATH` = the fork release build). The config is the
spec's day-one table (`resolver = "shadow"`, 7 recipes, 5 roles). `HA plain check` of every
phrase passes inside `validate_config`.

`picker-doctor` (the picker rows) with the key read from the throwaway key file:

```
[ok  ] picker: resolver is shadow; 7 recipe(s), 5 role(s)
[ok  ] picker key: at /var/tmp/ade-a4/home/.config/typesafe/api_key
[ok  ] picker key mode: 600
[ok  ] curl: curl 8.7.1 (x86_64-apple-darwin26.0) libcurl/8.7.1 (SecureTransport) LibreSSL/3.3.6 …
[ok  ] picker models: GET /v1/models answered 200
[ok  ] picker recipes: 7 recipe(s) valid
[ok  ] kind agy: /Users/rolfie/.local/bin/agy
[ok  ] kind claude: /Users/rolfie/.local/bin/claude
[ok  ] kind codex: /opt/homebrew/bin/codex
[ok  ] kind cursor: /Users/rolfie/.local/bin/cursor-agent
[warn] codex quota: Codex usage is not visible to the plugin; disable the recipe by hand when …
exit=0
```

The plain `doctor` verb also ran in the same tree (exit 1: the fork binary reports `0.9.0`, the
pre-install state; A0's item 7 and SPEC item 42):

```
[FAIL] herdr: 0.9.0 (/Users/rolfie/projects/herdr/.target/install/release/herdr); 0.9.1 or later is required
[ok  ] session: /Users/rolfie/.config/herdr/herdr.sock (name: -)
[ok  ] git: git version 2.54.0 (Apple Git-157)
[ok  ] ssh: OpenSSH_10.3p1, LibreSSL 3.3.6
[ok  ] rsync: rsync  version 3.5.0  protocol version 32
[ok  ] gh: gh version 2.97.0 (2026-07-31)
[warn] gh auth: You are not logged into any GitHub hosts. …
[ok  ] root: 0 project(s)
[warn] ticker: not running
herdr-ade: some checks failed
exit=1
```

Live-call accounting: 4 `GET /v1/models` probes in total (two on `6c89c34`, two on the final
binary; env-key and file-key branches), all HTTP 200. **Zero System One inference calls**, so the
cost for this lane is **$0.00** against the $0.05 cap. The key file was copied into the
throwaway home for the file path, read only by the throwaway doctor, and deleted afterwards;
nothing was written outside `/var/tmp/ade-a4`. The throwaway herdr session `ade-a4` was never
started, so there was nothing to stop. `herdr status server` (read-only) was called once by the
plain `doctor` gate, the same call A0's gate makes.

## Left for the reviewer (exact lines)

1. `src/contracts.rs`, `struct ThreadRecord` line 59 — carry the picker fields on the launch:
   ```rust
   pub launch: Launch,
   ```
   (today `pub launch: LaunchRecipe,`). Every `LaunchRecipe` field keeps its name and meaning, so
   every existing read of `thread.launch.kind/args/env/ready_timeout_ms/policy_hash/attempt/brief_hash`
   still compiles.
2. `src/threads.rs`, in `thread start`, before the worktree and tab exist and **not** holding
   the project lock (SPEC-jev-picker §3 step 5):
   ```rust
   let launch = crate::launch::resolve_launch(
       &ctx,
       &project,
       &crate::launch::ResolveInput {
           role: role.as_deref().unwrap_or("lane"),
           task: &task,
           recipe: recipe.as_deref(),
           project_pin: /* PROJECT.md kind/args pin for this role, else None */ None,
           sibling: None,
           opted_in: /* PROJECT.md `jev = true`, question 13 */ false,
           round: round.as_deref(),
           title: Some(&title),
           sentence: Some(&plain),
           repo: /* repository basename, else None */ None,
           policy: None,
       },
   )?;
   ```
   Then set `launch.brief_hash` after the brief commit (D9) and write the record under the lock.
   The `--agent <kind>` without `--recipe` refusal (`role_args_missing`, decision 13) stays on the
   verb: `resolve_launch` never sees `--agent`.
3. `src/cli.rs`, `ThreadCommand::Start`:
   ```rust
   /// Which allowed set and gates the picker sees (default: lane)
   #[arg(long, value_name = "NAME")]
   role: Option<String>,
   /// Pin this recipe; the picker is skipped
   #[arg(long, value_name = "ID")]
   recipe: Option<String>,
   ```
   and pass both through `StartArgs`.
4. `src/doctor.rs`, in `report` after the tool rows:
   ```rust
   match crate::launch::doctor_rows(ctx) {
       Ok(rows) => {
           for row in rows {
               check(&mut out, row.ok, &row.label, row.detail.clone());
           }
       }
       Err(error) => check(&mut out, Some(false), "picker", format!("{error:#}")),
   }
   ```
   Then delete my hidden `picker-doctor` block in `src/cli.rs` and drop the two
   `#[allow(dead_code)]` lines in `src/main.rs`.
5. `src/ticker.rs` — launch from the record only (`hp:src/ticker.rs:456` and `:593` today read
   live safety args):
   ```rust
   herdr.agent_start(&t.agent_name, &t.launch.kind, &t.pane_id, &t.launch.args)?;
   ```
   and publish `ade_last` from the stored sentence:
   ```rust
   workspace_token("ade_last", &t.launch.compact_reason)
   ```
6. Round and dialogue verbs (A3's modules): `resolve_launch` for `reviewer`, `drafter`, then
   `critic` with `sibling: Some(&drafter_launch)`. `pro` and `coordinator` never call it.
7. PROJECT.md: add the `jev = true` opt-in to `Settings` (question 13, default off) and pass it
   as `opted_in`. Until question 21 is answered, PROJECT.md pins `kind`/`args` only.

## What I could not do

1. Wire the integration into `project.rs`, `threads.rs`, `thread.rs`, `ticker.rs`, `doctor.rs`,
   the round/dialogue verbs and the `--role`/`--recipe` flags: those files are the reviewer's
   per the brief; exact lines are above.
2. Run a real gate pick against live Jev (by design: the brief allows one live call, and that is
   the doctor's `GET /v1/models`; every resolve test uses the FakeRunner). No System One call was
   made, so no behaviour was verified against the live model.
3. Make crate-wide `cargo fmt --check` exit 0 without touching unowned files (pre-existing on
   main; A0 issue 1, SPEC item 36).
4. `XDG_CONFIG_HOME` isolation: `paths.rs` still uses `$HOME/.config/herdr-ade` (A0 issue 4), so
   the throwaway doctor used a throwaway `HOME` and a copied key file instead.
5. The `command -v` row maps every kind to its own name except `cursor` → `cursor-agent`; the
   fork's `--help` does not print the executable per kind.

## Open questions (numbered from 1 for SPEC-ADE §6)

1. `compact_reason` is a new `Launch` field not in SPEC-jev-picker's Output list. I added it so
   the ticker can publish `ade_last` without rereading live config (D2, D17 item 14). Keep it, or
   move the sentence elsewhere?
2. A PROJECT.md pin has no `plain` phrase; I fall back to the role default's phrase ("You chose
   the usual coding helper for this task."). Should front matter carry a `plain` for a pinned
   row instead?
3. A gate's id is its recipe id, so two gates on one recipe are refused (`gate_duplicate`). A
   custom gate recipe without a fixed reason clause gets the default sentence rather than a
   refusal. Acceptable, or should both be config errors?
4. An inline D2 role without `plain` gets the shipped phrase "the usual helper". Should the
   config be refused instead (`recipe_reason_not_plain`)?
5. `resolve_launch` returns `attempt = 1` and an empty `brief_hash`; the caller fills the hash
   after the brief commit. Confirm that ordering with D9.
6. The daily-cap file lives at `<project>/.state/jev-calls/<utc-day>.count`. Is that the right
   home for a per-project, per-UTC-day counter?
7. The hidden `picker-doctor` verb exists only until A1's `doctor` prints `doctor_rows`; delete it
   then?
8. The r2 fork's `herdr agent start --help` has no `chatgpt` or `dsh`. An inline `[roles.pro]`
   with kind `chatgpt` is skipped from kind validation (it never resolves), but a
   `[recipes.<id>]` row with those kinds is refused `recipe_kind_unknown`. Right for D7/D15?
9. `doctor`'s models probe uses a 3 s budget (the spec names none). Keep 3 s?
````

### ade-pi (lane a5, `lane/ade-pi` d6f71088)

````markdown
# Lane a5 report: ade-pi

Final commit sha: `d6f71088461ebc3910071d5acf04adc625e8adcb` (short `d6f7108`)
Branch: `lane/ade-pi`
Worktree: `/Users/rolfie/projects/herdr-ade/.worktrees/a5`
Toolchain: `rustc 1.89.0` (`cargo +1.89.0`), edition 2024,
`CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a5`,
`DEVELOPER_DIR=/Library/Developer/CommandLineTools`.

Final commit: `d6f7108 feat(pi): crate-side runner adapter for check and doctor rows`.

Commits on top of A0's `040f755` (merge of `main`):

1. `4594e01 feat(pi): the pinned pi library for herdr-ade (SPEC-pi v2 §3)`
2. `6be7b03 feat(pi): thin herdr-pi binary with its own [[bin]] entry (SPEC-pi v2 §3)`
3. `f0b7018 feat(pi): register the pi module in the herdr-ade binary`
4. `294c19b docs(pi): what a coordinator types, and the one-time logins (SPEC-pi v2 §2, §3.4)`
5. `d6f7108 feat(pi): crate-side runner adapter for check and doctor rows`

## What I read

- `tasks/ade-pi.md` (this brief).
- SPEC-pi v2 `/Users/rolfie/projects/herdr/tasks/pi/SPEC-pi.md`: §3 whole
  (3.1 to 3.10), §4, §7 acceptance, §8 open questions, fold notes.
- A0's report `.worktrees/a0/.reports/ade-contracts-report.md` and A0's pin
  (`src/contracts.rs`, `src/plain.rs`, FakeRunner wiring, acceptance skeleton).
- SPEC-ADE v3.1 §4.2 (my owned files), §1.2 D1 to D18, §1.3 (FakeRunner
  scenarios), §4.3 (reviewer rows), §6 items 32 to 35.
- `tasks/ade/decisions.md` rows 17:50, 18:05, 18:30, 19:25.
- `tasks/pi/research-a.md` (pin, provider catalog, mock provider recipe),
  `tasks/pi/turns/02-opus-a.md` §3.7 guard prototype.
- The fork asset `/Users/rolfie/projects/herdr/src/integration/assets/pi/herdr-agent-state.ts`
  and the isolated pi 0.85.1 extension docs
  (`/var/tmp/pi-research-a/npm/node_modules/@earendil-works/pi-coding-agent/docs/extensions.md`)
  and type declarations (`dist/core/extensions/types.d.ts`, `dist/core/event-bus.d.ts`,
  `dist/core/exec.d.ts`) for the extension API: `pi.on`, `pi.events.emit`,
  `pi.exec`, `agent_end.messages[].stopReason`/`errorMessage`,
  `after_provider_response.status`, `ui_prompt_start/end`, `input`.

## What I built, per owned file (spec lines)

### `src/pi/mod.rs` (§3.1, §3.2, §3.3)

- Constants: `PI_PACKAGE = @earendil-works/pi-coding-agent`, `PI_VERSION = 0.85.1`,
  `MIN_NODE = 22.19.0`, `GUARD_FILE`, `GUARD_MARKER = herdr-pi-guard:version=1`,
  `HERDR_EXTENSION_FILE`.
- `Env` (home + vars, testable), `Layout` (every path under one root:
  `npm/`, `bin/pi`, `agent/{settings,models,auth,trust}.json`,
  `agent/extensions/`, `agent/sessions/`, `lanes/`).
- `resolve_root`: `HERDR_PLUGIN_STATE_DIR` then `HERDR_ADE_ROOT` then
  `~/.herdr-ade`, plus `/pi` (§3.1 "Shared state lives under ADE's
  `HERDR_PLUGIN_STATE_DIR` at .../pi/").
- `check(provider)` — the exact entry A1 calls (`crate::pi::check`), process
  env + real runner, `Err` carrying every failing row (§3.4 refusal list).
- `login_instructions()` — the one-time login steps per provider, printed by
  the binary; never a login itself (§2).

### `src/pi/sh.rs` (new; the seam, not in the brief's file list)

`src/pi/` compiles into both binaries (see "What I could not do", item 2), so
it cannot use `crate::runner`. `sh` is a small `Cmd`/`Output`/`Runner` seam
and a `FakeRunner` with the same first-match scripted shape as
`crate::runner::fake`; `login_shell` runs `zsh -lic`. Unit-tested
(stdout/stderr/exit, stdin closed, `~` expansion).

### `src/pi/install.rs` (§3.2, §3.4 marker)

- `npm_install_args`: `npm install --prefix <pi/npm> --save-exact --no-fund
  --no-audit @earendil-works/pi-coding-agent@0.85.1`; never `-g`.
- `installed_version`, `is_installed_exactly`, `cli_js_exists`,
  `install` (runs npm, asserts 0.85.1 and `dist/bundle/cli.js`).
- `GUARD_TS = include_str!("../../extensions/herdr-pi-guard.ts")`,
  `write_guard`, `guard_ok` (marker check).
- `link_line` (`ln -s <wrapper> ~/.local/bin/pi`), `setup` (install, folder,
  wrapper, guard, `herdr integration install pi`, link line) — the §3.4
  setup action, `PI_CODING_AGENT_DIR` passed in env only.

### `src/pi/folder.rs` (§3.3)

- The exact `settings.json` (`defaultProjectTrust: "never"`,
  `enableInstallTelemetry: false`, `quietStartup`, `skills.enabled: false`,
  `retry.maxRetries: 1`, `retry.provider.maxRetries: 0`,
  `maxRetryDelayMs: 60000`), `models.json` with empty providers (no Pro),
  `trust.json` empty; `auth.json` is never written by setup.
- `ensure` (idempotent, never clobbers settings), `read_settings`
  (`trust_never`, `skills_disabled`, `telemetry_off`, `retries_capped`),
  `trust_has_true`, `extension_state`, `lane_session_dir` (shared-cwd case).

### `src/pi/launch.rs` (§3.2 wrapper, §3.4 start line)

- `agent_dir_for`: uses `PI_CODING_AGENT_DIR` when set and not under
  `~/.pi`; otherwise the baked path; refuses a dir under `~/.pi` (§3.2).
- `wrapper_script`: refuses `install|remove|uninstall|update|config`,
  finds `node` on the given `PATH`, refuses below 22.19.0, sets
  `PI_SKIP_VERSION_CHECK=1` and `PI_TELEMETRY=0`, `exec node <cli.js> "$@"`.
  `write_wrapper` writes it 0755. Real-run evidence below.
- `start_args` / `agent_start_args` / `agent_start_line`: the exact §3.4
  line, `--no-skills` always, `launch.resume_session` appended only as
  `--session <path>` (§3.8), never inside a recipe.
- `validate_args` (`pi_args_forbidden`: the 13 forbidden flags plus `--force`,
  required `--model`/`--no-skills`), `validate_env` (`pi_env_forbidden`),
  `validate_provider_column`, `validate_thinking` (`deepseek-v4-flash`/`k3`
  no `xhigh`, `grok-4.6` no `max`), `flag_value`.

### `src/pi/resume.rs` (§3.8)

- `restore_line` = `pi --session <path>`; `append_resume_session` refuses a
  session flag in the recipe; `SESSION_PICKING` and `r2_strip_list()`
  (`--session`, `--fork`, `--no-session`, `-c`, ...; no `-s`);
  `session_from_pane_get` reads `result.pane.agent_session.value` (and the
  agent shape tolerantly).

### `src/pi/roles.rs` (§3.4, §3.5, §6.2)

- `pi_recipes()`: `pi_deepseek_flash`, `pi_codex_sol_high` (escalate only),
  `pi_codex_astra_xhigh` (`enabled = false`), `pi_opencode_muse`,
  `pi_kimi_k3`. Each row: `kind = "pi"`, `provider`, `model_family`, the
  four-flag `args`, `env = []`, `ready_timeout_ms = 30000`, `enabled`,
  `start_time_allowed`, and a D17 `plain` birth sentence.
- `withheld_recipes()`: `pi_xai_grok_xhigh` and the OpenCode Grok door,
  withheld per decision 19:25 item 6. No `pi_cursor_*` row; `validate()`
  refuses a Cursor provider/add-on (`pi_cursor_forbidden`).
- `enabled_providers()` for doctor; `is_pi_provider`.

### `src/pi/priming.rs` (§3.6)

- `adapter_row()` with the D15 row verbatim (start, receipt, pre-ready and
  post-ready traps, required, untested, `capability: unqualified`),
  `prime_line(role)` = `ha skill <role>`.

### `src/pi/limits.rs` (§3.7, §3.10)

- `LimitClass` (`limit|login|unreachable|error`) and `classify` (429 and
  billing/quota words; 401/403 and credentials/token words; refused/ENOTFOUND/
  timeout words), `waiting_line` (`WAITING <lane> <provider> <class>: <first
  120>`), `blocked_label` (with HTTP status), `Throttle` (once per class per
  10 minutes), `rate_limit_table()` for §3.10.

### `src/pi/doctor.rs` (§3.4, §3.9)

- `Row`/`Level`/`line()` (`[ok  ]`, `[warn]`, `[FAIL]` shape).
- `doctor_rows_with` runs the §3.9 table: node (`zsh -lic`), npm, wrapper
  `pi --version` = 0.85.1, prefix `package.json` exact pin (not `^0.85.1`),
  wrapper on the login PATH and no earlier `pi`, prefix isolation vs
  `npm root -g`, pi folder, settings trust/skills/retry, `trust.json`,
  `herdr integration status` = `pi: current`, guard marker, Cursor
  informational (fails on `@cursor/sdk`/`pi-cursor*`/`herdr-pi-cursor.ts`),
  one row per enabled provider (`pi auth check --provider <p> --json
  --no-refresh`, stdin closed), `~/.codex` bridge URL (fails, pro-bridge
  risk 2), `~/.pi` existence only.
- `doctor_rows()` (process env; what A1 wires). `check_report` returns the
  same checks as JSON (`ok`, `provider`, `checks[]`), `check_with` refuses
  with all failing rows.
- Note: SPEC-pi writes `zsh -lic 'command -v -a pi'`; that is a bash form and
  zsh rejects it (`zsh: command not found: -v`, verified on this Mac). The row
  uses `whence -va pi`, zsh's own list form. See open question 1.

### `src/pi/ade.rs` (crate-side seam; registered only in `herdr-ade`)

- `Adapter` implements `pi::sh::Runner` over `crate::runner::Runner` (the
  real runner or a `FakeRunner`), `check_with`, `doctor_rows_with`,
  `process_prefix`. This is the seam for A1's scripted tests. Unit-tested
  through the plugin's own `FakeRunner`.

### `src/pi/scenarios.rs` (test-only; §1.3, SPEC-pi §7)

FakeRunner scenarios, named `scenario_*`:
`scenario_start_is_the_spec_line_and_never_a_trust_flag`,
`scenario_restart_uses_the_reported_session_and_the_recipe_stays_clean`,
`scenario_the_guard_reports_a_429_as_waiting_once_and_never_done`,
`scenario_the_guard_reports_a_401_as_login_and_a_dead_endpoint_as_unreachable`,
`scenario_setup_then_check_for_deepseek`,
`scenario_check_refuses_a_missing_login_before_any_start`.

### `extensions/herdr-pi-guard.ts` (§3.7)

Marker `herdr-pi-guard:version=1` on line 1. `agent_end` remembers the last
assistant message when `stopReason == "error"`; `after_provider_response`
keeps the HTTP status; `agent_settled` classifies, emits
`pi.events.emit("herdr:blocked", { active: true, label })` once, and runs
`ha waiting "<provider> <class>: <first 120>"` through `pi.exec` at most once
per class per 10 minutes when `HERDR_ADE_LAUNCH` is set. `agent_start` and
`input` clear it; `ui_prompt_start`/`ui_prompt_end` raise/clear a "waiting for
you" reason. When `ha` is not on `PATH` it falls back to
`herdr agent list --json` + `agent prompt <parent>`. Never `ha done`, never a
retry of its own.

### `src/bin/herdr-pi.rs` (§3)

`setup`, `login [provider]` (per-provider on-screen steps; opens pi on the
shared folder only with a TTY; never types `/login`), `doctor`, `check
<provider>` (read-only JSON, exit 1 when not ready). Compiles `src/pi/`
through `#[path = "../pi/mod.rs"]` because the brief allows only the one
`[[bin]]` entry and no library target.

### `skill/PI.md` (§3.4, §3.7, §3.8)

What a coordinator types, the row table, the one-time logins, the check verb,
the guard classes, blocked recovery (`herdr pane send-text` + Enter, never
`agent prompt`), what a restart needs, and the never list.

### `src/pi/testdata/` (test-only fixtures)

`mock-provider.js` (429 / 401 / ok) and `guard-check.sh`, the real run of
the shipping guard against the isolated pi (see gate 6).

## Contract fields added

None. `src/contracts.rs` is untouched. A4 owns `Recipe` (its
`contracts(ade-picker)` commit); my `roles::PiRecipe` is a standalone row
type A4 maps into its table, with the fields its brief lists (`kind`, `args`,
`env`, `ready_timeout_ms`, `provider`, `enabled`, plus `model_family`,
`start_time_allowed`, `plain`).

## Ticker entry point

None. SPEC-pi v2 gives the pi library no per-tick work: the guard is
event-driven inside the pi process, and the pre-launch check runs at launch.
For the same reason I cannot write the exact
`pub fn tick(t: &mut crate::ticker::Ticker)` on this branch: `Ticker` does not
exist yet (A1 owns `src/ticker.rs`), and `src/pi/` also compiles into
`herdr-pi`, where `crate::ticker` cannot exist. If the round wants a call
site anyway, add the function to `src/pi/ade.rs` (crate-only) and wire it in
A1's ticker as `crate::pi_ade::tick(&mut ticker)?;`.

## Gates

All with `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a5`,
`DEVELOPER_DIR=/Library/Developer/CommandLineTools`, `cargo +1.89.0`.

### 1. `cargo fmt --check` — exit 1, pre-existing only

248 diffs, all in files I do not own (same set as `main`): `src/adopt.rs`,
`src/coordinator.rs`, `src/doctor.rs`, `src/inbox.rs`, `src/lifecycle.rs`,
`src/project.rs`, `src/remote.rs`, `src/thread.rs`, `src/threads.rs`,
`src/ticker.rs`. Every file I own, `src/main.rs`'s marked block and both
binary roots pass:

```
rustup run 1.89.0 rustfmt --edition 2024 --check --config skip_children=true \
  src/pi/{mod,sh,folder,install,launch,resume,roles,priming,limits,doctor,scenarios,ade}.rs \
  src/bin/herdr-pi.rs src/main.rs
exit 0 (no output)
```

Tail of `cargo fmt --check`:

```
Diff in .../src/ticker.rs:859:
     #[test]
     fn log_is_capped() {
...
248
src/adopt.rs src/coordinator.rs src/doctor.rs src/inbox.rs src/lifecycle.rs src/project.rs src/remote.rs src/thread.rs src/threads.rs src/ticker.rs
```

### 2. `cargo clippy --all-targets --locked -- -D warnings` — exit 0

```
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.05s
```

### 3. `cargo test --locked` — exit 0

```
     Running unittests src/main.rs (.../herdr_ade-64f5f4c1db523e9f)
test result: ok. 221 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 3.31s
     Running unittests src/bin/herdr-pi.rs (.../herdr_pi-f58efabeeeb0ebaa)
test result: ok. 51 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.03s
     Running tests/cli.rs (.../cli-26950f38ca2bebef)
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.16s
```

169 pre-existing plus my 52 (51 under `pi` and 1 in `pi_ade`; the pi module
also compiles into the `herdr-pi` target, where the same 51 run again).

### 4. `cargo build --release --locked` — exit 0

```
    Finished `release` profile [optimized] target(s) in 0.02s
```

Binaries: `.target/a5/release/herdr-ade`, `.target/a5/release/herdr-pi`.

### 5. Throwaway `herdr-pi setup` + `doctor` + `check`

Isolation: `HOME=/var/tmp/ade-a5/home`,
`HERDR_ADE_ROOT=/var/tmp/ade-a5/prefix`, `XDG_CONFIG_HOME=/var/tmp/ade-a5/xdg`,
`XDG_STATE_HOME=/var/tmp/ade-a5/state`, `HERDR_SESSION=ade-a5`,
`HERDR_BIN_PATH=/Users/rolfie/projects/herdr/.target/install/release/herdr`
(the fork's release build; no live server was contacted). The fake HOME's
`.local/bin/pi` symlink points at the throwaway wrapper. Nothing was written
to `~/.pi`, `~/.codex`, `~/.herdr-ade` or `~/.local`; the fake HOME has no
`.pi` (verified).

`setup` (a real `npm install`, only under `/var/tmp/ade-a5/prefix`):

```
installed @earendil-works/pi-coding-agent@0.85.1 into /var/tmp/ade-a5/prefix/pi/npm (`npm install --prefix /var/tmp/ade-a5/prefix/pi/npm --save-exact --no-fund --no-audit @earendil-works/pi-coding-agent@0.85.1`)
wrote the shared pi folder /var/tmp/ade-a5/prefix/pi/agent (settings.json, models.json, trust.json)
wrote the wrapper /var/tmp/ade-a5/prefix/pi/bin/pi
wrote the guard /var/tmp/ade-a5/prefix/pi/agent/extensions/herdr-pi-guard.ts
installed the herdr state hook into /var/tmp/ade-a5/prefix/pi/agent/extensions
...
ln -s /var/tmp/ade-a5/prefix/pi/bin/pi /var/tmp/ade-a5/home/.local/bin/pi
```

`doctor` tail (exit 1: only the four provider login rows fail, which is the
fail-closed contract):

```
[ok  ] node: v24.19.0
[ok  ] npm: .../fnm_multishells/.../npm
[ok  ] pi version: wrapper 0.85.1
[ok  ] pin: 0.85.1 in /var/tmp/ade-a5/prefix/pi/npm/node_modules/@earendil-works/pi-coding-agent/package.json
[ok  ] wrapper on PATH: /var/tmp/ade-a5/home/.local/bin/pi -> /private/var/tmp/ade-a5/prefix/pi/bin/pi
[ok  ] prefix: /var/tmp/ade-a5/prefix/pi/npm (global npm root: .../node_modules)
[ok  ] pi folder: /var/tmp/ade-a5/prefix/pi/agent
[ok  ] settings trust: defaultProjectTrust: never
[ok  ] settings skills: skills.enabled: false
[ok  ] trust.json: no true entry
[ok  ] herdr extension: pi: current
[ok  ] guard: .../herdr-pi-guard.ts with marker
[ok  ] Cursor: outside pi; native cursor lanes only, then retired
[FAIL] provider deepseek: missing login: credentials_not_configured (run `herdr-pi login deepseek`)
[FAIL] provider openai-codex: missing login: credentials_not_configured ...
[FAIL] provider opencode: missing login: credentials_not_configured ...
[FAIL] provider kimi-coding: missing login: credentials_not_configured ...
[ok  ] ~/.codex: no config.toml
[ok  ] ~/.pi: not present
herdr-pi: some checks failed
```

`check deepseek` (exit 1, JSON tail) and `check cursor` (exit 1):

```
  "check": "login",
  "detail": "missing login: credentials_not_configured (run `herdr-pi login deepseek`)",
--- cursor ---
  "check": "provider",
  "detail": "pi_cursor_forbidden: Cursor stays outside pi (decision 18:30)",
```

Wrapper behaviour (real run):

```
$ /var/tmp/ade-a5/prefix/pi/bin/pi --version
0.85.1                                                  (exit 0)
$ /var/tmp/ade-a5/prefix/pi/bin/pi install foo
herdr-ade pi: 'install' is refused; run herdr-pi setup, which owns the pinned install   (exit 2)
$ PI_CODING_AGENT_DIR=~/.pi/agent .../bin/pi --version
herdr-ade pi: refusing a config dir under ~/.pi; use PI_CODING_AGENT_DIR elsewhere      (exit 2)
$ PI_CODING_AGENT_DIR=/var/tmp/ade-a5/other .../bin/pi --version
0.85.1                                                  (exit 0)
```

The pinned prefix `package.json` holds the exact range (not a caret):

```
{"dependencies":{"@earendil-works/pi-coding-agent":"0.85.1"}}
```

### 6. Guard against the isolated pi 0.85.1 (real run, T7)

`sh src/pi/testdata/guard-check.sh` with
`/var/tmp/pi-research-a/npm/node_modules/@earendil-works/pi-coding-agent/dist/bundle/cli.js`,
`PI_CODING_AGENT_DIR=/var/tmp/ade-a5/guard-test/agent` (guard copied in, mock
`models.json`), a fake `ha` on `PATH`, `HERDR_ADE_LAUNCH` set. Exit 0:

```
PASS T7a 429 becomes blocked + WAITING limit: mock hits 2, ha waiting once (class limit)
PASS T7b 401 becomes blocked + WAITING login: mock hits 1, ha waiting once (class login)
GUARD PASS
```

Recorded `ha waiting` lines (two hits on the 429, one on the 401: pi's v2
retry cap is visible):

```
waiting mock-provider limit: 429: {"message":"Rate limit reached for mock-model","type":"rate_limit_error"}
waiting mock-provider login: 401: {"message":"Incorrect API key provided: mock-key","type":"invalid_request_error"}
```

The pane `blocked` half of T7 needs a herdr session and is the reviewer's row
(the emit is the same `herdr:blocked` channel the herdr asset consumes).

## Left for the reviewer (exact lines)

### A1's `src/doctor.rs` (pi rows)

```rust
    // before `(out, healthy)`:
    match crate::pi::doctor::doctor_rows() {
        Ok((rows, pi_healthy)) => {
            healthy &= pi_healthy;
            for row in rows {
                let _ = writeln!(out, "{}", row.line());
            }
        }
        Err(error) => check(&mut out, Some(false), "pi", format!("{error:#}")),
    }
```

For scripted tests A1 passes its runner through
`crate::pi_ade::doctor_rows_with(runner)` (same rows, `crate::runner::Runner`).

### A1's launch refusal and process check (`src/threads.rs`, `src/herdr.rs`)

```rust
// before `herdr agent start` for a `kind = "pi"` row (provider = the row's column):
crate::pi::check(provider)?;                     // every failing §3.4 row in one refusal
// or, scripted: crate::pi_ade::check_with(ctx.runner, provider)?;

// after start, from `herdr pane process-info <pane>`:
let prefix = crate::pi_ade::process_prefix()?;   // .../pi/npm
if !process.path.starts_with(&prefix.display().to_string()) {
    anyhow::bail!("refused: the pane's pi process is not under {}", prefix.display());
}
```

### A2's adapter table (`src/adapters.rs`)

```rust
let pi_row = crate::pi::priming::adapter_row();  // D15 row, kind "pi"
```

The guard may run `ha waiting`; it never runs `ha done`. Blocked recovery is
`herdr pane send-text` + `send-keys enter`, never `herdr agent prompt`.

### A4's picker table (`src/launch.rs` / `src/project.rs`)

```rust
for row in crate::pi::roles::pi_recipes() { /* recipe_id = row.id, provider = row.provider,
    model_family, args, env, ready_timeout_ms, enabled, plain */ }
let withheld = crate::pi::roles::withheld_recipes();   // pi_xai_grok_xhigh, OpenCode Grok
```

No `pi_cursor_*`; `cursor_grok_xhigh` stays native. `provider` must equal
`--provider` (`crate::pi::launch::validate_provider_column`).

### `herdr-plugin.toml` actions (manifest owner; SPEC-pi §3)

```toml
[[actions]]
id = "pi-doctor"
title = "Pi: check setup"
contexts = ["workspace"]
command = ["target/release/herdr-pi", "doctor"]

[[actions]]
id = "pi-setup"
title = "Pi: install the pinned pi"
contexts = ["workspace"]
command = ["target/release/herdr-pi", "setup"]

[[actions]]
id = "pi-login"
title = "Pi: open a login pane"
contexts = ["workspace"]
command = ["target/release/herdr-pi", "login"]
```

### A3's `scripts/acceptance` (§7 rows, NOT-RUN today)

Suggested lines for `run` (A0's `row` shape; pi kind rows are NOT-RUN until
the coding lane moves, deterministic rows are required):

```sh
row pi-T1 required "herdr-pi doctor: pin exactly 0.85.1, pi: current, prefix isolated, wrapper the only pi on the login PATH, trust never"
row pi-T2 required "herdr-pi check refuses: missing guard, missing herdr extension, trust not never, wrapper not first on PATH, provider not ready"
row pi-T3 required "mock lane in a repo with .pi/extensions: no trust question, kind pi idle at prompt, repo extension did not run"
row pi-T7 required "guard: mock 429 and mock 401 -> pane blocked + one WAITING <lane> <provider> limit|login, typing clears, no done"
row pi-T9 required "no global npm: zsh -lic 'npm root -g' does not contain @earendil-works/pi-coding-agent"
row pi-T10 required "wrong pi earlier on PATH: doctor fails and start refuses (process-info prefix)"
row pi-T11 required "missing login: start refused by pi auth check, never a lane that waits for its first prompt"
row pi-T13 required "no Cursor route: no @cursor/sdk, no pi-cursor*, doctor informational, --provider cursor refused"
row pi-T4 optional "one live lane per enabled provider; priming ha skill records bootstrap acknowledged"
row pi-T5 optional "ha done from a pi lane: DONE line, report hashes, clean tree"
row pi-T6a optional "live handoff: pi pid unchanged, no resume typed"
row pi-T6b optional "cold restore: pi --session through ~/.local/bin/pi, same session file, agent_session reported"
row pi-T8 optional "after r2: exactly one --session, no --approve, no -c"
row pi-T12 optional "two lanes on one OAuth provider with an expired token: both keep working; one auth.json.lock path"
row pi-T14 optional "not a pi gate: Pro pane restore with its -c args, after r2's args replay"
```

Exact commands for the deterministic rows, all on a throwaway with the
isolated env from gate 5 (the T3/T7 rows need the throwaway herdr session and
the mock provider `src/pi/testdata/mock-provider.js`):

- T1: `herdr-pi doctor` (expect exit 1 only while no login exists).
- T2: delete `agent/extensions/herdr-pi-guard.ts` → `herdr-pi check deepseek`
  exit 1 `guard`; set `settings.json` trust `ask` → `settings trust`;
  put `~/.local/bin/pi` after another `pi` → `wrapper on PATH`;
  a fresh root → `login` row.
- T3: `herdr agent start <name> --kind pi --pane <pane> --parent <coord> --
  --provider mock-provider --model mock-model --thinking low --no-skills`
  with the mock `models.json` and `mock-provider.js ok <port>`; assert no
  trust screen and no marker side effect.
- T7: `sh src/pi/testdata/guard-check.sh` (proves the two `ha waiting`
  lines); the pane `blocked` and the clear-on-typing half needs the herdr
  session.
- T9: `zsh -lic 'npm root -g'` from the isolated env.
- T10: `PATH=<earlier-pi-dir>:$PATH herdr-pi doctor` and a start attempt.
- T11: `herdr-pi check deepseek` on a root with no `auth.json`.
- T13: `herdr-pi check cursor` (exit 1 `pi_cursor_forbidden`) and a
  `node_modules` scan.

### r2 strip list (SPEC-pi §3.8)

When `review/durable-r2` merges, drop for pi: `--session <v>`, `--fork <v>`,
`--no-session`, and treat `-c`/`--continue`/`-r`/`--resume` as boolean flags.
Pi has no `-s`. Do not require replay of `--provider` / `--model` /
`--thinking`; never replay `--approve`. `crate::pi::resume::r2_strip_list()`
holds the exact tokens.

### Seam status

`src/cli.rs` untouched (no `herdr-ade` subcommand is needed; the thin binary
covers setup/login/doctor/check). `src/main.rs` has only the `ade-pi begin/end`
block (five lines): `mod pi`, `#[path = "pi/ade.rs"] mod pi_ade`. The
`[[bin]]` entry is the only `Cargo.toml` change.

## Fork must ship (not my files, SPEC-pi §4)

- **Required for v2: nothing on master.** The wrapper plus the one shared
  folder cover cold restore; model and thinking come from the session file.
- **Should ship (defense, not a start gate):** a `blocked` screen rule in
  `src/detect/manifests/pi.toml` for `Trust project folder?` and the
  missing-session-folder `Continue / Cancel` question (they render before the
  extension reports). The settings file stays the control.
- **On `review/durable-r2` when it merges:** `persisted_session_from_launch_args`
  for `Agent::Pi`; `--session`, `--fork` (and `--no-session`) in
  `strip_session_picking_args`, with pi's `-c` as a boolean and no `-s`.
- **Later, shared with pro-bridge:** persist tab `--env` on restore so
  `HERDR_ADE_LAUNCH` survives a server bounce (not needed for
  `PI_CODING_AGENT_DIR` once the wrapper supplies the folder).

Plugin-side, this lane ships: the pinned prefix, the wrapper, setup/login/
doctor/check actions, the shared folder with `settings.json`, the herdr
integration install into it, the guard extension, the D2 rows, the D15 row,
and `crate::pi::check` for the pre-launch refusal.

## What I could not do and why

1. **Run T4, T5, T6a, T6b, T8, T12, T14.** No provider login exists (no
   `/login` was ever typed, by the rules), no herdr session was started for a
   pi lane by this lane, and `review/durable-r2` is not on this branch. These
   are the reviewer's rows; exact lines above.
2. **Expose `pub fn tick(t: &mut crate::ticker::Ticker)`.** `Ticker` does not
   exist on this branch (A1 owns `src/ticker.rs`), and `src/pi/` also compiles
   into `herdr-pi`, where `crate::ticker` cannot exist. There is no per-tick
   pi work in SPEC-pi v2. A crate-side home for it is `src/pi/ade.rs`; the
   wiring line is in "Left for the reviewer".
3. **Add the three pi actions to `herdr-plugin.toml`.** The manifest is not
   in my owned files; the exact TOML is in "Left for the reviewer".
4. **A scripted `doctor_rows` for A1's unit tests.** Provided through
   `crate::pi_ade::doctor_rows_with(runner)`; A1 must call that (its own
   `Runner`) instead of `crate::pi::doctor::doctor_rows()`.
5. **The literal spec probe `command -v -a pi`.** It is not zsh syntax; see
   open question 1 and the `whence -va` note.
6. **Pane `blocked` verification (T7's herdr half), the trust-dialog absence
   (T3) and cold restore (T6b).** They need the throwaway herdr session and
   the reviewer's script; the guard's `ha waiting` half is proven on the real
   isolated pi (gate 6).

## Open questions (numbered from 1 for SPEC-ADE §6)

1. SPEC-pi §3.2/§3.9 write `zsh -lic 'command -v -a pi'`. On this Mac zsh
   answers `command not found: -v` (the `-a` flag is a bash form). I
   implemented the zsh form `whence -va pi` and kept the same semantics
   (first resolution must be `~/.local/bin/pi`, no earlier `pi`). Should the
   spec text change to `whence -va pi`?
2. SPEC-pi question 13 / §3.2 makes `~/.local/bin/pi` always use the harness
   folder, with no `HERDR_ENV` gate (the v1 draft had one). My wrapper follows
   v2: a plain-terminal `pi` uses the harness folder and never `~/.pi`.
   Rolf's 19:25 item 2 reading ("one pi on this Mac, the harness pi") accepts
   this; confirm the no-gate reading.
3. `herdr-pi doctor` fails while any enabled provider has no login. On a
   fresh install that means doctor exits 1 until Rolf's first `/login`. Is
   that the wanted fail-closed reading, or should provider rows be `warn`
   until the roles table actually enables that provider?
4. The guard's `ha`-missing fallback reads `tokens.parent` from
   `herdr agent list --json`. The `tokens` field lands with A1's r2 fork
   (`src/api/schema/agents.rs:208`); on today's 0.9.0 the fallback silently
   does nothing. Acceptable until install day?
5. `pi_opencode_muse` uses model id `muse-spark-1.3` (research a / SPEC-pi
   §3.4). SPEC-pi §3.5 says "when you name the installed id". Is
   `muse-spark-1.3` the installed OpenCode Zen id, or should A4 ship the row
   without an id until Rolf names it?
6. Decisions 19:25 item 6 says workers skip Grok until Rolf says. SPEC-pi
   §3.4/§3.5 still list the OpenCode Grok row and the optional xAI row. I
   shipped no Grok row and listed both as withheld. Confirm that reading.
````
