# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T22:57:25-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (working), `w1N` PRL-8-53 (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (11 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | c34bc4e | 0 | Merge commit 'fcbfa7821813a9ec21937e6704d564ec0a232179' |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r87` | `review/r87` | 2562629 | 0 | docs(tasks): t-0212 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r88` | `review/r88` | a502f5c | 0 | docs(tasks): t-0214 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r89` | `review/r89` | af0a93a | 0 | docs(tasks): t-0215 |

Last commits on the integration branch:

```
c34bc4e Merge commit 'fcbfa7821813a9ec21937e6704d564ec0a232179'
e5ff94c review(r89): brief for revision 1
7bd61ab review(r88): brief for revision 1
fcbfa78 docs(review): round r87 verdict
7f3a0ed docs(tasks): t-0213
3abaf78 Merge commit 'b82d5fc5ac56bc138917489901f6a01ca84eb9e5' into hp/adeherdr/t-0212-review-r87-every-coordinator-sees-each-h
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0212.md`, `tasks/review-r89.md`, `tasks/review-r88.md`, `tasks/t-0213.md`, `tasks/review-r87.md`, `tasks/t-0211.md`, `tasks/t-0210.md`, `tasks/t-0209.md`, `tasks/t-0208.md`, `tasks/review-r86.md`, `tasks/t-0207.md`, `tasks/t-0199.md`
- verdicts: `tasks/reviews/code-r87.md`, `tasks/reviews/code-r86.md`, `tasks/reviews/code-r82.md`, `tasks/reviews/code-r85.md`, `tasks/reviews/code-r84.md`, `tasks/reviews/code-r83.md`, `tasks/reviews/code-r81.md`, `tasks/reviews/code-r80.md`, `tasks/reviews/code-r79.md`, `tasks/reviews/code-r78.md`, `tasks/reviews/code-r77.md`, `tasks/reviews/code-r75.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r87` was merged into `main` at verdict commit `fcbfa7821813a9ec21937e6704d564ec0a232179`; this checkpoint is its child.

