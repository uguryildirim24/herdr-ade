+++
verdict = "MERGE"
round = "r5"
candidate = "827304e422f13ffe0dff9b708f5c25ce8bea4eb9"
manifest_hash = "db14696ec68f76ed56f6bf5b67771ef20df697085017fe70f7a5e01f03fa40ee"
policy_hash = "172e51b8408490ed414d7c6414e94353ab6f2e321ec23e1d68c59ce47abc193d"
gates = []
+++

# Round r5 review

## t-0009 — dedicated Pro Codex home

MERGE. The lane gives every Pro worker the one plugin-owned home at `<state dir>/codex-home`; removes the old override and daily-Codex fallback; writes the short instructions and AGENTS file; disables memories, multi-agent features, apps, plugins, skills instructions and sub-agents; passes `CODEX_HOME` on start and resume; writes exact-cwd trust only in the Pro config; checks the separate login; and still rejects a bridge route in Rolf's daily Codex config. The inherited trust-prompt and missing-rollout fixes fail closed before a turn is typed.

I added `review(pro): keep the shared home clean and serialized`. Re-running init now removes stale MCP or plugin configuration while retaining `[projects]`, and the start lock now covers the shared trust write and state decision so concurrent starts cannot lose trust entries or claim the same lane.

The built binary was run with a temporary HOME and `--state-dir`. First init produced `config.toml` (595 bytes), `instructions.md` (173 bytes), and `AGENTS.md` (543 bytes). After adding an existing project trust entry and a stale MCP entry, a second init retained the exact project entry and removed the MCP entry. The generated config had the expected instruction path and every required feature pin set to false. `herdr-pro login` printed the temporary Pro-home `CODEX_HOME=... codex login` line and said that `~/.codex/auth.json` is never copied. No Pro lane or message was started.

## Validation

The round gate manifest is empty. I also ran every check required by the task.

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(no output; exit 0)
```

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked
...
test result: ok. 52 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
```

(The preceding target summaries were 324 passed and 41 passed, also with zero failures.)

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets --locked -- -D warnings
...
    Checking toml v0.9.12+spec-1.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.95s
```

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo build --release --locked
   Compiling herdr-ade v0.1.0 (/home/agent/projects/herdr-ade/.worktrees/t-0012)
    Finished `release` profile [optimized] target(s) in 9.84s
```
