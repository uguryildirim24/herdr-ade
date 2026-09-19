+++
verdict = "MERGE"
round = "r19"
candidate = "d12791a788713f6d32020f3a7e20179f69f8e4fb"
manifest_hash = "6a8e043f2d3e9fdeed9cfbdc9884e21b16a0d9d5547ad42345b6c8aebf1c68cd"
policy_hash = "7cf667dea9748fe2844267e971e613ff74b94a3ef0af9df5ee974d759cb7e9da"
gates = []
+++

# Round r19 review

MERGE. The frozen manifest remains revision 1 and still contains only t-0044 at the pinned sha. The brief commit and pinned lane sha are ancestors of the candidate.

## t-0044

The change fixes the failing cloud-box start at the right boundary. `check_on_machine` now invokes the box's dedicated `herdr-pi check <provider>` binary while keeping `HERDR_ADE_ROOT` pointed at the box's ADE root, so readiness uses the box's setup and login store. The existing SSH quoting and failure propagation remain intact. The new constant and operations note make the split from `herdr-ade` explicit. I found no other remote pi verb routed through the wrong binary and made no review fix.

## Gates and focused proof

The round lists no gates, so `gates = []`.

I ran the two defect-specific tests added by the lane (these are focused review proofs, not round gates):

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked the_box_readiness_check_calls_the_pi_binary_with_the_provider
running 1 test
test pi_ade::tests::the_box_readiness_check_calls_the_pi_binary_with_the_provider ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 379 filtered out
```

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked a_box_readiness_refusal_surfaces_the_box_stderr
running 1 test
test pi_ade::tests::a_box_readiness_refusal_surfaces_the_box_stderr ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 379 filtered out
```
