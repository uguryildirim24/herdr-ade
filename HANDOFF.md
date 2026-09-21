# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T13:32:02-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (8 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | d039852 | 0 | review(r69): repair verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r69` | `review/r69` | e5dde6a | 0 | docs(tasks): t-0162 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r69-2` | `review/r69-2` | 151ede4 | 0 | docs(tasks): t-0163 |

Last commits on the integration branch:

```
d039852 review(r69): repair verdict
5f3f5c4 Merge earlier r69 candidate into repair review
151ede4 docs(tasks): t-0163
ac3f656 review(r69): brief for revision 1
a4e136d checkpoint(r68): HANDOFF after merging the round
aeb46dc merge(talk): t-0159 installed binary hand-off
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0163.md`, `tasks/t-0162.md`, `tasks/t-0161.md`, `tasks/review-r69.md`, `tasks/t-0160.md`, `tasks/review-r68.md`, `tasks/t-0159.md`, `tasks/t-0158.md`, `tasks/t-0156.md`, `tasks/t-0155.md`, `tasks/t-0154.md`, `tasks/review-r67.md`
- verdicts: `tasks/reviews/code-r69.md`, `tasks/reviews/code-r68.md`, `tasks/reviews/code-r67.md`, `tasks/reviews/code-r66.md`, `tasks/reviews/code-r65.md`, `tasks/reviews/code-r64.md`, `tasks/reviews/code-r63.md`, `tasks/reviews/code-r62.md`, `tasks/reviews/code-r60.md`, `tasks/reviews/code-r61.md`, `tasks/reviews/code-r58.md`, `tasks/reviews/code-r57.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r69` was merged into `main` at verdict commit `d039852282463740f0423cce444826396f56bbdb`; this checkpoint is its child.

