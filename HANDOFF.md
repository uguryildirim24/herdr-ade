# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T17:12:43-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (12 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | f0100d1 | 0 | review(r75): approve typed failures with r76 cleanup |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r75` | `review/r75` | 9757d53 | 0 | docs(tasks): t-0177 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r75-2` | `review/r75-2` | 653b0d5 | 0 | docs(tasks): t-0180 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r75-3` | `review/r75-3` | 437b5e1 | 0 | docs(tasks): t-0182 |

Last commits on the integration branch:

```
f0100d1 review(r75): approve typed failures with r76 cleanup
8f57cbe review(cli): make failed work the usable default
98199b6 Merge reviewed r75 candidate for revision 1
437b5e1 docs(tasks): t-0182
d068ea4 review(r75): brief for revision 1
402ad49 review(core): preserve typed recovery across retry commands
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0182.md`, `tasks/t-0180.md`, `tasks/t-0177.md`, `tasks/review-r75.md`, `tasks/t-0181.md`, `tasks/review-r76.md`, `tasks/t-0178.md`, `tasks/t-0176.md`, `tasks/t-0179.md`, `tasks/review-r74.md`, `tasks/t-0175.md`, `tasks/review-r73.md`
- verdicts: `tasks/reviews/code-r75.md`, `tasks/reviews/code-r76.md`, `tasks/reviews/code-r74.md`, `tasks/reviews/code-r73.md`, `tasks/reviews/code-r72.md`, `tasks/reviews/code-r71.md`, `tasks/reviews/code-r70.md`, `tasks/reviews/code-r69.md`, `tasks/reviews/code-r68.md`, `tasks/reviews/code-r67.md`, `tasks/reviews/code-r66.md`, `tasks/reviews/code-r65.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r75` was merged into `main` at verdict commit `f0100d1adf5a9be4f533625fcb4520272af79d82`; this checkpoint is its child.

