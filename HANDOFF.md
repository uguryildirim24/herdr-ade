# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T14:41:21-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (8 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 668d59d | 0 | review(herdr-ade): approve repaired r71 typed results |
| `/home/agent/projects/herdr-ade/.worktrees/review-r71` | `review/r71` | aac28db | 0 | docs(tasks): t-0168 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r71-2` | `review/r71-2` | 9512387 | 0 | docs(tasks): t-0170 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r72` | `review/r72` | a5388d7 | 0 | docs(tasks): t-0169 |

Last commits on the integration branch:

```
668d59d review(herdr-ade): approve repaired r71 typed results
2c901ca review(herdr-ade): type integrated cleanup results
a15d8fe Merge earlier r71 candidate into repair review
9512387 docs(tasks): t-0170
5111744 review(r71): brief for revision 1
1ee48b3 checkpoint(r70): HANDOFF after merging the round
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0170.md`, `tasks/t-0168.md`, `tasks/review-r71.md`, `tasks/t-0167.md`, `tasks/review-r72.md`, `tasks/review-r70.md`, `tasks/t-0166.md`, `tasks/t-0165.md`, `tasks/t-0164.md`, `tasks/t-0163.md`, `tasks/t-0162.md`, `tasks/t-0161.md`
- verdicts: `tasks/reviews/code-r71.md`, `tasks/reviews/code-r70.md`, `tasks/reviews/code-r69.md`, `tasks/reviews/code-r68.md`, `tasks/reviews/code-r67.md`, `tasks/reviews/code-r66.md`, `tasks/reviews/code-r65.md`, `tasks/reviews/code-r64.md`, `tasks/reviews/code-r63.md`, `tasks/reviews/code-r62.md`, `tasks/reviews/code-r60.md`, `tasks/reviews/code-r61.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r71` was merged into `main` at verdict commit `668d59da9d3c077f3579ee58069e8b45dcd765ec`; this checkpoint is its child.

