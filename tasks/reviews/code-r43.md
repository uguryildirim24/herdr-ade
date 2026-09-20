+++
verdict = "MERGE"
round = "r43"
candidate = "cf4895b531e0e2ab71a82b75fe35978c90d7396e"
manifest_hash = "0dda1fbef17ef4fbd80dbaadde1eb2b400a873bf1a53b84d793b1ea0a6044e73"
policy_hash = "3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174"
gates = []
+++

# Round r43 review

## Verdict

MERGE. The task brief and repository state now drive model selection through three editable Jev scores. Reviews use the same path with their committed brief and pinned diffs. The roles table and model-selection flags are refused, while executable recipes remain separate from routing policy. Failure events choose a strictly stronger tier, preserve the worktree, and stop at the recorded bound.

The shipped rubric is an initial policy, not a measured accuracy claim. I read the completed t-0083 research report that the box lane could not access, aligned its weight ordering, and kept the documentation explicit that real labelled cases must drive later tuning.

## Lane t-0082

Accepted after fixes:

- Kept drafter and critic workflow labels as instruction selection only. `thread start --workflow ...` does not name or pin a model, and dialogue start lines no longer lose their specialist skill text.
- Changed the built-in coordinator from Opus xhigh to Opus high.
- Replaced the gone-reviewer repair line that still used the removed `--role reviewer` flag with restart of the reviewer's recorded launch.
- Removed stale roles-table guidance, renamed the pi table as recipes, and repaired the doctor fixture exposed by the integration.
- Marked the routing policy as provisional and used the research report's difficulty/ambiguity/blast-radius weight order.

## Checks

The round brief listed no gates, so the verdict gate list is empty. I additionally ran:

- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools CARGO_TARGET_DIR=/home/agent/projects/herdr-ade/.target/review-r43 cargo fmt --check` — exit 0, no output.
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools CARGO_TARGET_DIR=/home/agent/projects/herdr-ade/.target/review-r43 cargo test` — `test result: ok`; 479 main, 56 pi, 78 pro, 6 CLI and 2 routing CLI tests passed.
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools CARGO_TARGET_DIR=/home/agent/projects/herdr-ade/.target/review-r43 cargo clippy --all-targets -- -D warnings` — `Finished dev profile` with exit 0.
- One manual `routing-eval` case without a saved response reached live TypeSafe Jev 1.13.0 and returned valid distributions for all three Score questions. The conservative confidence rule moved that trivial case one tier higher; this is one observation, not a policy pass criterion.
- `git diff --check e991567..HEAD` — exit 0, no output.

## Install note

Before this candidate runs, remove every `[roles.*]` table, install `config/routing.json` as `~/.config/herdr-ade/routing.json`, keep complete executable rows under `[recipes.*]`, and ensure the coordinator and ticker inherit `TYPESAFE_API_KEY`. Run `ha doctor` after installation.
