# D62 — held machine and deferred placement

Run `tools/wall/regressions/D62/repro --instance N` against an installed wall instance. It resets first, selects a genuinely different already-staged wall build, establishes version skew, holds the machine, then restores the candidate build. The deferred start must stay unplaced and display the hold reason. Release must place the same attempt once. A different-commit stage in `/var/lib/herdr-wall-builds` is required. Optional `--evidence PATH` captures held and released records.

Rust regression: `held_box_refuses_deferred_and_recovery_placement_until_release`. Existing explicit-hold and default-fallback tests remain. Placement checks the saved machine hold again at recovery, checkout, terminal binding and agent submission; holds do not spend launch attempts or expire as provider waits.
