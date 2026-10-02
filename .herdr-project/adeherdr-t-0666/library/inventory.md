# Inventory at d69a2c8

Audit machine: saved box `oci` (hostname `instance-20260918-1928`). No private config, project records, notes, panes or remote repositories were inspected. All source references below are to `d69a2c8e8f6c5a40b56890f8831891d06db3eb6e`, not a future refactor.

## Measured size, not the brief's rounded figures

Command used for physical lines and final test-block boundaries:

```python
from pathlib import Path
import re
files = sorted(Path('src').rglob('*.rs'))
print(sum(len(p.read_bytes().splitlines()) for p in files), len(files))
for name in ['threads','ticker','doctor','steps','cli','plan','thread','scenarios']:
    p = Path('src') / (name + '.rs')
    s = p.read_text()
    m = re.search(r'^#\[cfg\(test\)\]\nmod tests \{', s, re.M)
    n = s[:m.start()].count('\n') if m else (0 if name == 'scenarios' else len(s.splitlines()))
    print(p, len(s.splitlines()), n, len(s.splitlines()) - n)
```

Output:

```text
68961 64
src/threads.rs    8347 5569 2778
src/ticker.rs     7290 3266 4024
src/doctor.rs     3995 2254 1741
src/steps.rs      2882 1566 1316
src/cli.rs        2371 2051  320
src/plan.rs       1977 1100  877
src/thread.rs     2357 1486  871
src/scenarios.rs  3289    0 3289
```

Columns: total physical lines; before final test module; final test module. The middle column still includes scattered `#[cfg(test)]` helpers. Comments and blank lines count; this is not executable LOC.

A second count sums final `#[cfg(test)] mod tests` / `pub(crate) mod fake` suffixes, plus the standalone `tests.rs`, `scenarios.rs`, `testkit.rs` and `src/**/tests/*.rs` files:

```text
cfg(test) suffix lines (includes fake modules): 22094
standalone test-only src files:                 6116
test-only lower bound:                        28210
remaining src incl scattered cfg(test):       40751
```

Thus about 41% of `src/` is demonstrably test-only text. It is wrong to describe all 68,961 lines as runtime machinery. `src/main.rs:67-72` excludes scenarios/testkit from production. `src/threads.rs:5570`, `src/ticker.rs:3267`, `src/doctor.rs:2255`, `src/steps.rs:1567`, `src/cli.rs:2052` mark their final test modules.

Field-count command: take the text between `struct NAME {` and its next column-zero `}`, then count `^\s*pub(?:\(crate\))?\s+(\w+)\s*:`. Output:

```text
Thread fields=87              src/thread.rs:75-230
Review fields=29              src/review.rs:60-95
MachineDeclaration fields=13 src/remote.rs:59-75
```

These are size measurements, not limits to enforce.

## CLI: 27 visible implemented commands, plus help

Command: `$CARGO_TARGET_DIR/debug/herdr-ade --help` after `cargo test` built this pin. Its command list:

```text
new list open context overview inbox task note thread machine
pause resume archive unarchive delete adopt-workspace
done waiting failed skill close doctor review harness ask plan ticker help
```

The brief's “28” includes Clap's generated `help`. Source enumeration of `src/cli.rs:49-305` gives **32 implemented top-level variants, five explicitly hidden**, hence 27 visible. Hidden: `action`, `recover`, `event`, `pane`, `hook` (`src/cli.rs:184-195,229-242`). `event review-advance` is a no-op compatibility exit for an old loaded manifest, not a surviving round driver (`src/cli.rs:1192-1195,1927-1928`). Delete that shim under S1; the current manifest has no event hook (`herdr-plugin.toml:1-96`). Do not invent a second round engine to delete.

### Proposed help surface (finding S9; no functionality removed)

| Audience | Keep visible / show in its role skill | Hide or demote from first-run help |
|---|---|---|
| First project | `new`, `list`, `open`, `close`, `overview`, `doctor` | Everything else can be introduced by workflow. |
| Coordinator | `context`, `inbox`, `task`, `note`, `thread`, `review`, `plan`, `pause`, `resume` | Recovery and exceptional cleanup belong in long subcommand help. |
| Lane protocol | `skill`, `done`, `waiting`, `failed` | Hide from top-level human help, keep exact command spellings for generated briefs. |
| Administration | `archive`, `unarchive`, `delete`, `machine` | Keep discoverable in long help/docs, not the getting-started path. |
| Plumbing | Already-hidden `action`, `recover`, `pane`, `hook`; `ticker`, `harness`, `adopt-workspace` | Hide these three too. Delete obsolete `event` under S1. Workspace adoption remains reachable through the plugin action. |
| Retired workflow | `ask`, `ask close` | Delete, including writers and publication (M1), not just hide. |

