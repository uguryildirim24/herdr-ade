# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T14:57:21-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (24 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | bf6e390 | 0 | docs(review): round r111 repair verdict |
| `/home/agent/projects/herdr-ade/.worktrees/review-r111` | `review/r111` | c2801eb | 0 | docs(tasks): t-0297 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r111-2` | `review/r111-2` | 6c7760a | 0 | docs(tasks): t-0301 |

Last commits on the integration branch:

```
bf6e390 docs(review): round r111 repair verdict
a7334b3 review(lifecycle): refuse overlapping project records
c88153c review(lifecycle): keep deletion retryable through old-trash cleanup
f7ecfea review(r111): merge earlier reviewed candidate
6c7760a docs(tasks): t-0301
714d1f7 review(r111): brief for revision 6
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0301.md`, `tasks/t-0297.md`, `tasks/review-r111.md`, `tasks/t-0300.md`, `tasks/t-0299.md`, `tasks/t-0298.md`, `tasks/t-0296.md`, `tasks/review-r110.md`, `tasks/t-0295.md`, `tasks/t-0294.md`, `tasks/t-0293.md`, `tasks/t-0292.md`
- verdicts: `tasks/reviews/code-r111.md`, `tasks/reviews/code-r110.md`, `tasks/reviews/code-r109.md`, `tasks/reviews/code-r108.md`, `tasks/reviews/code-r107.md`, `tasks/reviews/code-r106.md`, `tasks/reviews/code-r105.md`, `tasks/reviews/code-r104.md`, `tasks/reviews/code-r100.md`, `tasks/reviews/code-r103.md`, `tasks/reviews/code-r102.md`, `tasks/reviews/code-r98.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r111` was merged into `main` at verdict commit `bf6e3907c802a6a19c42e07bd869446b2923be55`; this checkpoint is its child.

