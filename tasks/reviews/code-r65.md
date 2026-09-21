+++
verdict = "MERGE"
round = "r65"
candidate = "b7ba243625b140a3704cdd1d0e9503094cd415fb"
manifest_hash = "1453d0bf8b32a53ebd6e55626137f046682d436954e76999ff62566df36b825a"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++

## t-0150

MERGE. The shared `RoundRecord::carries` rule preserves historical admissions while releasing abandoned members. Plan derivation now ignores the abandoned r59-style membership once the lane lands in a later round, and the regression exercises that full abandon, re-admit, land, and derive path. The talk overview uses the same rule; cost and board projections no longer present an abandoned round as current work. Context already omits closed rounds, and doctor has no round-membership projection to change.

No project gates were listed for this review.