Evidence for existing role grouping: `ha thread --help` already labels everyday, recovery and administration; source `src/cli.rs:122-128`. Its 11 implemented subcommands are `start retry cancel rebind attest prompt list show adopt ack resolve` (`src/cli.rs:832-964`). Keep that grouping. `overview` is grouped project work; `thread list` is a lane inventory; `context` is an acknowledging agent digest. They are different views, not three redundant commands (`src/cli.rs:84-105,905-916`; `src/coordinator.rs:1174`).

Do **not** collapse `waiting` into `failed`, or `close` into `archive`. Waiting and failed payloads affect recovery differently (`src/lane.rs:102-192`), while coordinator closure and project status are separate operations (`src/cli.rs:1892-1900,1941`). Do not remove `resolve`'s exceptional controls during a help cleanup (`src/cli.rs:946-963`). Rename neither `thread` nor its records just to match the prose word “lane”: little benefit, broad churn.

Estimated net deletion for help-only work: **0 lines**; this is cognitive simplification, not a LOC win. Effort S, low risk.

## Config: actual knobs and a smaller shape

Config reads are already centralized at the file boundary in `src/config.rs:17-49`. The problem is parallel schemas and duplicate authority, not a missing configuration framework.

| Surface | Evidence | Proposal |
|---|---|---|
| `root` | `src/paths.rs:84-103` | Keep. One shared environment/root resolver for both binaries; currently repeated in `src/pi/mod.rs:176-205`. |
| `[recipes.*]`, `[routing]` | `src/contracts.rs:8-33`; `src/routing.rs:10-26` | Keep executable recipe + ordered matching/retry policy. Delete the disabled shipped row, not arbitrary custom disabled entries. Put historical catalogs in examples rather than growing core defaults. |
| `[adapters.*]` | `src/adapters.rs:14-65` | Keep harness capabilities and execution details; not a second place for model choice. |
| `[dispatch].machine` | `src/launch.rs:20-24` | Keep default placement; explicit lane/repo placement remains meaningful. |
| `[machines.*]` | `src/remote.rs:59-111,178-224` | Have one authority for machine identity/SSH target/session. Herdr's saved profile already supplies these; ADE should primarily add paths, kinds and repo mappings. Present fallback config-only profiles are another implementation, not necessary history readers. This is part of S5, not an immediate removal without checking actual supported deployments. |
| `[harness].repos` | `src/harness.rs:124-145,1253-1311` | Advanced self-install configuration. Do not require every project to understand it. Keep repository gate/install policy. |
| `[worktrees].disposable`, `[doctor].min_free_disk_gb` | `src/worktrees.rs:19-23,107-125`; `src/launch.rs:27-38` | Keep: they encode what may be deleted and disk readiness, not old presentation preferences. |
| `[ticker]` | `src/ticker.rs:1381-1395,1473-1474` | Keep progress observation thresholds with ticker policy, not provider readiness. |
| `PROJECT.md` | `src/project.rs:139-175` | Only `name`, `goal`, `repos`; repo rows contain `path`, `branch`, `push_remote`, `gates`, `machine`, `box_path`, `publish_url`, `disposable`. This is already fairly small. Delete removed-key diagnostics, not valid repo-level distinctions. |
| Brief front matter | `src/launch.rs:250-281` | `product`, `capability`, `once`; workflow is supplied separately. Keep intent separate from recipe selection. |
| Obsolete keys | `src/launch.rs:82-101`; `src/project.rs:190-227`; `src/doctor.rs:1118-1134` | Remove named `[roles]`, `requires_claude`, old agent/max-parallel/project-wide-gates refusal machinery, its tests and instructions. No replacement migration switch. See S3. |

`push_remote` and `publish_url` are **not** interchangeable solely because both name remotes: integration publication and lane publication are distinct (`src/project.rs:145-160`; `src/review.rs:1393-1435`). Likewise frozen `Launch` data is evidence of the actual recipe, not just a redundant copy of current config (`src/contracts.rs:91-140`).

### Environment inventory / recommendation

Production reads, located with `rg -n 'HERDR_ADE_[A-Z_]+|HERDR_PI_[A-Z_]+' src` and manual exclusion of tests/protocol output strings:

