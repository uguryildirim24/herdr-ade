# ADE → Herdr call inventory

Source baseline: `6051f43e94d790d1ec219410d0127a7aae4c86b2`.
Scope: every command constructed by `src/herdr.rs`, plus its production
`call()` callers and direct Herdr invocations elsewhere in `src/`.
`src/runner.rs` executes argv without a shell; its fake validates syntax then
returns scripted responses. It is **not** a live-resource/state emulator.

Notation: `P/T/W` = recorded pane/tab/workspace ID; `N` = agent name;
`S` = arbitrary Rust string; `D` = filesystem path; `U` = unsigned milliseconds;
`[x]*` = repeated argument pair. Each table row denotes argv, not shell syntax.

## Common envelope

- Binary: `HERDR_BIN_PATH`, otherwise `herdr` (`src/paths.rs:60`).
- Bound calls (`src/herdr.rs:107`): set `HERDR_SOCKET_PATH` to the recorded socket,
  remove inherited `HERDR_SESSION`; optionally prefix `--machine M` for a
  nonempty saved machine route. No `--session` is added by these wrappers.
- An empty local socket is rejected before execution; forwarded calls may have
  an empty local socket. `reachable()` separately checks local socket existence.
- Ordinary calls have a 10-second process deadline. JSON success is
  `result`, JSON errors may be on stdout **or stderr**; no-output successful
  commands are accepted. Pane reads return terminal text, not JSON.
- Names generated from project slugs: `hp-SLUG-ROLE`; if over 32 bytes,
  `hp-<16 hex SHA256 chars>-ROLE`. Roles are `coordinator` or a thread ID.
- Paths and labels are individual arguments, not shell-interpolated fragments.
  NUL cannot reach exec. Linux also limits each argv/environment string (H01).

## All typed calls (`src/herdr.rs`)

| Entry point / line | Exact argument shape | Live coverage |
|---|---|---|
| `version` 38 | `--version` | yes |
| `session_list` 57 | `session list --json` | yes; reply is top-level `sessions`, not `result` |
| `parent_on_start_supported` 819 | `agent start --help` | yes; text contains `--parent` |
| `reachable` 129 | `status server` | yes, including after failed note submission |
| `workspace_list` 421 | `workspace list` | yes |
| `tab_list` 425 | `tab list` | yes |
| `pane_list` 429 | `pane list` | yes |
| `agent_list` 433 | `agent list` | yes, idle/done/working/blocked/unknown |
| `workspace_create_env` 458 | `workspace create --cwd D --label S (--focus\|--no-focus) [--env KEY=VALUE]*` | real `ha open`, long project name and Unicode display name |
| `tab_create_env` 486 | `tab create --workspace W --cwd D --label S (--focus\|--no-focus) [--env KEY=VALUE]*` | real lane start, boundary fixtures |
| `tab_rename` 516 | `tab rename T S` | empty, Unicode, 4096 bytes, leading dash |
| `tab_close` 521 | `tab close T` | open and already-closed |
| `workspace_label` 526 | `workspace get W` | real open/Rundown |
| `workspace_rename` 534 | `workspace rename W S` | empty, Unicode, 4096 bytes, leading dash |
| `pane_cwd` / `pane_get` 540/544 | `pane get P` | ordinary, odd IDs, closed pane |
| `pane_process_info` 552 | `pane process-info --pane P` | pi, shell after failed start, odd IDs, closed pane, restart |
| `pane_read_text` / `pane_read_ansi` 561/565 | `pane read P --source S --format (text\|ansi)` | every accepted source spelling, invalid `screen`/empty, closed pane; visible launch failure |
| `workspace_close` 592 | `workspace close W` | scratch workspace cleanup |
| `agent_start_opts` / `agent_start_many` 627/635 | `agent start N --kind KIND --pane P --timeout U [--parent PARENT] [--env PATH=D] [-- ARGS...]` | actual coordinator/lane starts; name/kind/timeout boundaries; parent from lane start; missing executable |
| `agent_wait_ready` 669 | `agent wait TARGET --until idle --until done --timeout U` | 0, 1, 3000, 3001, 300000, 300001, u64::MAX with an already-ready agent |
| `agent_prompt` 698 | `agent prompt TARGET S` | empty, whitespace, newline, Unicode, dash, 64 KiB, 128 KiB; blocked refusal |
| `pane_submit_text` 706 | `pane send-text P S`, then `pane send-keys P Enter` | call shape inventoried; adapter recovery journey not established |
| `pane_send_keys` 711 | `pane send-keys P KEY` | Enter, Down, Escape, C-c, F0, F255, Unicode, invalid/empty |
| `agent_prompt_wait_started` 717 | `agent prompt TARGET S --wait --until working --until blocked --timeout U` | actual file-note H01; delayed working transition; zero-deadline blocked case |
| `agent_focus` 743 | `agent focus TARGET` | shape inventoried; interactive client focus not established |
| `agent_rename` 751 | `agent rename TARGET N` | empty, 32/33 bytes, Unicode, spaces, dash |
| `notification_show` 756 | `notification show TITLE --body BODY` | blank TITLE mismatch H02; Unicode/long labels |
| `pane_report_tokens` 766 | `pane report-metadata P --source herdr-ade --ttl-ms U [--token KEY=VALUE]*` | actual ticker metadata; no exhaustive TTL/capacity sweep |
| `pane_clear_tokens` 790 | `pane report-metadata P --source herdr-ade [--clear-token KEY]*` | shape inventoried; exhaustive clear combinations not established |
| `pane_set_parent` 800 | `pane report-metadata P --source herdr-ade --token parent=PARENT` | actual lane parent, nonexistent-parent value, self-parent refusal |

