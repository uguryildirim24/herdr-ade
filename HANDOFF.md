# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T20:59:20-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `e090742b-3a62-46fd-8bc3-73a7232d4e90`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1P` PRL-8-53 (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (7 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 0fc7a02 | 0 | review(round): approve r119 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r119` | `review/r119` | 167ecb0 | 0 | docs(tasks): t-0327 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r119-2` | `review/r119-2` | e42f84f | 0 | docs(tasks): t-0329 |

Last commits on the integration branch:

```
0fc7a02 review(round): approve r119
8265941 review(ade): finish moving workflow evidence to state
8b5df9a Merge pinned lane t-0324 into review r119
e42f84f docs(tasks): t-0329
440173d review(r119): brief for revision 2
62f359b refactor(project): remove legacy workflow file support
```

### Record files (newest first)

- verdicts: `tasks/reviews/code-r119.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
ha pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume e090742b-3a62-46fd-8bc3-73a7232d4e90
```

Round `r119` was merged into `main` at verdict commit `0fc7a02599ece2b103179ad2ecaf46822a2996b7`; this checkpoint is its child.
