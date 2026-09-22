# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T14:18:24-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (21 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 0fc18e4 | 0 | Merge commit 'ed62e579955dd260d548e57dac094b889ba5be1e' |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r110` | `review/r110` | ef7d6fa | 0 | docs(tasks): t-0296 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r111` | `review/r111` | c2801eb | 0 | docs(tasks): t-0297 |

Last commits on the integration branch:

```
0fc18e4 Merge commit 'ed62e579955dd260d548e57dac094b889ba5be1e'
ed62e57 review(ade): verdict r110
45d7fbd review(cleanup): reconcile closed round threads
bad0e84 review(r111): brief for revision 6
185febd Merge commit 'e14e3e90f21b5bc7c471b277ca2e8513a54a9b93' into hp/adeherdr/t-0296-review-r110-the-first-cuts-guides-match
6e112e9 Merge commit '089ee408300f278f646a7a30ee6710c1ec544e36' into hp/adeherdr/t-0296-review-r110-the-first-cuts-guides-match
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0296.md`, `tasks/review-r111.md`, `tasks/review-r110.md`, `tasks/t-0295.md`, `tasks/t-0294.md`, `tasks/t-0293.md`, `tasks/t-0292.md`, `tasks/t-0291.md`, `tasks/t-0290.md`, `tasks/t-0289.md`, `tasks/t-0285.md`, `tasks/t-0288.md`
- verdicts: `tasks/reviews/code-r110.md`, `tasks/reviews/code-r109.md`, `tasks/reviews/code-r108.md`, `tasks/reviews/code-r107.md`, `tasks/reviews/code-r106.md`, `tasks/reviews/code-r105.md`, `tasks/reviews/code-r104.md`, `tasks/reviews/code-r100.md`, `tasks/reviews/code-r103.md`, `tasks/reviews/code-r102.md`, `tasks/reviews/code-r98.md`, `tasks/reviews/code-r99.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r110` was merged into `main` at verdict commit `ed62e579955dd260d548e57dac094b889ba5be1e`; this checkpoint is its child.

