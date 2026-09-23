# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T20:32:30-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `e090742b-3a62-46fd-8bc3-73a7232d4e90`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (11 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 7505341 | 0 | review(round): approve r120 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r119` | `review/r119` | 167ecb0 | 0 | docs(tasks): t-0327 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r120` | `review/r120` | 572a55b | 0 | docs(tasks): t-0326 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r120-2` | `review/r120-2` | ae64bf3 | 0 | docs(tasks): t-0328 |

Last commits on the integration branch:

```
7505341 review(round): approve r120
73a3191 review(ade): retain coordinator recipe across rebind
f24ad9d merge(r120): t-0325 coordinator hooks and recipe
ae64bf3 docs(tasks): t-0328
1acabe6 review(r120): brief for revision 2
bef5f28 fix(hooks): rebind coordinators after install
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0328.md`, `tasks/review-r120.md`, `tasks/review-r119.md`, `tasks/t-0325.md`, `tasks/t-0324.md`, `tasks/t-0323.md`, `tasks/review-r118.md`, `tasks/t-0322.md`, `tasks/t-0321.md`, `tasks/t-0320.md`, `tasks/review-r117.md`, `tasks/t-0319.md`
- verdicts: `tasks/reviews/code-r120.md`, `tasks/reviews/code-r118.md`, `tasks/reviews/code-r117.md`, `tasks/reviews/code-r116.md`, `tasks/reviews/code-r115.md`, `tasks/reviews/code-r114.md`, `tasks/reviews/code-r113.md`, `tasks/reviews/code-r112.md`, `tasks/reviews/code-r111.md`, `tasks/reviews/code-r110.md`, `tasks/reviews/code-r109.md`, `tasks/reviews/code-r108.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
ha pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume e090742b-3a62-46fd-8bc3-73a7232d4e90
```

Round `r120` was merged into `main` at verdict commit `7505341ce369ae6628ea464fe6d95319d38f8b9f`; this checkpoint is its child.

