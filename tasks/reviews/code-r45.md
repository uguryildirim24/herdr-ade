+++
verdict = "MERGE"
round = "r45"
candidate = "c36e12ce58788028023ac444fcfdd602110bfdfb"
manifest_hash = "99d15a741f3f58461789a5ad6ee4cd87dc840a62b00debe3f2583cc23f0c2c41"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = []
+++

# Round r45 review

## Verdict and lane t-0097

MERGE after fixes. Merged pinned `93c56098d3166ef57234640b433302c53e4fd2c8`; the brief commit and pinned lane are ancestors of the candidate. No integration branch was moved.

Thread/round inbox projections are removed and old files are ignored without migration. Context reads their owning records and sealed evidence. Courier imports retain one replay-safe delivery message, written before advancing the cursor. Local completion wake-ups remain independent of inbox messages. Tests cover delivery, ignored legacy kinds, PR metadata, lineage, round actions and exact coordinator-bound context receipts.

Review fixes:
- Preserved r43's escalation tick while resolving the merge conflict.
- A sealed completion hid a different, later report. Context now suppresses the report line only when its hash matches the sealed artifact. The regression failed before the fix and passes afterward.
- r43 added `failed` events after this lane's base. Context previously acknowledged that evidence without printing it. It now prints the failure and tests its peek/wrong-pane/acknowledgement behavior. The regression failed before the fix and passes afterward.
- Corrected the manual-test row that still promised PR inbox messages.

## Checks on oci

The round brief lists no gates; its front-matter gate list remains empty. Additional checks below ran against the final candidate. Cargo used the inherited `CARGO_TARGET_DIR=/home/ubuntu/build/lanes/adeherdr-t-0100`.

```text
PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(no output; exit 0)

PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test
test result: ok. 491 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 27.03s
test result: ok. 56 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.81s
test result: ok. 78 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.83s
test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.87s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.07s
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
(exit 0; 635 total)

PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets -- -D warnings
Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.82s
(exit 0)

git diff --check
(no output; exit 0)

git diff --check 1421976..HEAD
tasks/t-0100.md:1952: trailing whitespace.
(exit 2; generated task brief contains whitespace in its embedded diff)

git diff --check ee6a42f..HEAD
(no output; exit 0; checks all review changes after the supplied task commit)

python3 scripts/context-size.py /tmp/t0097-before-bin /home/ubuntu/build/lanes/adeherdr-t-0100/debug/herdr-ade /tmp/r45-context
before: 28853 bytes, 314 lines, 3004 whitespace words
after: 11161 bytes, 211 lines, 1497 whitespace words
(exit 0)
```

The size check reuses the lane's saved baseline binary and runs this candidate against the same synthetic fixture: 61.3% fewer bytes. It is not a live Mac inbox measurement. Logs, failing regression output and both context outputs accompany the lane report under `library/`.

No install or live Mac tests ran. `ha round show adeherdr r45` on this box returns `round_manifest_unavailable`: the box has lane bootstrap state, not the coordinator's authoritative round record. The verdict uses the committed brief's hashes; live manifest/policy freshness must still pass the coordinator's merge validation.
