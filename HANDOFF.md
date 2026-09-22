# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T00:20:05-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (13 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 4a772b4 | 0 | Merge commit 'bc988eec744e8f53d2d31c63d81bd476285ce3ed' |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r91` | `review/r91` | 9d5509a | 0 | docs(tasks): t-0227 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r92` | `review/r92` | f4cbd52 | 0 | docs(tasks): t-0228 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r93` | `review/r93` | 345f901 | 0 | docs(tasks): t-0229 |

Last commits on the integration branch:

```
4a772b4 Merge commit 'bc988eec744e8f53d2d31c63d81bd476285ce3ed'
bc988ee docs(review): round r91 verdict
1bdc76b review(r93): brief for revision 1
e37b427 review(r92): brief for revision 1
abb53f6 Merge commit 'ced57749e87035275d0c66dab9a9ee77529005ac' into review/r91
9d5509a docs(tasks): t-0227
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0227.md`, `tasks/review-r93.md`, `tasks/review-r92.md`, `tasks/review-r91.md`, `tasks/t-0226.md`, `tasks/t-0225.md`, `tasks/t-0224.md`, `tasks/t-0223.md`, `tasks/t-0222.md`, `tasks/t-0221.md`, `tasks/t-0219.md`, `tasks/t-0216.md`
- verdicts: `tasks/reviews/code-r91.md`, `tasks/reviews/code-r90.md`, `tasks/reviews/code-r88.md`, `tasks/reviews/code-r89.md`, `tasks/reviews/code-r87.md`, `tasks/reviews/code-r86.md`, `tasks/reviews/code-r82.md`, `tasks/reviews/code-r85.md`, `tasks/reviews/code-r84.md`, `tasks/reviews/code-r83.md`, `tasks/reviews/code-r81.md`, `tasks/reviews/code-r80.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r91` was merged into `main` at verdict commit `bc988eec744e8f53d2d31c63d81bd476285ce3ed`; this checkpoint is its child.

