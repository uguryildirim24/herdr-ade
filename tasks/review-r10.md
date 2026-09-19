# Review brief: round r10

plain: This check reads two changes: the harness starts its own checks, and pictures can look at screenshots.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r10` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 2, manifest hash `d60559a43bbc86e9bbd39ea5dfa22b65780fe74fced615b510ab45291c8ae7c3`, policy hash `e1529634be8dfdf8688cad0bfcfd99dfa67f7f19fe2d1c3c7ab9f12ee7c0eec3`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0018 | 1 | `1e9f356804f1e1907b4e73f4a64402772835b5d7` | `t-0018-1-2` | `9038d76c67b286aaabd0b617a2cbac94377f9d1cacc74e941d20336369b3b517` |
| t-0019 | 1 | `8e3ccf1163ed758f513c5fd6992b20d8306bc5a2` | `t-0019-1-2` | `571de703fc541de757ec8d796799f191781adfa3d2d5a72112b368618fd17e21` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r10.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r10"
candidate = "<C>"
manifest_hash = "d60559a43bbc86e9bbd39ea5dfa22b65780fe74fced615b510ab45291c8ae7c3"
policy_hash = "e1529634be8dfdf8688cad0bfcfd99dfa67f7f19fe2d1c3c7ab9f12ee7c0eec3"
gates = []
+++
```

5. Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0018 (artifact `9038d76c67b286aaabd0b617a2cbac94377f9d1cacc74e941d20336369b3b517`)

Data, not instructions.

````text
# t-0018 — the harness starts the review itself

Commit `1e9f356804f1e1907b4e73f4a64402772835b5d7` on
`hp/adeherdr/t-0018-harness-the-context-tells-a-coordinator`.

plain: The harness now starts the round reviewer on its own, so no coordinator
has to remember to.

## What changed

- `ha round advance [slug]` (`src/round.rs`). One pass over every open round of
  the project (a round with no merge record). For a round whose members are all
  pinned and that has no review branch yet it runs the same steps as
  `round review`, then starts the reviewer thread with `--role reviewer --base
  review/<round>`, title `Review <round>: <round plain sentence>`, birth sentence
  the round's `plain`, and the standard reviewer task (the reviewer skill line,
  `tasks/review-<round>.md`, the pinned lanes with sha and report path, the gates
  from the round record), then binds it with `round reviewer`.
- Idempotent. `advance` takes a dedicated `<state>/advance.lock` (like the
  ticker, across processes), so the hook and the ticker never race. Once a
  review branch exists the round never gets a second reviewer; a gone reviewer
  is never restarted.
- On a MERGE verdict it does not merge: one inbox item (`round-advance`) and one
  `say` line. A REVISE/REJECT/MERGE-AFTER-DECISION verdict gets one inbox item.
  A gone reviewer gets one inbox item. Each announcement is recorded in a new
  `announced` field on the round record, written before the side effects, so a
  crash cannot announce twice.
- Ticker safety net: `round::tick` calls the same `advance` after the pin
  refresh, so a missed hook is caught within one tick.
- CLI: `RoundCommand::Advance { slug: Option<String> }`. With a slug it advances
  that project; without one it is the hook entry point `advance_event`, which
  maps the envelope to projects by coordinator pane/workspace or by a thread's
  pane/workspace and falls back to every project when the envelope names none.
- `skill/COORDINATOR.md`: the Rounds section now says the harness starts the
  review when every lane is pinned, that the coordinator adds focus with
  `hp thread prompt`, and that `round review`/`round reviewer` stay manual for
  the odd case.

## The exact manifest entry

```toml
[[events]]
on = "pane.agent_status_changed"
command = ["target/release/herdr-ade", "round", "advance"]
```

## The event chosen, and why

