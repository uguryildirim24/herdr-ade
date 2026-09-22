# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T03:22:07-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (5 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 4f619d9 | 0 | docs(review): round r103 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/review-r100` | `review/r100` | 97f81cd | 0 | docs(tasks): t-0256 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r103` | `review/r103` | ec1e920 | 0 | docs(tasks): t-0269 |

Last commits on the integration branch:

```
4f619d9 docs(review): round r103 verdict
da23480 Merge commit 'b9f92da8130db10323ce6c14f283432a911f1ef9' into hp/adeherdr/t-0269-review-r103-one-install-always-moves-the
ec1e920 docs(tasks): t-0269
9663d44 review(r103): brief for revision 1
b4fd82a checkpoint(r102): HANDOFF after merging the round
959bed0 docs(review): round r102 repair verdict
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0269.md`, `tasks/review-r103.md`, `tasks/t-0268.md`, `tasks/t-0267.md`, `tasks/review-r102.md`, `tasks/t-0265.md`, `tasks/t-0263.md`, `tasks/t-0255.md`, `tasks/t-0252.md`, `tasks/t-0245.md`, `tasks/t-0266.md`, `tasks/review-r98.md`
- verdicts: `tasks/reviews/code-r103.md`, `tasks/reviews/code-r102.md`, `tasks/reviews/code-r98.md`, `tasks/reviews/code-r99.md`, `tasks/reviews/code-r97.md`, `tasks/reviews/code-r96.md`, `tasks/reviews/code-r92.md`, `tasks/reviews/code-r95.md`, `tasks/reviews/code-r93.md`, `tasks/reviews/code-r94.md`, `tasks/reviews/code-r91.md`, `tasks/reviews/code-r90.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r103` was merged into `main` at verdict commit `4f619d9193e31ca9d89ee3f79be9077b62df010b`; this checkpoint is its child.

