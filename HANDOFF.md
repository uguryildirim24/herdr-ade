# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T17:02:31-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `e090742b-3a62-46fd-8bc3-73a7232d4e90`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (16 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 078caf1 | 0 | review(r114): merge |
| `/home/agent/projects/herdr-ade/.worktrees/review-r114` | `review/r114` | 36d5d64 | 0 | docs(tasks): t-0311 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r114-2` | `review/r114-2` | badfeb4 | 0 | docs(tasks): t-0312 |

Last commits on the integration branch:

```
078caf1 review(r114): merge
d4a62f4 review(round): reserve repair review branch numbers
000cd15 Merge commit '2ee06374ee6e430b438d95e6a51811280eb2fd65' into hp/adeherdr/t-0312-review-r114-a-merge-that-installs-record
7356968 Merge commit '45849463dc869ba8c5e0023fddb01db4f7d6bef7' into hp/adeherdr/t-0312-review-r114-a-merge-that-installs-record
933d268 Merge commit '3575c380bbfc7416c75fd61a66796ee8681bc581' into hp/adeherdr/t-0312-review-r114-a-merge-that-installs-record
badfeb4 docs(tasks): t-0312
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0312.md`, `tasks/review-r114.md`, `tasks/t-0310.md`, `tasks/t-0309.md`, `tasks/t-0308.md`, `tasks/t-0307.md`, `tasks/review-r113.md`, `tasks/t-0306.md`, `tasks/t-0305.md`, `tasks/t-0304.md`, `tasks/t-0303.md`, `tasks/t-0302.md`
- verdicts: `tasks/reviews/code-r114.md`, `tasks/reviews/code-r113.md`, `tasks/reviews/code-r112.md`, `tasks/reviews/code-r111.md`, `tasks/reviews/code-r110.md`, `tasks/reviews/code-r109.md`, `tasks/reviews/code-r108.md`, `tasks/reviews/code-r107.md`, `tasks/reviews/code-r106.md`, `tasks/reviews/code-r105.md`, `tasks/reviews/code-r104.md`, `tasks/reviews/code-r100.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
ha pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume e090742b-3a62-46fd-8bc3-73a7232d4e90
```

Round `r114` was merged into `main` at verdict commit `078caf1dddfc2d8731a6d49af3adc672f6ad22e2`; this checkpoint is its child.

