# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T18:01:50-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `e090742b-3a62-46fd-8bc3-73a7232d4e90`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (9 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 9a0440f | 0 | review(round): approve r115 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r115` | `review/r115` | 2455729 | 0 | docs(tasks): t-0315 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r115-2` | `review/r115-2` | ae57b5d | 0 | docs(tasks): t-0316 |

Last commits on the integration branch:

```
9a0440f review(round): approve r115
068cca4 Merge pinned lane t-0313 into review r115
ae57b5d docs(tasks): t-0316
99e2573 review(r115): brief for revision 5
49575d9 review(r115): brief for revision 4
f9e8aa8 fix(round): resume reviews after follow-up completion
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0316.md`, `tasks/review-r115.md`, `tasks/t-0314.md`, `tasks/t-0313.md`, `tasks/t-0312.md`, `tasks/review-r114.md`, `tasks/t-0310.md`, `tasks/t-0309.md`, `tasks/t-0308.md`, `tasks/t-0307.md`, `tasks/review-r113.md`, `tasks/t-0306.md`
- verdicts: `tasks/reviews/code-r115.md`, `tasks/reviews/code-r114.md`, `tasks/reviews/code-r113.md`, `tasks/reviews/code-r112.md`, `tasks/reviews/code-r111.md`, `tasks/reviews/code-r110.md`, `tasks/reviews/code-r109.md`, `tasks/reviews/code-r108.md`, `tasks/reviews/code-r107.md`, `tasks/reviews/code-r106.md`, `tasks/reviews/code-r105.md`, `tasks/reviews/code-r104.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
ha pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume e090742b-3a62-46fd-8bc3-73a7232d4e90
```

Round `r115` was merged into `main` at verdict commit `9a0440f595c475d47c3da8afed5ca44f24488d46`; this checkpoint is its child.

