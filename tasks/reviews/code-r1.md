+++
verdict = "MERGE"
round = "r1"
candidate = "5dcf62466512331223c4e74b1ba89e690aec6faf"
manifest_hash = "ac4e2fcefc536e1909bb85d7f143308ebdd1a61b2bec5cce0936e8875ab2bf9b"
policy_hash = "acfaae9ab98bf4e0b0925f47cd4a8cc784944e12bbd9b9caae384ce29bfd3ff4"
gates = []
+++

# Round r1 review

## t-0001: word check fix

MERGE. The larger SCOWL list admits the ordinary words named in the incident, while the plain checker still rejects an unknown fixture word. A failed reply now blocks once per turn and then emits the fixed exhaustion notice; the translator rewrite path is gone. The rejection gives the exact `ha term add` command.

The review cleaned the three current Clippy findings without changing behavior: the test-only talk record is test-only, file ordering uses the suggested key sort, and Cursor flag validation uses a guarded match arm. It also removed the coordinator approval-restriction bullet exactly as Rolf asked.

The brief listed no gates. Supplementary checks passed: formatting, Clippy with warnings denied, all 431 tests, and the release build.
