+++
verdict = "MERGE"
round = "r25"
candidate = "02ec12e1ab3fb598ab1552fc04cd751b72899a34"
manifest_hash = "9f3600cb0171cd15429d3bf9c481253c092a764c3c2dbdc7a1064514bb24ef7e"
policy_hash = "941e9ea13a16368ca1ee73b91e4a1dc137816aed5c9c2f6e7e6acea90d78dfbd"
gates = []
+++

# Round r25 verdict

## t-0054

MERGE. The lane adds the shared harness repository list, lets every project open harness work and rounds, adds the locked Mac-and-box installer, and updates the coordinator skill and operations guide with Rolf's self-evolving-harness rule.

Review fixed two release blockers. Box lanes had kept the old warning-only path and could still start on an unlisted repository; all local and box starts now use the same allowlist. The installer copied directly over running executables, which can fail on the box's running `herdr` image and can expose partial bytes. It now stages and atomically renames every binary on both machines. Machine-list and installed-version failures also fail closed, timed-out local builds kill their process group, and a resumed no-op merge repeats the install instruction.

No project gates were listed. Additional formatting, test, lint, and release-build checks passed; details are in the review report.
