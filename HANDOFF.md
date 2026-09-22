# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T20:11:42-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (working), `w1N` PRL-8-53 (working)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (13 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 41ba4b5 | 0 | Merge commit 'f10a56605b7484b340a6bb42662dd598046a1d1f' |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r81` | `review/r81` | c50c4ea | 0 | docs(tasks): t-0193 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r81-2` | `review/r81-2` | 2070d5c | 0 | docs(tasks): t-0196 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r82` | `review/r82` | 8ba36e1 | 0 | docs(tasks): t-0197 |

Last commits on the integration branch:

```
41ba4b5 Merge commit 'f10a56605b7484b340a6bb42662dd598046a1d1f'
41b40ea docs(tasks): t-0198
f10a566 review(r81): approve repaired candidate
e3a9380 review(r81): merge earlier reviewed candidate
25f97dc review(r82): brief for revision 1
2070d5c docs(tasks): t-0196
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0196.md`, `tasks/t-0193.md`, `tasks/t-0198.md`, `tasks/review-r82.md`, `tasks/review-r81.md`, `tasks/t-0195.md`, `tasks/t-0194.md`, `tasks/t-0192.md`, `tasks/review-r80.md`, `tasks/t-0191.md`, `tasks/t-0189.md`, `tasks/review-r79.md`
- verdicts: `tasks/reviews/code-r81.md`, `tasks/reviews/code-r80.md`, `tasks/reviews/code-r79.md`, `tasks/reviews/code-r78.md`, `tasks/reviews/code-r77.md`, `tasks/reviews/code-r75.md`, `tasks/reviews/code-r76.md`, `tasks/reviews/code-r74.md`, `tasks/reviews/code-r73.md`, `tasks/reviews/code-r72.md`, `tasks/reviews/code-r71.md`, `tasks/reviews/code-r70.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r81` was merged into `main` at verdict commit `f10a56605b7484b340a6bb42662dd598046a1d1f`; this checkpoint is its child.

