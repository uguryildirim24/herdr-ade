+++
verdict = "MERGE"
round = "r22"
candidate = "d287c4b2a8bab61808ed342b643c9e4abdb7a794"
manifest_hash = "e0585873ac1b7df0865cfc55c942c9022dcf389cca0c7e70386ed76aa751bb36"
policy_hash = "b935a15b66c060eb5d410761c6c8d496171d6407c95b458c7630f137eafe6b1b"
gates = []
+++

# Round r22

## t-0051 — a finished lane always wakes the coordinator

MERGE. Delivery now keys retry suppression on the transport fact, `submitted`, rather than treating any journal entry as proof that the wake-up line was sent. An event read or handled before transport is repaired by typing its line and appending `submitted`; an already submitted event is not retyped. Ready-pane, recipient-binding, stale-attempt, and remote-publish checks remain in the existing delivery path.

The writer audit is consistent with the fix: the coordinator's `context` command is the only production caller that appends `acknowledged`, while `inbox done` appends `handled`; the automatic round path does neither. The focused tests cover the observed acknowledged-before-submitted sequence, ordinary one-time delivery, and verdict advancement without acknowledgement. The operations text now distinguishes the typed wake-up from the durable inbox record.

I found no review defect and added no review commit. The round froze no PROJECT.md gates, hence `gates = []`. The lane's four requested checks were also rerun on the merged candidate: formatting, 527 tests, clippy with warnings denied, and the release build all passed.
