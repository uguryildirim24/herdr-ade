+++
verdict = "MERGE"
round = "r32"
candidate = "beb349ef194d28d1d445b3893a627760c48f5831"
manifest_hash = "3a004808bc8e09d074e815428dd524e6e158eb2ada88b10a5319a13f4a01f522"
policy_hash = "df69155c02a5636dc8a86ec27ade9e888d03782a6c7bd85df748f4c262aaee68"
gates = []
+++

# Round r32 review

## t-0069

The compact overview now clips each heading and content row to one terminal line by terminal-cell width, while F2 keeps the complete wrapping text. New round and thread birth sentences and plan text remain subject to the existing 25-word checker. Stored long records render instead of being replaced by an invalid-text fallback.

Review fixed two remaining layout gaps. The `box` and `last seen` markers now have reserved cells, so clipping cannot remove them. The compact progress row also uses one line and shortens its bar to the available width instead of passing through the wrapping prose renderer.

No project gates were listed. Supplemental `cargo fmt --check`, `cargo test --locked` (433 + 56 + 78 + 4 tests), and `cargo clippy --all-targets --locked -- -D warnings` passed with the required PATH and developer directory.
