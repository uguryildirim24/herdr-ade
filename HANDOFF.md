# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T14:56:32-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (8 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 9b7960a | 0 | review(r72): approve repaired editable routing |
| `/home/agent/projects/herdr-ade/.worktrees/review-r72` | `review/r72` | a5388d7 | 0 | docs(tasks): t-0169 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r72-2` | `review/r72-2` | bfa0f5b | 0 | docs(tasks): t-0171 |

Last commits on the integration branch:

```
9b7960a review(r72): approve repaired editable routing
34a3208 Merge earlier r72 candidate into repair review
bfa0f5b docs(tasks): t-0171
ae2382e review(r72): brief for revision 1
08071c3 checkpoint(r71): HANDOFF after merging the round
668d59d review(herdr-ade): approve repaired r71 typed results
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0171.md`, `tasks/t-0169.md`, `tasks/review-r72.md`, `tasks/t-0170.md`, `tasks/t-0168.md`, `tasks/review-r71.md`, `tasks/t-0167.md`, `tasks/review-r70.md`, `tasks/t-0166.md`, `tasks/t-0165.md`, `tasks/t-0164.md`, `tasks/t-0163.md`
- verdicts: `tasks/reviews/code-r72.md`, `tasks/reviews/code-r71.md`, `tasks/reviews/code-r70.md`, `tasks/reviews/code-r69.md`, `tasks/reviews/code-r68.md`, `tasks/reviews/code-r67.md`, `tasks/reviews/code-r66.md`, `tasks/reviews/code-r65.md`, `tasks/reviews/code-r64.md`, `tasks/reviews/code-r63.md`, `tasks/reviews/code-r62.md`, `tasks/reviews/code-r60.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r72` was merged into `main` at verdict commit `9b7960a17aa5e6df64ce7e73d26e7bf42d6bbede`; this checkpoint is its child.