Start timeout is clamped to **3001..300000**, including requests 0 and u64::MAX;
process deadline is effective timeout + 5000 ms. Start-many executes independent
starts concurrently, preserves input ordering, and uses the same argv.
Wait/prompt-wait retain the supplied u64 without start's clamp and add 5000 ms
to the enclosing deadline. An already-ready u64::MAX wait was exercised; an
actually unbounded wait was not left running.

`--parent` is omitted for None/empty, `--env PATH=...` only for `launch_bin`, and
`--` only for nonempty agent arguments. Real Herdr supports literal dash-leading
prompt text in the second positional slot; inserting `--` there is incorrect.
Agent args (unlike prompt text) reject control characters. The fake models
these constraints, name/kind validation and start timeout bounds.

Read sources: `visible`, `recent`, `recent-unwrapped`, `recent_unwrapped`,
`detection`; production readers predominantly use `visible` and `detection`.
Odd IDs do not necessarily mean syntax errors: resource lookup often returns
`pane_not_found`/`agent_not_found`. These are scripted-state responsibilities,
not missing static validation findings. In particular `pane get w2-1` resolved
an existing pane while `agent get w2-1` did not; ADE persists canonical IDs.

## Calls outside the typed methods

| Location | Argument shape | Coverage |
|---|---|---|
| `src/actions.rs:72` | `plugin pane open --plugin herdr-ade --entrypoint ENTRYPOINT` | shape only; action popup entrypoints absent from sandbox manifest |
| `src/rundown/mod.rs:26` | `plugin pane open --plugin herdr-ade --entrypoint rundown --placement tab --workspace W --env HERDR_RUNDOWN_PROJECT=SLUG --env HERDR_RUNDOWN_TITLE=TITLE --env HERDR_ADE_ROOT=D --no-focus` | actual `ha open`, including Unicode title |
| `src/rundown/mod.rs:73`, `src/harness.rs:1368` | `api snapshot` | yes |
| same modules, lines 88/1379 | `plugin pane focus P` | normal Rundown handling; client visual focus not established |
| same modules, lines 110/1396 | `tab focus T` | focus restoration shape; client visuals not established |
| `src/coordinator.rs:678` | `tab rename T coordinator` | actual open |
| `src/claude_trust.rs:146` | `pane wait-output P --source visible --regex '(?m)^[ \\t]*❯ Yes, I trust this folder[ \\t]*\\r?$' --timeout 1000` | source review only; 6-second outer deadline; Mac trust dialog unavailable |
| `src/remote.rs:201` | `machine list --json` | yes; saved-machine lookup (SSH deadline) |
| `src/doctor.rs:434`, called by machine probes and `src/steps.rs:1090` | `(pane\|agent) list`, env `HERDR_SESSION=NAME`, own process group, 10-second deadline | real courier helper H03; **inherited socket is not removed** |
| `src/steps.rs:1162` courier screen | `pane read P --source detection --format text`, env `HERDR_SESSION=NAME`, 30-second deadline | shape inventoried; shares H03's unremoved ambient socket |
| `src/journey.rs:441,1246`, `src/threads.rs:3083,3093` | `session stop NAME [--json]`, `session delete NAME [--json]` | scratch session stop/delete; flags used by thread cleanup |
| `src/journey.rs:975` | `--session NAME server` | direct `std::process::Command`, isolated server process group; same shape used by fixtures |
| `src/pi/install.rs:169` | `integration install pi` | real wall install/reset; separate `pi::sh` runner, 120-second deadline, `PI_CODING_AGENT_DIR=D` |
| `src/pi/doctor.rs:400` | `integration status` | yes; separate `pi::sh` runner, own process group, `PI_CODING_AGENT_DIR=D` |

`Herdr::call()` can accept arbitrary argv internally; the above enumerates its
production callsites, not every command the Herdr CLI supports. Fake-validator
support for extra commands (`agent get`, `pane run`, plugin administration,
etc.) does not mean ADE emits them in production. Installed Herdr lifecycle
hooks (`pane report-agent`) belong to Herdr's integration; the fixture uses
that real hook API to provide deterministic lifecycle observations.

## Runner boundaries

`RealRunner` (`src/runner.rs:329`) passes cwd/env/argv, closes absent stdin,
drains bounded stdout/stderr, applies process deadlines, and executes parallel
starts. FakeRunner (`:682`) validates before matching scripted responses;
process existence, OS exec limits, JSON contents, changing state, PTY writes,
connection loss and races are not simulated. Direct doctor/courier constructors are exceptions to the common bound-command
envelope; H03 demonstrates why their competing environment matters.
The live 111-call matrix was also
fed through **the actual** `fake_herdr::validate` using a temporary standalone
compiler wrapper; no handwritten approximation was used.