- Configuration entry points: `HERDR_ADE_ROOT`, `XDG_CONFIG_HOME`, `HERDR_BIN_PATH`, `HERDR_SESSION`, `HERDR_SOCKET_PATH` (`src/paths.rs:50-62,84-103,142-150`). Keep these; no alternate config system needed.
- Runtime identity: `HERDR_ADE_LAUNCH`, `HERDR_PANE_ID`, plugin context/workspace variables (`src/project.rs:583-667`; `src/lane.rs:333-376`; `src/actions.rs:47-59,100-106`). Document as injected, not editable setup knobs.
- Internal ticker control: `HERDR_ADE_TICKER_SUPERVISOR`, `HERDR_ADE_INSTALL_TICKER` (`src/ticker.rs:183,212`). Keep private. `HERDR_ADE_BOX_*` / `HERDR_ADE_INSTALLED_HEAD` in the installer are shell-output markers, **not** extra config env vars (`src/harness.rs:672-685,886-920`).
- Two policy overrides live elsewhere: `HERDR_ADE_OUTAGE_SECS` (`src/steps.rs:689`) and `HERDR_ADE_REVIEW_IDLE_SECS` (`src/review.rs:339`). **Nice to have · S · 0–10 lines, low risk:** either document as diagnostic overrides or consolidate with existing ticker policy when touching that code. Do not add a second spelling now. Excluded from the plan totals.
- Rundown receives `HERDR_RUNDOWN_PROJECT`, `HERDR_RUNDOWN_TITLE`, `HERDR_ADE_ROOT` (`src/bin/herdr-rundown.rs:54-57,130`). Keep private.
- Pi wrapper supplies `PI_CODING_AGENT_DIR`, `PI_SKIP_VERSION_CHECK`, `PI_TELEMETRY`; `SHELL` selects login-shell probing (`src/pi/launch.rs:124-129`; `src/pi/sh.rs:271-283`). Keep runtime-owned.
- `HERDR_ADE_SWAP_DIR` belongs only to the retired script/test pair; delete with that pair (S1).

## Skill size and history

Command: count `splitlines()` and whitespace-separated `split()` words for each `skill/*.md`.

```text
COORDINATOR.md 81 lines 1334 words
LANE.md        51 lines  664 words
PI.md         108 lines 754 words
REVIEWER.md    32 lines  436 words
```

- Lane and reviewer are not the main bloat. Keep worktree identity, immutable finish evidence, isolated visual probes and reviewer gate/verdict instructions (`skill/LANE.md:5-37`; `skill/REVIEWER.md:7-32`). They prevent lost work or wrong landing, not vocabulary mistakes.
- Coordinator repeats “ask in chat” / ordinary reversible choices in lines 63 and 69. The single long plan/dependency paragraph at line 49 mixes command reference, history and operating instructions. Replace it with the normal sequence and link advanced command reference. Estimated **5–10 physical lines**, mainly a word-count improvement; no numeric word budget proposed.
- PI is carrying history: “Rows this round” including a disabled recipe (`skill/PI.md:20-34`); a read-only/no-network claim (`:46-48`) conflicts with the current live cached model probe (`src/pi/doctor.rs:1-7,19-22,848-957`); bounded explicit retry prose (`skill/PI.md:67-71`) conflicts with current CLI policy (`src/cli.rs:122-125`) and manual retry implementation (`src/threads.rs:1762-1765`, `src/launch.rs:379-415`). Rewrite against current behavior, remove the historical model table, keep login/trust/failure instructions. Estimated **35–55 lines**.

## Gates run on the unchanged code

Full output: [gates.txt](gates.txt). Commands, actual exit and wall-clock elapsed:

```text
cargo fmt --check                         exit=0 elapsed=1s
cargo test                                exit=0 elapsed=156s
cargo clippy --all-targets -- -D warnings  exit=0 elapsed=23s
git diff --check                          exit=0 elapsed=0s
```

`cargo test`: **740 passed, 0 failed, 2 ignored** across the eight reported test binaries (661+50+9+11+3+2+1+3 passed). The first two include the same pi/config unit modules twice: `rg -n '^test (pi::|config::)' library/gates.txt | wc -l` returned **100**. They are still ordinary passing gate tests; duplication is architectural, not evidence of unreliable code. See S4.

No extra smoke tests, provider calls, new agents, live sessions or install commands were run. Gate source is `.github/workflows/rust.yml:15-17,24-30`; the separately supplied brief also names these four gates.
