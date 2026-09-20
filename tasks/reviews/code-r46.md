+++
verdict = "MERGE"
round = "r46"
candidate = "77927e344c7c7dd23d4586061f3184fa4895eb2d"
manifest_hash = "e48bd44c35732ec687584207a021234657e852b32e9a7e8252caf8886c68b3d9"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = []
+++

# Round r46 review

## Verdict: MERGE — t-0099

Merged pinned commit `40c93cd78bcd636b12ad2a2f2945b57b4213c288`. Both the pin and brief commit `d34308c` are ancestors of the candidate. No integration branch was moved.

The serialized request is bounded at both construction and transport. Repository evidence is removed before failure evidence, the complete task is preserved or explicitly refused, and omissions are disclosed to the scorer and ledger. Large reviewer sources remain accessible through the committed brief and named diff ranges. HTTP refusals retain status and bounded, key-redacted text. Round abandonment records its reason and releases the branch reservation without permitting cancellation of a begun merge.

Review fixes:
- Curl normally reaches its own deadline before the runner watchdog. Exit 28 was incorrectly classified as `jev_transport`; it now reports `jev_timeout`. The expanded regression failed before the fix and passes afterward.
- The error-text ellipsis previously exceeded the 4 KiB ceiling by three bytes. It now fits inside the ceiling, with multibyte, control-character and key-redaction coverage.
- Added checks that abandonment refuses blank reasons, cannot replace a closed round's reason, and cannot cancel a pending merge transaction.

## Checks on oci

The brief specifies no gates, so its front-matter gate list remains empty. Additional checks ran against the final candidate tree, using inherited `CARGO_TARGET_DIR=/home/ubuntu/build/lanes/adeherdr-t-0101`:

```text
PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(no output; exit 0)

PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test
test result: ok. 498 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 27.30s
test result: ok. 56 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.81s
test result: ok. 78 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.83s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.83s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
(exit 0; 642 tests)

PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets --all-features -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 15.39s
(exit 0)

git diff --check
(no output; exit 0)
```

An initial formatting check and clippy run caught formatting and test-module placement in my additions; both were corrected before the final successful runs above. Logs and the failing timeout regression accompany the lane report under `library/`.

No live authenticated TypeSafe measurement or install ran. The 256 KiB cap is a conservative margin below the reported failing ~750 KiB request, not an independently measured endpoint maximum. Tests prove the local cap, not server acceptance of every request below it.

The cloud bootstrap state does not contain the authoritative r46 round record. This verdict uses the committed brief's hashes; the coordinator's merge validation must still check live manifest and policy freshness.
