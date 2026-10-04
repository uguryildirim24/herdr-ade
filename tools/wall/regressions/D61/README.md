# D61 — connection loss during box start

Run `tools/wall/regressions/D61/repro --instance N` against an installed wall instance. It resets first, runs a 65-second simulated network outage, checks the visible linked pending start, then observes recovery on the same lane/attempt and one box card/agent. Optional `--evidence PATH` captures the final records.

Rust regression: `unreachable_box_start_is_visible_and_reconnect_places_one_linked_attempt`. The ambiguous received-create case is covered by `recovery_reclaims_box_terminal_when_create_reply_was_lost`: recovery identifies the existing terminal by the project workspace and frozen lane checkout rather than creating another terminal.
