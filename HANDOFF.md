# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T18:38:11-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `e090742b-3a62-46fd-8bc3-73a7232d4e90`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (11 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | e8620ec | 0 | Merge commit '450c34c5d1c274fd8acc1155caf190aeb70a6980' |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r116` | `review/r116` | 359ca1d | 0 | docs(tasks): t-0318 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r116-2` | `review/r116-2` | 5ac462c | 0 | docs(tasks): t-0319 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r117` | `review/r117` | 9d2a572 | 0 | docs(tasks): t-0320 |

Last commits on the integration branch:

```
e8620ec Merge commit '450c34c5d1c274fd8acc1155caf190aeb70a6980'
450c34c review(round): approve r116
73fa92e Merge pinned lane t-0317 into review r116
2819ce3 review(r117): brief for revision 1
5ac462c docs(tasks): t-0319
3533d19 review(r116): brief for revision 2
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0319.md`, `tasks/review-r117.md`, `tasks/review-r116.md`, `tasks/t-0317.md`, `tasks/t-0316.md`, `tasks/review-r115.md`, `tasks/t-0314.md`, `tasks/t-0313.md`, `tasks/t-0312.md`, `tasks/review-r114.md`, `tasks/t-0310.md`, `tasks/t-0309.md`
- verdicts: `tasks/reviews/code-r116.md`, `tasks/reviews/code-r115.md`, `tasks/reviews/code-r114.md`, `tasks/reviews/code-r113.md`, `tasks/reviews/code-r112.md`, `tasks/reviews/code-r111.md`, `tasks/reviews/code-r110.md`, `tasks/reviews/code-r109.md`, `tasks/reviews/code-r108.md`, `tasks/reviews/code-r107.md`, `tasks/reviews/code-r106.md`, `tasks/reviews/code-r105.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
ha pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume e090742b-3a62-46fd-8bc3-73a7232d4e90
```

Round `r116` was merged into `main` at verdict commit `450c34c5d1c274fd8acc1155caf190aeb70a6980`; this checkpoint is its child.

