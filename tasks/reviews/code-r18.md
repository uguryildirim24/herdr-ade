+++
verdict = "MERGE"
round = "r18"
candidate = "e6aadab66262d38c06f7f465dc12d67d830f5676"
manifest_hash = "041fa869b066d3a36953fb6e63ac611cd510c73adf637015df18f0357482db96"
policy_hash = "7cf667dea9748fe2844267e971e613ff74b94a3ef0af9df5ee974d759cb7e9da"
gates = []
+++

MERGE.

## t-0041

The lane adds the durable plan card, automatic step projection, decision history with authority references and replacements, the three-open-question cap, and verified round landing lines. The command and record shapes match SPEC-talk v2's first build lane, old optional fields still load, and the coordinator and operations text state the authority boundary.

I added `review(plan): keep record transitions automatic`. Plan mutations now refresh bound step states in the same atomic write and unchanged mutations no longer consume a revision. Re-asking an older open question is refused so only the newest can absorb a merged question. A repeated merge now reconciles landing evidence and plan state after a process dies between the durable checkpoint and those follow-ups.

The round lists no gates. As review checks, `cargo test --locked` passed 378 + 50 + 78 + 4 tests, and `cargo clippy --all-targets --locked -- -D warnings` completed cleanly.
