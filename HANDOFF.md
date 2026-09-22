# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T01:51:51-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (13 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | fe5c038 | 0 | Merge commit 'f52c92611a8dcdaeab1848a498d0f64c29f7b44b' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r96` | `review/r96` | eb52279 | 0 | docs(tasks): t-0242 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r96-2` | `review/r96-2` | 9a56f88 | 0 | docs(tasks): t-0250 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r97` | `review/r97` | 1189bfa | 0 | docs(tasks): t-0244 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r97-2` | `review/r97-2` | 7466be5 | 0 | docs(tasks): t-0251 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r98` | `review/r98` | 5e45996 | 0 | docs(tasks): t-0245 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r98-2` | `review/r98-2` | 63bb915 | 0 | docs(tasks): t-0252 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r99` | `review/r99` | 72a6ea3 | 0 | docs(tasks): t-0253 |

Last commits on the integration branch:

```
fe5c038 Merge commit 'f52c92611a8dcdaeab1848a498d0f64c29f7b44b'
f52c926 review(pro): verdict for repaired r96
fe52091 review(r99): brief for revision 1
c25def4 Merge reviewed r96 candidate onto revised base
de79778 review(r98): brief for revision 1
1449ef5 review(r97): brief for revision 1
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0250.md`, `tasks/t-0242.md`, `tasks/review-r99.md`, `tasks/review-r98.md`, `tasks/review-r97.md`, `tasks/review-r96.md`, `tasks/t-0249.md`, `tasks/t-0243.md`, `tasks/t-0241.md`, `tasks/t-0236.md`, `tasks/t-0233.md`, `tasks/t-0228.md`
- verdicts: `tasks/reviews/code-r96.md`, `tasks/reviews/code-r92.md`, `tasks/reviews/code-r95.md`, `tasks/reviews/code-r93.md`, `tasks/reviews/code-r94.md`, `tasks/reviews/code-r91.md`, `tasks/reviews/code-r90.md`, `tasks/reviews/code-r88.md`, `tasks/reviews/code-r89.md`, `tasks/reviews/code-r87.md`, `tasks/reviews/code-r86.md`, `tasks/reviews/code-r82.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r96` was merged into `main` at verdict commit `f52c92611a8dcdaeab1848a498d0f64c29f7b44b`; this checkpoint is its child.

