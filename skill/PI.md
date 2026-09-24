# PI.md — running pi lanes

A pi lane runs many model services through one program. The harness owns the
install, the settings folder, and the logins. Start a pi lane through
`ha thread start` like any other lane. The wrapper supplies the folder, so
the launch record carries no path and no secret.

## Launch record

```
herdr agent start <name> --kind pi --pane <pane> --parent <coordinator pane> \
  --timeout 30000 -- \
  --provider <provider> --model <model> --thinking <level> --no-skills
```

Always `--no-skills`. Never `--approve` or `-a`. Never `--session`, `-c`,
`-r`, `--config-dir` or an extension flag on a recipe: those are refused
(`pi_args_forbidden`).

## Rows this round

| Recipe | Provider | Model | Thinking |
|---|---|---|---|
| DeepSeek through OpenCode Go | `opencode-go` | `deepseek-v4.1-flash` | `high` |
| OpenCode Muse | `opencode-go` | `muse-spark-1.3-contributor` | `high` |
| Kimi k3 | `kimi-coding` | `k3` | `high` |
| ChatGPT Sol | `openai-codex` | `gpt-5.6-sol` | `high` |
| ChatGPT Astra (disabled) | `openai-codex` | `gpt-6-astra` | `xhigh` |

Your context is compacted near 372k tokens on DeepSeek; nothing is lost, the
full record stays in the session file.

Cursor stays outside pi. There is no
`pi_cursor_*` row.

## The one-time login is Rolf's

The plugin never runs a login and never sees a key. Rolf runs
`herdr-pi login` and types `/login` inside pi, in the shared folder:

| Provider | What he does once |
|---|---|
| `openai-codex` | `/login`, "ChatGPT Plus/Pro (Codex)", finish in the browser or with the device code |
| `opencode-go` | `/login`, "Use an API key", OpenCode Go (the Go plan's key) |
| `kimi-coding` | `/login`; use the device flow when the screen shows it, else paste the key |

One login serves every pi lane. A lane that starts without a login is
refused before the tab opens (`pi auth check`, no network refresh).

## Check before a start

```
herdr-pi check <provider>    # read-only JSON; exit 1 when not ready
```

ADE checks this itself before `herdr agent start`. It refuses when the pin is
not exactly 0.85.1, when `pi` in a login shell is not the wrapper, when
`settings.json` does not say `defaultProjectTrust: "never"`, when the herdr
extension or the guard is missing, or when the login is not ready.

## When a lane is stuck

A provider limit, a dead login or an unreachable endpoint is not idle and not
done. The guard reports the pane `blocked` and seals one typed provider-failure
event. Its provider kind is `limit`, `login`, `unreachable` or `error`.

The event starts bounded recovery automatically. Every provider failure starts
the same task as a new process on the same recipe; it never selects a fallback.
An explicit `ha thread retry <slug> <id> --reason "<evidence>"` does the same and
consumes the next bounded retry. It refuses after the limit is exhausted and
waits for the coordinator. Never re-prompt or type recovery into the pane.

- `limit`: wait.
- `login`: tell Rolf to run `herdr-pi login` again.
- `unreachable` or `error`: let the bounded recovery run.
- The guard never sends `DONE`. Only `ha done` does.

If you see the trust question or the missing-folder question on screen, type
nothing. Report the broken start through thread recovery. A trusted folder
runs repository code, so the settings file never trusts one.

## After a herdr restart

A saved `pi --session <path>` comes back through `~/.local/bin/pi`, which
points at the wrapper. The shared folder, the model and the thinking level
come back by themselves. Do not replay `--provider`, `--model` or
`--approve`.

## On the cloud box

A pi lane on the box runs the guarded wrapper at its configured pi path
against the box's own `~/.herdr-ade/pi` login store.
The Mac login does not count. Rolf signs each provider in once on the box
(the coordinator opens a terminal for it): `herdr-pi login openai-codex`,
`login opencode-go`, `login kimi-coding`. `herdr-pi check <provider>` on the
box gates the start, and `herdr-pi doctor` shows the per-machine rows. Never
copy `auth.json` or any Mac credential to the box.

On the box, `ha done` publishes a pi lane's branch and seals its event. The lane is
restarted from the brief after a reboot or resize like any other box lane.

## What a pi lane must not do

- Never `pi install`, `pi update`, `pi remove` or `pi config`; the wrapper
  refuses them.
- Never write `~/.pi`, `~/.codex` or the shared `auth.json` by hand.
- Never pass a project's own `.pi/` add-ons through trust. Ask for the
  harness instead.
