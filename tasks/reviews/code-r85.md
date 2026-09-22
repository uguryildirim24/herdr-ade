+++
verdict = "MERGE"
round = "r85"
candidate = "defffa31d738c770f183b490660695a965395ca2"
manifest_hash = "a334ee4e5e45239cf71531e76e1d117efa1e64239e5b21d4360469055587129d"
policy_hash = "d523603c07eaba1d3f8e4dbae733f1c5746755d35294ffa5fa08cfe24ff202b3"
gates = []
+++

`t-0200` correctly resolves the saved profile by stable id and its machine declaration by label, supplies the declared PATH to box-side `herdr`, `herdr-ade`, `herdr-pi`, `cargo`, and `git` commands, imports sealed events through the courier, preserves historical records without `machine_id`, and reserves build folders for every unresolved lane.

The review fixed remaining identity and recovery gaps: repository and cleanup paths now use the resolved profile label; doctor discovers active machines by stable route; local lookup faults leave no unreachable view and clear stale false connection failures; any successful courier clears persisted lost-connection state after a ticker restart; and the displayed box restart command carries the declared PATH. Regression coverage proves lookup faults remain unknown and successful collection repairs stale connection state.

All requested checks passed. The candidate is ready to merge.
