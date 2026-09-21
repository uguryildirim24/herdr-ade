# D1 report

Removed Jev and its TypeSafe client, score/price/tier routing, evaluator command, policy files, probes, documentation, tests, coordinator kind refusal, and tier escalation. Replaced them with ordered `[routing]` rules in `config.toml`, hash pins, and bounded retry/fallback recovery. Launch and ledger records now name `pin`, `default`, or `rule[n]`; old records with removed fields still deserialize.

`git diff --numstat` reports 2,373 deleted lines and 475 added lines (net -1,898). The deleted implementation includes `src/jev.rs` (221), old `src/routing_tests.rs` (1,025), `config/routing.json` (38), 554 old lines from `src/routing.rs`, and 274 old lines from `src/launch.rs`.

Install this starting table in `~/.config/herdr-ade/config.toml` on the Mac and box:

```toml
[routing]
default = "pi_codex_sol_high"
retries = 1
fallback = []

[[routing.rules]]
workflow = "coordinator"
recipe = "claude_coordinator_opus"

[[routing.rules]]
product = "spec"
recipe = "claude_fable_xhigh"

[[routing.rules]]
product = "web-research"
recipe = "agy_gemini_flash"

[[routing.rules]]
requires_claude = true
recipe = "claude_fable_xhigh"
```

Verified:

- `cargo fmt --check`
- `cargo test` (705 tests across all targets)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
