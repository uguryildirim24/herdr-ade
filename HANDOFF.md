# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T14:18:25-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (12 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | eaa6efd | 0 | Merge commit '9520390653f47fadaa229c3e12c14ae8ef516900' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r70` | `review/r70` | 6b03df0 | 0 | docs(tasks): t-0167 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r71` | `review/r71` | aac28db | 0 | docs(tasks): t-0168 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r72` | `review/r72` | a5388d7 | 0 | docs(tasks): t-0169 |

Last commits on the integration branch:

```
eaa6efd Merge commit '9520390653f47fadaa229c3e12c14ae8ef516900'
9520390 review(r70): verdict
eb6aaf2 review(threads): remove clean abandoned worktrees safely
c9ddae3 review(r72): brief for revision 1
1720a46 review(r71): brief for revision 1
da87bac Merge t-0164 into review r70
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0167.md`, `tasks/review-r72.md`, `tasks/review-r71.md`, `tasks/review-r70.md`, `tasks/t-0166.md`, `tasks/t-0165.md`, `tasks/t-0164.md`, `tasks/t-0163.md`, `tasks/t-0162.md`, `tasks/t-0161.md`, `tasks/review-r69.md`, `tasks/t-0160.md`
- verdicts: `tasks/reviews/code-r70.md`, `tasks/reviews/code-r69.md`, `tasks/reviews/code-r68.md`, `tasks/reviews/code-r67.md`, `tasks/reviews/code-r66.md`, `tasks/reviews/code-r65.md`, `tasks/reviews/code-r64.md`, `tasks/reviews/code-r63.md`, `tasks/reviews/code-r62.md`, `tasks/reviews/code-r60.md`, `tasks/reviews/code-r61.md`, `tasks/reviews/code-r58.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r70` was merged into `main` at verdict commit `9520390653f47fadaa229c3e12c14ae8ef516900`; this checkpoint is its child.

