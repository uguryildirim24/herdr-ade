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
(`pi_args_forbidden`). Recipes and routing live in `config.toml`.

## Login and readiness

The plugin never runs a login and never sees a key. Rolf runs
`herdr-pi login <provider>` and types `/login` inside pi:

| Provider | What he does once per machine |
|---|---|
| `openai-codex` | Choose "ChatGPT Plus/Pro (Codex)", finish in the browser or with the device code |
| `opencode-go` | Choose "Use an API key", OpenCode Go (the Go plan's key) |
| `kimi-coding` | Use the device flow when the screen shows it, else paste the key |

One login serves every pi lane on that machine. A box uses its own configured
wrapper and `~/.herdr-ade/pi` login store; the Mac login does not count.
Never copy `auth.json` or any Mac credential to the box.

```
herdr-pi check <provider>    # readiness JSON; exit 1 when not ready
```

ADE checks readiness before `herdr agent start`. The credential check may
refresh OAuth; a tiny live, tool-free model call proves the provider works.
Results are cached for 15 seconds. `herdr-pi doctor` shows per-machine rows.
Starts are refused when the pin is not exactly 0.99.1, when `pi` in a login
shell is not the wrapper, when `settings.json` does not say
`defaultProjectTrust: "never"`, when the herdr extension or guard is missing,
or when the provider is not ready.

## Failure and recovery

A provider limit, a dead login or an unreachable endpoint is not idle or done.
The guard reports the pane `blocked` and seals one typed provider-failure
event: `limit`, `login`, `unreachable` or `error`. The harness starts a new
process for the same task on the same recipe; it never switches recipes.
Only automatic retries are bounded. An explicit
`ha thread retry <slug> <id> --reason "<evidence>"` remains available after
that allowance is exhausted. Never re-prompt or type recovery into the pane.

- `limit`: wait.
- `login`: tell Rolf to run `herdr-pi login <provider>` again.
- `unreachable` or `error`: let bounded recovery run.
- The guard never sends `DONE`. Only `ha done` does.

If the trust or missing-folder question appears, type nothing and report the
broken start through thread recovery. The settings file never trusts a folder.

## Restart

After a herdr restart, a saved `pi --session <path>` comes back through
`~/.local/bin/pi`, which points at the wrapper. The shared folder, model and
thinking level return by themselves. Do not replay launch flags.

On the box, `ha done` publishes the lane branch and seals its event. After a
reboot or resize, a box lane restarts from its brief, not a cold shell.

## What a pi lane must not do

- Never `pi install`, `pi update`, `pi remove` or `pi config`; the wrapper
  refuses them.
- Never write `~/.pi`, `~/.codex` or the shared `auth.json` by hand.
- Never pass a project's own `.pi/` add-ons through trust. Ask for the
  harness instead.
