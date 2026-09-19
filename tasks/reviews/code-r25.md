+++
verdict = "MERGE"
round = "r25"
candidate = "93f9731f663750867eba080723cbd8a86b223822"
manifest_hash = "9f3600cb0171cd15429d3bf9c481253c092a764c3c2dbdc7a1064514bb24ef7e"
policy_hash = "941e9ea13a16368ca1ee73b91e4a1dc137816aed5c9c2f6e7e6acea90d78dfbd"
gates = []
+++

# Round r25 verdict

## t-0054

MERGE. The lane adds the shared harness repository list, lets every project open harness work and rounds, adds the locked Mac-and-box installer, and updates the coordinator skill and operations guide with Rolf's self-evolving-harness rule.

Review fixed two release blockers. Box lanes had kept the old warning-only path and could still start on an unlisted repository; all local and box starts now use the same allowlist. The installer copied directly over running executables, which can fail on the box's running `herdr` image and can expose partial bytes. It now stages and atomically renames every binary on both machines. Machine-list and installed-version failures also fail closed, timed-out local builds kill their process group, and a resumed no-op merge repeats the install instruction.

The refreshed candidate also keeps round r24's role-default box placement, local fallback, and strict explicit machine choice, plus round r26's box-lane pickup and machine-qualified parent links. The only second-merge conflict was the unreachable-box test stub; it now uses r26's precise SSH readiness matcher while preserving the fallback behavior.

No project gates were listed. Additional formatting, 547 tests, lint, and release-build checks passed; details are in the review report.
