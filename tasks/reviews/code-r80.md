+++
verdict = "MERGE"
round = "r80"
candidate = "f0426970a2402eb21dfa9adc51530f1789926a1e"
manifest_hash = "14e1c86e863f9c767e5b7f759e05166f132e1697da16657a2401257674a408ac"
policy_hash = "3ff3234b04a6b9438ce5a97e9ebb249f4698636f90353f2a3946112ef9d3e1d2"
gates = []
+++

## t-0190

The install flow now re-executes the replaced plugin with the original arguments, records exact repository commits only after a stable build, and leaves the Herdr server running for Rolf's separate live-handoff decision. Installed task evidence is restricted to automatic install results whose merged round is in that machine's build; manual task evidence can record verification only. Historical thread adoption rejects reuse and repository mismatches.

The review repaired ticker replacement so the new image initializes before the old ticker is stopped, waits for the replacement to hold the projects-root lock, and keeps failures explicit. It also limited running evidence to machines carrying the task and added an unreachable-box check that reports unknown without a build.

The requested checks pass on `oci`, including a live stale-ticker handoff probe. The full Rust suites, formatting, lint, and diff checks pass.
