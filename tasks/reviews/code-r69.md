+++
verdict = "MERGE"
round = "r69"
candidate = "aeb46dc29fe3a9400b0f5639cb71de0a81d57399"
manifest_hash = "8e5f700888e3b8c40d7c199a56f4cf527f14596ef43b57e056b8c7fa407b5248"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++

# Review verdict: r69

## t-0159

MERGE. The talk screen no longer mistakes the installed path for proof that its loaded image is current. It reads the installed binary's build version on each slow poll, hands the terminal and draft to a different build, suppresses a retry of the same attempted build, and still permits a later install to take over. The focused regression covers the replaced-file case where the installed path resolves to the running executable path. No review fix was needed.

The round brief defines no gates. At the coordinator's request I also ran formatting, the full test suite, clippy with warnings denied, and the diff whitespace check on the candidate; all passed.
