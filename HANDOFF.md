# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T22:28:00-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (7 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | c50263a | 0 | review(r86): approve repaired candidate |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r86` | `review/r86` | 25dae55 | 0 | docs(tasks): t-0208 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r86-2` | `review/r86-2` | 4579086 | 0 | docs(tasks): t-0209 |

Last commits on the integration branch:

```
c50263a review(r86): approve repaired candidate
274f2e3 review(r86): merge earlier reviewed candidate
4579086 docs(tasks): t-0209
fcedf25 review(r86): brief for revision 1
586ff25 checkpoint(r82): HANDOFF after merging the round
5dae023 Merge commit '20d6a83f2c15328dfa44fea2696ada04a26dac19'
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0209.md`, `tasks/t-0208.md`, `tasks/review-r86.md`, `tasks/t-0207.md`, `tasks/t-0199.md`, `tasks/t-0197.md`, `tasks/review-r82.md`, `tasks/t-0206.md`, `tasks/t-0205.md`, `tasks/review-r85.md`, `tasks/t-0204.md`, `tasks/t-0203.md`
- verdicts: `tasks/reviews/code-r86.md`, `tasks/reviews/code-r82.md`, `tasks/reviews/code-r85.md`, `tasks/reviews/code-r84.md`, `tasks/reviews/code-r83.md`, `tasks/reviews/code-r81.md`, `tasks/reviews/code-r80.md`, `tasks/reviews/code-r79.md`, `tasks/reviews/code-r78.md`, `tasks/reviews/code-r77.md`, `tasks/reviews/code-r75.md`, `tasks/reviews/code-r76.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r86` was merged into `main` at verdict commit `c50263a557b9926499fcd27a83bd9e27993e8300`; this checkpoint is its child.

