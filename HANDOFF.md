# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T23:47:22-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (10 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 7f9bfa8 | 0 | review(autonomy): verdict for r90 revision 3 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r90` | `review/r90` | f3e6464 | 0 | docs(tasks): t-0216 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r90-2` | `review/r90-2` | bab4c62 | 0 | docs(tasks): t-0219 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r90-3` | `review/r90-3` | 26d5081 | 0 | docs(tasks): t-0221 |

Last commits on the integration branch:

```
7f9bfa8 review(autonomy): verdict for r90 revision 3
e792f5e Merge commit '3dc1c87dface7381ea02daff5c8940bd8a119d9f' into hp/adeherdr/t-0221-review-r90-every-coordinator-keeps-worki
26d5081 docs(tasks): t-0221
8249438 review(r90): brief for revision 1
97140dd checkpoint(r88): HANDOFF after merging the round
e6ad477 review(r88): approve candidate on W11 base
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0221.md`, `tasks/t-0219.md`, `tasks/t-0216.md`, `tasks/review-r90.md`, `tasks/t-0220.md`, `tasks/t-0217.md`, `tasks/t-0214.md`, `tasks/review-r88.md`, `tasks/t-0218.md`, `tasks/t-0215.md`, `tasks/review-r89.md`, `tasks/t-0212.md`
- verdicts: `tasks/reviews/code-r90.md`, `tasks/reviews/code-r88.md`, `tasks/reviews/code-r89.md`, `tasks/reviews/code-r87.md`, `tasks/reviews/code-r86.md`, `tasks/reviews/code-r82.md`, `tasks/reviews/code-r85.md`, `tasks/reviews/code-r84.md`, `tasks/reviews/code-r83.md`, `tasks/reviews/code-r81.md`, `tasks/reviews/code-r80.md`, `tasks/reviews/code-r79.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r90` was merged into `main` at verdict commit `7f9bfa8e806545d951a72bef886e61bcbe6e0e54`; this checkpoint is its child.

