# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T15:57:54-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (12 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | c191592 | 0 | Merge commit '75c2a2ccd41e43798b9892ae755c01bf98eaf9eb' |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r73` | `review/r73` | e7a3b8c | 0 | docs(tasks): t-0175 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r74` | `review/r74` | d9c3d29 | 0 | docs(tasks): t-0176 |

Last commits on the integration branch:

```
c191592 Merge commit '75c2a2ccd41e43798b9892ae755c01bf98eaf9eb'
75c2a2c review(r73): approve ignored-data-safe worktree cleanup
736cf6a review(core): make ignored-data inspection collision safe
2544f24 review(r74): brief for revision 1
c7877a6 Merge pinned t-0174 into review r73
e7a3b8c docs(tasks): t-0175
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0175.md`, `tasks/review-r74.md`, `tasks/review-r73.md`, `tasks/t-0174.md`, `tasks/t-0173.md`, `tasks/t-0172.md`, `tasks/t-0171.md`, `tasks/t-0169.md`, `tasks/review-r72.md`, `tasks/t-0170.md`, `tasks/t-0168.md`, `tasks/review-r71.md`
- verdicts: `tasks/reviews/code-r73.md`, `tasks/reviews/code-r72.md`, `tasks/reviews/code-r71.md`, `tasks/reviews/code-r70.md`, `tasks/reviews/code-r69.md`, `tasks/reviews/code-r68.md`, `tasks/reviews/code-r67.md`, `tasks/reviews/code-r66.md`, `tasks/reviews/code-r65.md`, `tasks/reviews/code-r64.md`, `tasks/reviews/code-r63.md`, `tasks/reviews/code-r62.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r73` was merged into `main` at verdict commit `75c2a2ccd41e43798b9892ae755c01bf98eaf9eb`; this checkpoint is its child.

