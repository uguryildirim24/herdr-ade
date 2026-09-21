+++
verdict = "MERGE"
round = "r72"
candidate = "85d94bf4dee7cc42ae99bcb7bb7f2f770e2bfa2b"
manifest_hash = "aa68f5e79be6feb79f184905d5fa03f9686280ca2443ffaa503302334d65dfd7"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++

## t-0165

MERGE. The pinned change removes score-based helper selection and its hosted picker, replaces it with ordered editable routing and bounded recovery, and removes the Claude-only coordinator restriction.

Review fixes make a missing routing table fail doctor and launch with the exact configuration remedy, verify old launch/dispatch/round fields still deserialize, and remove the remaining old routing-file references from source and current documentation.

The requested formatting, test, clippy, and diff checks pass on the candidate. The source/documentation scan for the removed picker, its key, and its old routing file is clean.