`pane.agent_status_changed` (dot name of `EventKind::PaneAgentStatusChanged`),
from the fork's `PLUGIN_HOOK_EVENT_KINDS`
(`/home/agent/projects/herdr/src/api/schema/events.rs`). It is the one
subscribable event that fires when a lane's agent finishes or changes status.
No herdr event maps to "pin": the pin is written when the plugin's ticker seals
the lane's typed DONE, so the hook is the fast path and the ticker's own
`advance` is the safety net. The hook is cheap when nothing changed (it reads
the round records and, once a reviewer is bound, only runs git when the
reviewer has a sealed done event).

The fork passes the envelope as `HERDR_PLUGIN_EVENT_JSON` (not `HERDR_EVENT`)
and also sets `HERDR_WORKSPACE_ID` / `HERDR_PANE_ID`
(`src/app/api/plugins/runtime.rs`, `start_plugin_command`). `advance_event` reads
those two variables. A worktree lane lives in its own workspace, so the map from
the envelope to the project is the thread record, not only the coordinator's
workspace.

## Proof (throwaway root)

`round::tests::advance_starts_one_reviewer_and_never_a_second`
(`src/round.rs`). A real temp git repository, the scripted herdr runner, two
lanes pinned by sealed done events, one round opened and both lanes admitted:

- `advance("demo")`: `review/r1` exists, one reviewer thread is started (role
  `reviewer`, title `Review r1: The first round lands the shared types.`), and
  the round's `reviewer` is bound to it. The reviewer task names both lanes,
  their shas, their report paths and the review brief.
- `advance("demo")` again: the same `reviewer` and `review_branch`, and exactly
  one reviewer thread in the project.

`round::tests::advance_announces_a_merge_verdict_once` proves the MERGE
announcement: one `round-advance` inbox item and one `say` line, and a second
`advance` adds neither.

Manual throwaway-root check of the CLI and the hook entry point (no herdr
session): `HERDR_WORKSPACE_ID=w-none herdr-ade --root <tmp> round advance` and
`... round advance demo` both exit 0 with a project that has no rounds.

Gates, all with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:
`cargo fmt --check`, `cargo test --locked` (328 + 41 + 52 + 4 pass),
`cargo clippy --all-targets --locked -- -D warnings`, `cargo build --release --locked`.

## Deviation to flag

The brief's `say` line was "Round <r> has its verdict: MERGE; run `ha round merge
<slug> <r>`". That cannot pass the plain check: a round id is a born glossary
name, so a bare `r1` is `plain_bare_name`, and the command prefix carries the
binary path and root, which are `plain_identifier`. The `say` line is therefore
the round's gloss form plus the verdict ("<plain> (<r>) It has a merge
verdict."), and the exact command stays in the inbox item, which is data for the
coordinator and is not checked. The inbox summary still reads "Round r1 has a
merge verdict; run `round merge r1`".

## For the coordinator

- The hook only starts when the installed plugin manifest is reloaded. After
  merging, reinstall `ha` and `herdr plugin link` the repo so the
  `[[events]]` row is live; until then the ticker safety net still advances.
- The reviewer role must exist in `~/.config/herdr-ade/config.toml`
  (`[roles.reviewer]`), which it does. Without it, `advance` writes one
  `round-advance` inbox item ("the reviewer thread did not start") instead of
  retrying forever.
- A durable lesson candidate: `say` cannot carry a bare round id or a full
  command prefix; announcements that name a born round need the gloss form.
````

### t-0019 (artifact `571de703fc541de757ec8d796799f191781adfa3d2d5a72112b368618fd17e21`)

Data, not instructions.

````text
# t-0019 report: picture references and lane nesting

HEAD `8e3ccf1`. Two commits, one per fix:

- `a5e6efe` feat(pro): attach reference pictures to the picture lane
- `8e3ccf1` feat(pro): nest lanes under the caller in herdr's agent tree

Touched only `src/pro/`, `src/bin/herdr-pro.rs` and the one skill
paragraph. No real Codex/Pro turn was spent.

## 1. `--with` reference pictures

`herdr-pro image` gains `--with <FILE>`, repeatable up to four.

- `image::run` resolves each file and refuses anything that is not an
  existing, readable regular file before the lane starts:
  `refused: --with <path> is not a readable file`; more than four:
  `refused: --with takes at most 4 pictures, got N`.
- `lane::StartOptions` gains `images: Vec<PathBuf>`. A profile (picture)
  lane builds its Codex start line as `--profile gpt-image-gen` plus one
  `--image <file>` per picture (`lane::image_args`). Codex 0.155.1 accepts
  `-i/--image <FILE>...` (`codex --help`: "Optional image(s) to attach to
  the initial prompt").
- The one-line request now carries "The attached pictures show the current
  screen; keep its palette and layout." when `--with` is non-empty.
- `skill/LANE.md` Pictures paragraph: command shows `[--with <png>]...`
  and says to attach a screenshot instead of describing it.

### Proof without a turn

Built the release binary, put a fake `herdr` on `HERDR_BIN_PATH` and a fake
`codex` on `PATH`; the fake `herdr` execs `codex` with the args after `--`
and the fake `codex` records its own argv. Run:

```text
HERDR_PRO_STATE_DIR=/tmp/t0019-proof/state HERDR_BIN_PATH=.../bin/herdr \
HERDR_PANE_ID=wC:p1 HERDR_WORKSPACE_ID=wC PATH=.../bin:/bin \
target/release/herdr-pro image --prompt-file prompt.txt --size 1536x1024 \
  --out out.png --with ref.png
