+++
verdict = "MERGE"
round = "r14"
candidate = "6d20c16e0ebaf94d95dc7738fe54b8901b868fda"
manifest_hash = "6aaa195d4ea728046568e5841f4936484a8102516890cd6ae485c92bbf8e6d4d"
policy_hash = "7cf667dea9748fe2844267e971e613ff74b94a3ef0af9df5ee974d759cb7e9da"
gates = []
+++

# Round r14 review

## t-0032

MERGE. `herdr-pi setup` derives the DeepSeek model ids from the recipe table and merges the 388384-token context window into the shared `models.json` without replacing unrelated providers, overrides, or keys. The setup path runs without the Pro relay, remains idempotent, and protects the replacement file with mode 0600.

The doctor reads the file back and names any recipe model with a missing or wrong override. The lane text accurately says compaction happens near 372k while the complete session record remains on disk.

## Gates

The round brief listed no gates, so none were run.
