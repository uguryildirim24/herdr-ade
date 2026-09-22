+++
verdict = "MERGE"
round = "r87"
candidate = "3abaf7886148b5d21f540e50d69eafa9220ba013"
manifest_hash = "9b0ac442248bd1fc5f98bb63c0caac54eeb59e2d0533899d3354a1400066d7d9"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# Review r87

## Verdict

MERGE. The pinned W10 lane is an ancestor of the candidate. I found no code defect that needed a review commit.

## t-0206 — recipe visibility and one-off selection

`ha context` renders one compact row per recipe from the merged shipped and configured recipe map. Each enabled row includes the id, plain use, capabilities, default and matching rule triggers, plus either the guarded `thread start --recipe ... --basis ...` command or the two Pro commands. A disabled row returns before route rendering, so it appears once with no route.

Clap refuses `--recipe` without `--basis`. The start path then loads the stable task and accepts only an authority entry beginning `request:` whose durable Rolf message contains the trimmed quote verbatim. A missing request authority or a quote absent from every attached request is refused. The selected recipe, quote and request are stored in the dispatch ledger and launch record; context prints them on the lane. The keyed decision is `money` for a non-default recipe and `routine` for the default. Normal routing remains the only path without Rolf's attached words.

For request `q-1790038203253-80143`, the lookup is project-local in the correct project, not session-local and not a search from Adeherdr. `thread start prl-8-53 ...` loads the `prl-8-53` project, loads that project's task, takes the request id from its authority list, and `talk::request_text` reads `prl-8-53/talk/journal.jsonl` in reverse for the matching `Entry::Rolf`. It then checks that the quoted basis occurs in that exact text. Thus the request being recorded in the prl-8-53 session is exactly what makes it available to a prl-8-53 start.

The live Mac's enabled `pi_codex_astra_max` needs no routing rule. Doctor treats every enabled non-Pro recipe whose kind has a declared adapter as command-reachable through the guarded explicit start. Pro recipes are command-reachable through `herdr-pro start` and `herdr-pro turn`. Routing and adapter validation still reject unknown, disabled or invalid rows, so Astra Max's rule-free row will not fail doctor after install.

The coordinator skill gives the recipe-add sequence, adapter-capability constraint, one-off selection with Rolf's quote, and the short Mac-only Pro start/turn sequence. The command shapes match the CLIs.

## Requested checks

The round manifest listed no gates, so the front matter remains `gates = []`. I also ran all four checks Rolf requested:

- `cargo fmt --check` — exit 0; no output.
- `cargo test` — exit 0. Final suite lines:
  - `test result: ok. 610 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 35.95s`
  - `test result: ok. 57 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.81s`
  - `test result: ok. 78 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.83s`
  - `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.85s`
  - `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.15s`
  - `test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s`
  - `test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s`
- `cargo clippy --all-targets -- -D warnings` — exit 0; `Finished dev profile [unoptimized + debuginfo] target(s) in 26.85s`.
- `git diff --check` — exit 0; no output.
