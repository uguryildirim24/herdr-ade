+++
verdict = "MERGE"
round = "r81"
candidate = "cf33840ddb587874486942da88646a727f6f576d"
manifest_hash = "467fe12ac87ae8cc68bf85072e21ba7c0960af753c2dac8b5f2c48a1f0dae757"
policy_hash = "3ff3234b04a6b9438ce5a97e9ebb249f4698636f90353f2a3946112ef9d3e1d2"
gates = []
+++

The pinned lane makes helpers, recipes, and machine paths declarative. Review fixes remove the remaining core checks for named agent kinds, route special readiness and blocked-prompt behavior through adapter fields, and make remote probes, courier recovery, installation, and build cleanup consume machine declarations.

The live agy start failure is fixed: its probe uses only agy's print flags and the full routed argv is pinned by a test. Claude's unsupported `--max-turns` probe flag is also gone, Cursor names its real executable, and unsupported-flag output stays an unknown local fault.

Doctor probes the routed provider/model pairs and now names the required live configuration migration exactly: replace `requires_claude = true` with `capability = "native-chat"` in the existing Claude routing rule.
