# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T02:48:39-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (blocked)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (16 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 6505c43 | 0 | Merge commit '82382bfbe3bd63227d1c2fddfb2d2c0a83aff90e' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r100` | `review/r100` | 97f81cd | 0 | docs(tasks): t-0256 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r102` | `review/r102` | 91fe439 | 0 | docs(tasks): t-0267 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r98` | `review/r98` | 5e45996 | 0 | docs(tasks): t-0245 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r98-2` | `review/r98-2` | 63bb915 | 0 | docs(tasks): t-0252 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r98-3` | `review/r98-3` | d7ab258 | 0 | docs(tasks): t-0255 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r98-4` | `review/r98-4` | 081352c | 0 | docs(tasks): t-0263 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r98-5` | `review/r98-5` | 6564ca5 | 0 | docs(tasks): t-0265 |

Last commits on the integration branch:

```
6505c43 Merge commit '82382bfbe3bd63227d1c2fddfb2d2c0a83aff90e'
30eb805 review(r102): brief for revision 1
34b9704 docs(tasks): t-0266
82382bf docs(review): round r98 repair verdict
fb00327 Merge commit '6c9a22640a61a0e32493bafa7d327d2dbe6c8f15' into hp/adeherdr/t-0265-review-r98-finished-work-leaves-nothing
6564ca5 docs(tasks): t-0265
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0265.md`, `tasks/t-0263.md`, `tasks/t-0255.md`, `tasks/t-0252.md`, `tasks/t-0245.md`, `tasks/review-r102.md`, `tasks/t-0266.md`, `tasks/review-r98.md`, `tasks/t-0264.md`, `tasks/t-0261.md`, `tasks/t-0259.md`, `tasks/t-0258.md`
- verdicts: `tasks/reviews/code-r98.md`, `tasks/reviews/code-r99.md`, `tasks/reviews/code-r97.md`, `tasks/reviews/code-r96.md`, `tasks/reviews/code-r92.md`, `tasks/reviews/code-r95.md`, `tasks/reviews/code-r93.md`, `tasks/reviews/code-r94.md`, `tasks/reviews/code-r91.md`, `tasks/reviews/code-r90.md`, `tasks/reviews/code-r88.md`, `tasks/reviews/code-r89.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r98` was merged into `main` at verdict commit `82382bfbe3bd63227d1c2fddfb2d2c0a83aff90e`; this checkpoint is its child.

