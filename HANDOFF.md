# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T19:28:48-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `e090742b-3a62-46fd-8bc3-73a7232d4e90`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (6 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 69b8014 | 0 | review(round): approve r118 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r118` | `review/r118` | dc90a80 | 0 | docs(tasks): t-0323 |

Last commits on the integration branch:

```
69b8014 review(round): approve r118
a20714e merge: t-0322 into review r118
dc90a80 docs(tasks): t-0323
779767e review(r118): brief for revision 1
3bbe951 refactor(project): remove completed conversion path
3e44ec8 docs(tasks): t-0322
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0323.md`, `tasks/review-r118.md`, `tasks/t-0322.md`, `tasks/t-0321.md`, `tasks/t-0320.md`, `tasks/review-r117.md`, `tasks/t-0319.md`, `tasks/review-r116.md`, `tasks/t-0317.md`, `tasks/t-0316.md`, `tasks/review-r115.md`, `tasks/t-0314.md`
- verdicts: `tasks/reviews/code-r118.md`, `tasks/reviews/code-r117.md`, `tasks/reviews/code-r116.md`, `tasks/reviews/code-r115.md`, `tasks/reviews/code-r114.md`, `tasks/reviews/code-r113.md`, `tasks/reviews/code-r112.md`, `tasks/reviews/code-r111.md`, `tasks/reviews/code-r110.md`, `tasks/reviews/code-r109.md`, `tasks/reviews/code-r108.md`, `tasks/reviews/code-r107.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
ha pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume e090742b-3a62-46fd-8bc3-73a7232d4e90
```

Round `r118` was merged into `main` at verdict commit `69b8014a111969bc0db33299a980d2f9732a7c2a`; this checkpoint is its child.

