+++
verdict = "MERGE"
round = "r2"
candidate = "b0464c24ea2eedb3cfc1345218afee7de1a95586"
manifest_hash = "69543a3f7ff3d44b5cd1e7f1837f6616babb794a7e7ab3681975f3c07da53d72"
policy_hash = "acfaae9ab98bf4e0b0925f47cd4a8cc784944e12bbd9b9caae384ce29bfd3ff4"
gates = ["cargo fmt --check", "cargo test --locked", "cargo build --release --locked", "cargo clippy --bin herdr-pro --locked -- -D warnings"]
+++

# Round r2: MERGE

## t-0003 — Pro bridge

The candidate keeps the bridge route on each Pro lane's Codex process and never installs it in Rolf's daily Codex config. It carries one conversation across turns, admits two turns by default with a hard maximum of four, drains for the two-hour breaker, and lets only the collector type a result-backed `DONE <tag> <out> -`.

Review fixes made admission atomic, reran the fail-closed doctor before every turn, made stop durable, made answer writes atomic, and refused rollout error events even when they carry text. State remains under the ADE root's `pro-bridge` folder and ignores `HERDR_PLUGIN_STATE_DIR`.

All four gates passed. The release build reports only the named base warning in `src/contracts.rs:421`, outside this lane and unchanged here. No Pro message was sent and no Pro lane was started.
