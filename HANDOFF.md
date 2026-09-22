# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T04:59:57-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (6 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 4b8bebd | 0 | docs(review): round r105 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r105` | `review/r105` | 7fc67a6 | 0 | docs(tasks): t-0274 |

Last commits on the integration branch:

```
4b8bebd docs(review): round r105 verdict
76c95e1 Merge commit '6592b3423b1e3c296bec7f483eb2a0ec1ddaa1f8' into review/r105
7fc67a6 docs(tasks): t-0274
555ea0f review(r105): brief for revision 1
6592b34 fix(decide): report class requirements together
0405a2f docs(tasks): t-0273
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0274.md`, `tasks/review-r105.md`, `tasks/t-0273.md`, `tasks/t-0272.md`, `tasks/review-r104.md`, `tasks/t-0271.md`, `tasks/t-0270.md`, `tasks/t-0256.md`, `tasks/review-r100.md`, `tasks/t-0269.md`, `tasks/review-r103.md`, `tasks/t-0268.md`
- verdicts: `tasks/reviews/code-r105.md`, `tasks/reviews/code-r104.md`, `tasks/reviews/code-r100.md`, `tasks/reviews/code-r103.md`, `tasks/reviews/code-r102.md`, `tasks/reviews/code-r98.md`, `tasks/reviews/code-r99.md`, `tasks/reviews/code-r97.md`, `tasks/reviews/code-r96.md`, `tasks/reviews/code-r92.md`, `tasks/reviews/code-r95.md`, `tasks/reviews/code-r93.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r105` was merged into `main` at verdict commit `4b8bebdb7600857c21e5f3e9e68646cc35b31fd0`; this checkpoint is its child.

