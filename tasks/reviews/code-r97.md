+++
verdict = "MERGE"
round = "r97"
candidate = "2401e06bded923ed076a19b6b0e3de2e4a6975c5"
manifest_hash = "0529d1ff5cff1bcaf3a7aa46e84e7dfeb806e6a5ce13135b728c0567a09a1745"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

## t-0239

MERGE. Drop evidence is dated and persisted on the task record, and old records load through the task's serde defaults. The dropped projection takes precedence over every delivery milestone, is terminal without a next step, is excluded from plan completion, install proof, and ticker nudges, and is rendered with its reason in all requested views. Verification evidence prevents the drop.

The lane's new test initially used the pre-merge `task::add` signature. The review candidate adds the missing replacement-authority argument. All requested checks then passed.
