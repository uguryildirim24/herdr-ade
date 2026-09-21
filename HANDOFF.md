# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T18:03:15-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (10 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | f59e11c | 0 | Merge commit 'd75dba04cb198929b36ce4b4985cfd75d41ff6b2' |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r77` | `review/r77` | 545e667 | 0 | docs(tasks): t-0185 |

Last commits on the integration branch:

```
f59e11c Merge commit 'd75dba04cb198929b36ce4b4985cfd75d41ff6b2'
d75dba0 docs(review): round r77 verdict
289f66b review(runtime): keep timeouts unknown and root ticker loop
ae0db4b docs(tasks): t-0186
0813e5d review(r77): merge pinned t-0183
545e667 docs(tasks): t-0185
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0185.md`, `tasks/t-0186.md`, `tasks/review-r77.md`, `tasks/t-0184.md`, `tasks/t-0183.md`, `tasks/t-0182.md`, `tasks/t-0180.md`, `tasks/t-0177.md`, `tasks/review-r75.md`, `tasks/t-0181.md`, `tasks/review-r76.md`, `tasks/t-0178.md`
- verdicts: `tasks/reviews/code-r77.md`, `tasks/reviews/code-r75.md`, `tasks/reviews/code-r76.md`, `tasks/reviews/code-r74.md`, `tasks/reviews/code-r73.md`, `tasks/reviews/code-r72.md`, `tasks/reviews/code-r71.md`, `tasks/reviews/code-r70.md`, `tasks/reviews/code-r69.md`, `tasks/reviews/code-r68.md`, `tasks/reviews/code-r67.md`, `tasks/reviews/code-r66.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r77` was merged into `main` at verdict commit `d75dba04cb198929b36ce4b4985cfd75d41ff6b2`; this checkpoint is its child.

