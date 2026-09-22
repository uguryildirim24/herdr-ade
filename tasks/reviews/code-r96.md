+++
verdict = "MERGE"
round = "r96"
candidate = "bf69e3d7d52fa51a69db81c4f6f6b6fc2159c6b5"
manifest_hash = "a73732ad79270252c5e10d90da53881483937bd898750b3905c7cf47aa660b7d"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

## t-0235

MERGE. The collector now reads `task_complete.payload.error.message`, preserves the provider's line, classifies page and stopped-thinking failures as provider failures, and keeps rate limits on the cooldown path.

The lane's packet-pair test initially loaded the whole fixture at once, so it did not prove the collector would reject the empty packet completion when the real task started after the existing 500 ms settle poll. The review fix identifies the observed 20–30 ms packet pair from its rollout timestamps and stages the fixture across that boundary. The real failed completion remains the selected turn.

The requested formatting, test, lint, and diff checks all passed.
