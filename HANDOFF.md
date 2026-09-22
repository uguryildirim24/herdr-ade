# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T03:07:35-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (blocked)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (7 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 959bed0 | 0 | docs(review): round r102 repair verdict |
| `/home/agent/projects/herdr-ade/.worktrees/review-r100` | `review/r100` | 97f81cd | 0 | docs(tasks): t-0256 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r102` | `review/r102` | 91fe439 | 0 | docs(tasks): t-0267 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r102-2` | `review/r102-2` | 6671177 | 0 | docs(tasks): t-0268 |

Last commits on the integration branch:

```
959bed0 docs(review): round r102 repair verdict
958c1d1 Merge commit '0ec88a01d89f66e0be3105e55e15ab0da5a376d2' into hp/adeherdr/t-0268-review-r102-a-helper-that-lost-its-conne
6671177 docs(tasks): t-0268
12bca15 review(r102): brief for revision 1
17bf688 checkpoint(r98): HANDOFF after merging the round
6505c43 Merge commit '82382bfbe3bd63227d1c2fddfb2d2c0a83aff90e'
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0268.md`, `tasks/t-0267.md`, `tasks/review-r102.md`, `tasks/t-0265.md`, `tasks/t-0263.md`, `tasks/t-0255.md`, `tasks/t-0252.md`, `tasks/t-0245.md`, `tasks/t-0266.md`, `tasks/review-r98.md`, `tasks/t-0264.md`, `tasks/t-0261.md`
- verdicts: `tasks/reviews/code-r102.md`, `tasks/reviews/code-r98.md`, `tasks/reviews/code-r99.md`, `tasks/reviews/code-r97.md`, `tasks/reviews/code-r96.md`, `tasks/reviews/code-r92.md`, `tasks/reviews/code-r95.md`, `tasks/reviews/code-r93.md`, `tasks/reviews/code-r94.md`, `tasks/reviews/code-r91.md`, `tasks/reviews/code-r90.md`, `tasks/reviews/code-r88.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r102` was merged into `main` at verdict commit `959bed0101881cc059a20f08348cb38d9344a827`; this checkpoint is its child.

