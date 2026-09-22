+++
verdict = "MERGE"
round = "r95"
candidate = "2ceea19fbbfcdb28f71aee6bd8c0232e138714ef"
manifest_hash = "713684cbfb97b73fce47aa9589c7ee15f4c3ea5099d462e6086bbc3a3e30e3a5"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# Review

## t-0226

MERGE. New notes and task notes carry recorded dates and validated Rolf request ids. Historical project instructions, memory files, and task notes remain visible as undated records. Replacement is driven only by explicit ids; context renders both sides of the relationship, orders explicit replacements correctly, and caps the section at 20 rows. Brief assembly folds replaced rows, selects unscoped rows plus rows for the linked task, and keeps the memory cap.

The review fixed one cross-record defect: a note or task that replaced a decision left the stale decision in the talk screen and allowed another decision to replace it again. Current decision views now honor all record kinds, and one shared lock makes cross-kind replacement check-and-write atomic.

The removed coordinator rules are enforced by code: Clap rejects `thread start --role` and `--model`; the fourth open ask is refused with `ask_cap`; and a second round merge is refused with `round_merge_busy` while another merge transaction owns the branch. `task note` has no internal caller outside its CLI dispatch, while install and running evidence retain separate APIs. The talk projection and generated `TASKS.md` tests pass.

The requested format, test, lint, and diff checks pass.