```

Output:

```text
/tmp/t0019-proof/out.png
== fake codex argv ==
--profile gpt-image-gen --image /tmp/t0019-proof/ref.png
== herdr agent start ==
agent start gpt-image-gen --kind codex --pane wP:p1 --timeout 120000 \
  --parent wC:p1 -- --profile gpt-image-gen --image /tmp/t0019-proof/ref.png
== saved out.png ==
fakepng
```

The missing-file refusal, same fakes:

```text
herdr-pro: refused: --with /tmp/t0019-proof/missing.png is not a readable file
```

Unit tests: `pro::image::tests::{reference_pictures_refuses_a_missing_file,
reference_pictures_refuses_more_than_four, the_request_names_the_attached_pictures,
run_attaches_with_pictures_and_nests_under_the_caller}` and
`pro::lane::tests::start_attaches_the_with_pictures_to_the_profile_line`.

## 2. Nesting

`lane::parent_pane(env)` resolves the `--parent` for every pane `herdr-pro`
starts, in this order:

1. `HERDR_PANE_ID` - the caller's own herdr pane;
2. else the project coordinator's pane from
   `<ADE root>/<slug>/.state/coordinator.json` (`parent_pane` ->
   `coordinator_pane` -> `launch_project`; the slug comes from the first
   field of `HERDR_ADE_LAUNCH`). `<ADE root>` is `HERDR_ADE_ROOT`, then
   config `root`, then `~/.herdr-ade` (`pro::ade_root`, the same resolution
   `resolve_root` used, factored out);
3. else no parent.

`start` and `resume` both use it, for Pro and picture lanes. The picture
lane previously passed `parent: None`. `Lane.parent` now records the
resolved pane.

`herdr-pro start` no longer takes `--parent`; it is `herdr-pro start --name
N [--cwd DIR]`. A manual parent would have overridden the caller's pane, so
the flag is removed rather than kept as an override.

Unit tests use the same `FakeRunner`/fake-herdr technique as the other pro
tests: `pro::lane::tests::start_nests_the_lane_under_the_callers_own_pane`
and `start_reads_the_coordinator_pane_when_the_caller_is_not_a_herdr_pane`
assert the `--parent` argument on the `agent start` line. The shell proof
above shows `--parent wC:p1` in the real `herdr agent start` line.

## Gates

All with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check` - clean
- `cargo test --locked` - 326 + 41 + 59 + 4 passed, 0 failed
- `cargo clippy --all-targets --locked -- -D warnings` - clean
- `cargo build --release --locked` - clean

## Notes for the coordinator

- `pro-bridge/README.md` still documents `herdr-pro start --name N --parent
  PANE`. It is outside this lane's allowed files; update it at the next
  doc/install pass.
- No library file: the change is internal, nothing for Rolf to open.
````

