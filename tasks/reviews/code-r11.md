+++
verdict = "MERGE"
round = "r11"
candidate = "61adf51d7b9c1c3905306adad2253968efe24fd6"
manifest_hash = "8314f37b36b576cc062f8436b96702f738b2bd4b07f4cee1d5c85b92c4f31a5a"
policy_hash = "e1529634be8dfdf8688cad0bfcfd99dfa67f7f19fe2d1c3c7ab9f12ee7c0eec3"
gates = []
+++

# Round r11

MERGE.

## t-0020

The relay makes Pro a plain pi lane while Codex remains behind it. It binds only to loopback, leaves `/healthz` open, protects every other route with the token from `serve.json`, and invokes one `codex exec --json` process per accepted request with the Pro home and bridge route.

Session affinity resumes the Codex thread selected by conversation, pi session header, or the instruction-and-lane fallback. The Responses event order works with pi; a fake Codex failure returned promptly instead of hanging. Same-session and overall admission return 429, two failures set the breaker, and stop removes `serve.json`.

Review fixes closed an unauthenticated-health mismatch, an overall-admission race, child-process leaks on disconnected streams, false success on truncated Codex output, secret file modes, startup readiness/config failures, and swallowed provider-merge errors. The old Codex-pane route is unchanged. The plugin startup path, merged `pro` provider, and four-flag `pi_pro` recipe are correct.

The round listed no gates. The reviewer additionally ran format, all tests, clippy with warnings denied, a release build, and the full fake-Codex curl/pi proof without using the real Pro state or sending a real turn.
