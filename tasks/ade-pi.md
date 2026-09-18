# Package ade-pi — lane a5

Plain: pi is the one program through which every subscription except Claude and agy runs
from now on (ChatGPT's coding models, OpenCode, DeepSeek, Kimi). This lane builds the pi
part of the harness plugin: a pinned pi install the plugin owns, one shared settings
folder and login file for every pi worker, the start line a coordinator types, the helper
inside pi that reports a usage limit or a dead login as "stuck" instead of "finished", a
launcher so a worker comes back after a herdr restart, and a small setup and check program.

Start line (Rolf starts OpenCode by hand in the a5 tab, pane `w1F:p13`; the coordinator
names it `a5` and links it; for the record):
`herdr agent start a5 --kind opencode --pane w1F:p13 --parent w1F:p1 -- --model opencode-go/deepseek-v4.1-flash`

## Spec

`/Users/rolfie/projects/herdr/tasks/pi/SPEC-pi.md` v2 (read only), §3 whole (3.1 what it
owns, 3.2 pinned install, 3.3 one shared pi folder and one worktree per lane, 3.4 start
line, 3.5 roles rows and Jev recipe ids, 3.6 priming, 3.7 DONE, 3.8 session resume and the
`pi` launcher, 3.9 doctor, 3.10 rate limits), §4 (what the fork ships versus the plugin:
the fork parts are NOT yours; list them in the report under "fork must ship"), §7
acceptance (you make the rows runnable; the reviewer runs them), §8 open questions
(Rolf's answers pending; ship the fold's defaults: trust never, no personal skills, one
shared folder, `resolver`-independent). Decisions: `tasks/ade/decisions.md` rows 17:50,
18:05, 18:30 (Cursor stays outside pi; Pro stays a Codex worker; OpenCode runs under pi).
The research behind it: `tasks/pi/research-a.md`, `research-b.md`; the checks
`tasks/pi/turns/02-opus-a.md`, `02-opus-b.md`. The isolated pi 0.85.1 for reading and
throwaway runs: `/var/tmp/pi-research-a/npm/node_modules/.bin/pi` with
`PI_CODING_AGENT_DIR=<your own dir under /var/tmp/ade-a5/>`; never `~/.pi` (read only),
no logins, no provider requests, no global npm install; a pinned install goes only under
`/var/tmp/ade-a5/prefix` in tests.

## Owned files (additive; SPEC-pi v2 §3)

New module `src/pi/` (`mod.rs`, `install.rs` §3.2: the pinned `@earendil-works/pi-coding-agent@0.85.1`
install into the plugin prefix with version check, never global; `folder.rs` §3.3: the shared
pi folder under the plugin root with `settings.json` (`defaultProjectTrust: "never"`, no
personal skills, no `pi install`), one `auth.json`, one `models.json`, the per-lane
worktree; `launch.rs` §3.4 and §3.8: the start line builder (kind `pi`, provider, model,
thinking level, `--session`) and the `pi` launcher script the plugin writes so a herdr
restart brings a lane back with its folder; `roles.rs` §3.5: the roles-table rows and Jev
recipe ids for pi (a `pub fn pi_recipes()` for lane A4's table and A1's `project.rs`);
`priming.rs` §3.6: what A2's adapter table needs for kind `pi` as `pub fn adapter_row()`;
`resume.rs` §3.8; `doctor.rs` §3.9 as `pub fn doctor_rows()` for A1's doctor; `limits.rs`
§3.10), the guard extension `extensions/herdr-pi-guard.ts` (reports a rate limit, a login
failure or a dead endpoint through the herdr state extension as blocked/WAITING, never idle;
tested against the isolated pi with a mock provider returning 429 and 401), the thin
binary `src/bin/herdr-pi.rs` (setup, the on-screen login instructions per provider, doctor;
never runs a login itself), `skill/PI.md` (what a coordinator types, the one-time login per
provider Rolf does), FakeRunner scenarios for start, restart and the guard, and the §7
acceptance rows as NOT-RUN entries handed to A3's `scripts/acceptance` through "Left for
the reviewer". `Cargo.toml`: only the `[[bin]]` entry for `herdr-pi`, in its own commit.

## Setup and rules (same for every lane of this round)

- Repo `/Users/rolfie/projects/herdr-ade` (fork of eliasstravik/herdr-projects; `origin` =
  uguryildirim24/herdr-ade). Your worktree `/Users/rolfie/projects/herdr-ade/.worktrees/a5`,
  branch `lane/ade-pi`, cut from A0's pin `20fd9e9c` (`lane/ade-contracts`: the rename to
  `herdr-ade`, `src/contracts.rs`, `src/plain.rs` with `plain/words.txt` and
  `plain/vocabulary.txt`, FakeRunner wiring in `src/runner.rs`/`src/scenarios.rs`, the
  `scripts/acceptance` skeleton). Read A0's report first:
  `/Users/rolfie/projects/herdr-ade/.worktrees/a0/.reports/ade-contracts-report.md`.
