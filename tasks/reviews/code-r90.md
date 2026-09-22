+++
verdict = "MERGE"
round = "r90"
candidate = "5e6819e7552066d82be0707fe339fc1c8893e550"
manifest_hash = "254538c2c5de6e34ba8d4274c5ce79c2b49c6bc6e0dd147ba76ed15ab8821df5"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# Code review: r90

## t-0213 — MERGE

The default is now `auto`, while an explicit `propose` setting still prevents starts. The coordinator rule keeps money beyond Rolf's request, irreversible or outside-machine actions, and taste or direction with Rolf; ordinary reversible choices continue without blocking other work.

The repeat guard now compares every open and answered question independent of case, punctuation, spacing, or use of `--reask`, and its refusal names the earlier ask and its answer. I closed the same-id re-ask escape and made the rule text state the decision boundary once.

Continue prompts contain the actionable task next steps and are recorded as ledger facts. They require an idle coordinator, no pending Rolf message, no working lane, and at least one task not waiting on Rolf. A repeat also requires both the configured interval and a later coordinator context read. I added checks for working and blocked coordinator states, pending Rolf input, cooldown, and the intervening-turn requirement, and made a context read during prompt delivery count as that turn.

The requested final gate run passed.
