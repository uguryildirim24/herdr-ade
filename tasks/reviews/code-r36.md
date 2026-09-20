+++
verdict = "MERGE"
round = "r36"
candidate = "735876a20f643623fccb6bd4dfab4e5d2d7b6043"
manifest_hash = "ade407d8e969b170791c24266a71e01aab3a38db4489b60d1b74169279da6119"
policy_hash = "3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174"
gates = []
+++

# Round r36 review

## t-0079

The lane found the lock-order cycle behind the silent reviewer failures. Review starts now use a non-blocking ticker check while `round advance` holds its lock, failed starts print their reason, dead bindings are removed, and the next pass retries up to a fixed bound. The new tests cover a refused start, a reviewer that never launched, the retry bound, and the deadlock property.

I fixed two integration issues. Only the review-start path now skips stale-ticker replacement; ordinary `thread start` keeps replacing a stale ticker as documented. A new review revision also resets the earlier revision's failed-start count, so an old exhausted review cannot disable automatic starts forever.

No project gates were listed. Additional verification was clean: `cargo fmt --check`, clippy with warnings denied, and the full test suite (447 + 56 + 78 + 4 tests).
