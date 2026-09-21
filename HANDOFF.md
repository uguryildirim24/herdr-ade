# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T16:22:54-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (13 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 4df30e5 | 0 | Merge commit '2637e371bdb6ee63f5eabd8ebdb51172778314c8' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r74` | `review/r74` | d9c3d29 | 0 | docs(tasks): t-0176 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r74-2` | `review/r74-2` | a8c9da7 | 0 | docs(tasks): t-0178 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r75` | `review/r75` | 9757d53 | 0 | docs(tasks): t-0177 |

Last commits on the integration branch:

```
4df30e5 Merge commit '2637e371bdb6ee63f5eabd8ebdb51172778314c8'
2637e37 review(r74): approve recovery on ignored-data-safe cleanup
97e1d8d docs(tasks): t-0179
7967370 Merge earlier r74 candidate into repair review
a8c9da7 docs(tasks): t-0178
d87c658 review(r74): brief for revision 1
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0178.md`, `tasks/t-0176.md`, `tasks/t-0179.md`, `tasks/review-r74.md`, `tasks/review-r75.md`, `tasks/t-0175.md`, `tasks/review-r73.md`, `tasks/t-0174.md`, `tasks/t-0173.md`, `tasks/t-0172.md`, `tasks/t-0171.md`, `tasks/t-0169.md`
- verdicts: `tasks/reviews/code-r74.md`, `tasks/reviews/code-r73.md`, `tasks/reviews/code-r72.md`, `tasks/reviews/code-r71.md`, `tasks/reviews/code-r70.md`, `tasks/reviews/code-r69.md`, `tasks/reviews/code-r68.md`, `tasks/reviews/code-r67.md`, `tasks/reviews/code-r66.md`, `tasks/reviews/code-r65.md`, `tasks/reviews/code-r64.md`, `tasks/reviews/code-r63.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r74` was merged into `main` at verdict commit `2637e371bdb6ee63f5eabd8ebdb51172778314c8`; this checkpoint is its child.

