# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T19:00:38-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `e090742b-3a62-46fd-8bc3-73a7232d4e90`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (9 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | ce1023f | 0 | review(round): approve r117 repair |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r117` | `review/r117` | 9d2a572 | 0 | docs(tasks): t-0320 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r117-2` | `review/r117-2` | 2613f22 | 0 | docs(tasks): t-0321 |

Last commits on the integration branch:

```
ce1023f review(round): approve r117 repair
c2b0e75 Merge commit '5fc03667c291b8e9b46985bf873b59b1440cb5eb' into hp/adeherdr/t-0321-review-r117-machine-records-move-out-of
2613f22 docs(tasks): t-0321
f0f3e14 review(r117): brief for revision 1
5fc0366 review(project): convert records before later install failures
751583d checkpoint(r116): HANDOFF after merging the round
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0321.md`, `tasks/t-0320.md`, `tasks/review-r117.md`, `tasks/t-0319.md`, `tasks/review-r116.md`, `tasks/t-0317.md`, `tasks/t-0316.md`, `tasks/review-r115.md`, `tasks/t-0314.md`, `tasks/t-0313.md`, `tasks/t-0312.md`, `tasks/review-r114.md`
- verdicts: `tasks/reviews/code-r117.md`, `tasks/reviews/code-r116.md`, `tasks/reviews/code-r115.md`, `tasks/reviews/code-r114.md`, `tasks/reviews/code-r113.md`, `tasks/reviews/code-r112.md`, `tasks/reviews/code-r111.md`, `tasks/reviews/code-r110.md`, `tasks/reviews/code-r109.md`, `tasks/reviews/code-r108.md`, `tasks/reviews/code-r107.md`, `tasks/reviews/code-r106.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
ha pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume e090742b-3a62-46fd-8bc3-73a7232d4e90
```

Round `r117` was merged into `main` at verdict commit `ce1023f9271aa5ff50b41f9231f3238a0656a69a`; this checkpoint is its child.

