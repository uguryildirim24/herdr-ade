+++
verdict = "MERGE"
round = "r99"
candidate = "cbae829cb427699289847bb7d5a21d918ae71ef8"
manifest_hash = "c9c7725ea179599cad1819955feb29a58f3980e15ab2ad3ba566a09b92ccaba9"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# W25 review

MERGE. Both generated box repository scripts rebuild only the ignored local index from `HEAD`, refresh its stat information, and do so before reading the source commit, checking dirtiness, or building. The working tree and refs are untouched. A clean working tree over a deliberately stale index becomes clean, while a genuinely modified tracked file remains modified and is reported by `git status`.

The requested formatting, test, lint, and diff checks pass.
