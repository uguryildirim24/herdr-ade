# herdr-pro (plugin `pro-bridge`)

Pro (GPT-6) as an ordinary Codex lane on the installed
[`codex-chatgpt-web`](https://github.com/miuuyy/codex-chatgpt-web) bridge. herdr
sees a normal `codex` agent; the plugin feeds it a packet and collects the
answer from the Codex rollout. It never runs a ChatGPT page, never writes
`~/.codex`, and never drives a login.

SPEC: `herdr/tasks/pro-bridge/SPEC-pro-bridge.md` v2 (on the fork branch
`agent-parent-nesting`).

## Commands

| command | who | what |
|---|---|---|
| `herdr-pro init` | Rolf | link `~/.local/bin/herdr-pro`, create the state dirs |
| `herdr-pro login` | Rolf | print the two login steps; never drives one |
| `herdr-pro doctor [--json]` | Rolf / coordinator | bridge, route, login, breaker, herdr |
| `herdr-pro start --name N --parent PANE [--cwd DIR]` | coordinator | start a Pro Codex lane |
| `herdr-pro turn N --brief F --out F --notify AGENT [--attach F]... [--id TAG]` | coordinator | run one turn; forks a detached collector |
| `herdr-pro resume N` | coordinator | start a gone lane again and resume its Codex thread |
| `herdr-pro reconcile` | plugin startup | mark lanes whose pane stopped running Codex as `gone`; print resume lines |
| `herdr-pro stop N` | Rolf | the stop switch: stop one lane so a restart never brings it back |
| `herdr-pro resume-bridge` | Rolf | clear the breaker and `POST /admin/resume` |

`collector` is hidden; `turn` starts it detached.

## State

Under `~/.herdr-ade/pro-bridge` (override with `HERDR_PRO_STATE_DIR`). Turn
concurrency defaults to 2; `HERDR_PRO_MAX_INFLIGHT` may set it from 1 through
the hard maximum of 4:

```
lanes/<name>.toml      pane, parent, cwd, session_id, rollout, state
turns/<tag>.toml       brief, out, notify, packet, state
packets/<tag>.md       the packet Pro reads
inflight/<tag>.lock    one per running collector (mtime liveness)
usage.jsonl            one line per Pro send
cooldown-until         RFC 3339; the 2 h breaker
start.lock             flock; one Codex start at a time
turn.lock              flock; turn admission and the two-turn cap are atomic
bridge-state.json      last /healthz pid and version
```

Lane states: `starting → ready → in_turn → ready`, plus
`sign_in_required`, `bridge_down`, `cooldown`, `gone`. The turn record keeps
`delivered` or `failed`; only a ready herdr agent (`idle`/`done`) accepts a
turn.

## Turn

1. Preflight refuses a missing lane, a busy agent, an active breaker, two
   in-flight turns, an existing `--out`, a secret-looking path, or a packet
   over 200 KB / 60k tokens.
2. `loading`: `herdr agent prompt N "!cat -- '<packet>'"`, then wait for the
   `<user_shell_command>` item in the rollout (30 s).
3. `in_flight`: the TURN prompt, then wait for the `task_complete` of that
   turn in the rollout (120 min). One line goes to `usage.jsonl`.
4. `collecting`: a non-empty `last_agent_message` with no error is delivered;
   `rate_limit_exceeded` / "Stopped thinking" trips the breaker;
   `chatgpt_session_expired` is `failed(login)`.
5. `delivered`: write `--out` atomically (never overwrite; falls back to
   `<name>.<n>.md`), then type `DONE <tag> <out> -` to the coordinator.
   Every failure types `WAITING <tag> pro <reason>`.

The collector never resends. The breaker is the only automatic brake: a rate
limit, a bridge daemon restart (changed `/healthz` pid), or two failed turns in
ten minutes writes `cooldown-until = now + 2 h` and calls `POST /admin/drain`
with the control token from the bridge config.

## Bridge host and login

Default host: the installed Codex Web GPT launcher in browser-only mode, port
17841. The Chrome fallback is port 17941. `herdr-pro doctor` records the
`/healthz` version and refuses an unseen major.

Two logins: the bridge's own ChatGPT login (in the launcher), and Codex's own
login in the lane `CODEX_HOME` (`~/.codex` in v1). Run
`codex-chatgpt-web route disconnect` after any bridge setup so daily Codex does
not go through the bridge; `doctor` fails when `~/.codex/config.toml` carries
the bridge `openai_base_url`.

## Pro home (v2)

Set `HERDR_PRO_CODEX_HOME` to a shared Pro home and `start` / `resume` pass
`--env CODEX_HOME=<home>` on the lane tab and use that home for login, trust and
rollout lookup. Empty by default: v1 uses `~/.codex`. Persisting that env
across a cold restart is the fork change in thread t-0002.

## Link

Build with the repo's own gate, then link this directory:

```bash
cargo build --release --locked
herdr plugin link /home/agent/projects/herdr-ade/pro-bridge
herdr-pro init
```

`plugin link` does not run build commands. The manifest commands reach the
binary at `../target/release/herdr-pro` (this subdirectory is the plugin root).