- Build: `CARGO_TARGET_DIR=/Users/rolfie/projects/herdr-ade/.target/a5`,
  `DEVELOPER_DIR=/Library/Developer/CommandLineTools`, Rust 1.89, edition 2024.
- Never: `herdr plugin link` or `install`, `cargo install`, anything under `~/.config`,
  `~/.herdr-ade`, `~/.herdr-projects`, `~/.claude`, `~/.codex`, `~/.cursor`, the live herdr
  server (session `default`, socket in `~/.config/herdr`), another worktree, `git stash`,
  `git push`. Throwaway runs: `HERDR_ADE_ROOT=/var/tmp/ade-a5/root`,
  `XDG_CONFIG_HOME=/var/tmp/ade-a5/xdg`, `XDG_STATE_HOME=/var/tmp/ade-a5/state`, a throwaway
  herdr session `env -u CLAUDE_CODE_CHILD_SESSION /Users/rolfie/projects/herdr/.target/install/release/herdr --session ade-a5 server`
  (the fork's release build, 0.9.1-based, has `--parent` on `agent start`; stop it with
  `herdr --session ade-a5 session stop ade-a5` when done; never `herdr server stop`).
- Spec (read only): `/Users/rolfie/projects/herdr/tasks/SPEC-ADE.md` v3.1. §4.2 names your
  lane's owned files; §1.2 D1 to D18 the behaviour; §1.3 the FakeRunner scenarios; §4.3 the
  reviewer's acceptance rows (you do not run those; you make them runnable); §6 items 32
  to 35 the fixtures that are tests. Decisions: `/Users/rolfie/projects/herdr/tasks/ade/decisions.md`.
  Base plugin line references use `hp:` = `/Users/rolfie/projects/herdr-projects` at a4cdb0a.
- Seams, so four lanes merge without conflicts: you edit only your owned files. The two
  shared files take one marked block per lane and nothing else: in `src/main.rs` your `mod`
  lines between `// ade-pi begin` and `// ade-pi end`; in `src/cli.rs` your subcommand enum
  variants and match arms inside the same markers. A field you need in `src/contracts.rs`
  goes in its own commit `contracts(ade-pi): <field>` that only adds (new fields carry
  `#[serde(default)]`; never rename or remove). Ticker passes: expose
  `pub fn tick(t: &mut crate::ticker::Ticker) -> anyhow::Result<()>` in your module and name
  it in your report; the round's reviewer wires the call into A1's `ticker.rs`. Nothing
  else crosses lanes; what you cannot do without another lane's code goes in the report
  under "Left for the reviewer", with the exact line you would have written.
- Gates (all before DONE; paste each command and its last lines in the report):
  `cargo fmt --check` (pre-existing wrapping on unowned files fails on `main`; format only
  what you own and say so), `cargo clippy --all-targets --locked -- -D warnings`,
  `cargo test --locked` (your §1.3 scenarios included), `cargo build --release --locked`,
  and the throwaway `doctor`. Commits small and named per package. Never claim a check you
  did not run.

## Report and closing steps (verbatim, every turn ends with one of these, also on failure)

Write `.reports/ade-pi-report.md` (git-ignored) in your worktree: what you read, what you
built per owned file with spec lines, your `tick` entry point if any, contract fields you
added, each gate with its output tail, "Left for the reviewer" with exact lines, what you
could not do and why, open questions numbered from 1 for SPEC-ADE §6, the final commit sha.

```
herdr pane report-metadata $HERDR_PANE_ID --source lane --token lane=ade-pi --token done=1
herdr agent prompt hcoord "DONE ade-pi .reports/ade-pi-report.md <final commit sha>" || herdr agent prompt hcoord "DONE ade-pi .reports/ade-pi-report.md <final commit sha>"
```

If you must stop: `herdr agent prompt hcoord "WAITING ade-pi <what>"`.
A turn that ends without one of these pushes is the one failure nothing catches.
