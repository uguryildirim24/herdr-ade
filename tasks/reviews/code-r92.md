+++
verdict = "MERGE"
round = "r92"
candidate = "41cc0a0e67eea2b15848093b07eaf1621bd860e3"
manifest_hash = "f17bac1fbdb5a5af50147fe10d6e98811b15518661183a2180441fb05752aa05"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++

# Round r92 repair review

Verdict: **MERGE**.

## t-0222: reliable install and process evidence

The earlier repaired candidate merged automatically onto the W15 base with no conflicts or unmerged paths. The ticker proof still accepts only a complete build/PID snapshot, same-commit clean installs still preserve the installed inode, and later unknown process evidence does not erase a job's earlier verification.

## W15 integration base

The machine-kind changes remain intact: Claude and agy stay on the Mac, pi work can still use a declared ready box, and doctor limits box checks to the kinds that machine declares.

All requested checks passed. The detailed command exits and final lines are in `.herdr-project/adeherdr-t-0241/report.md`.
