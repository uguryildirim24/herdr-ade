# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T09:49:24-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (working), `w1N` PRL-8-53 (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (7 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 6d1a972 | 0 | review(r107): verdict |
| `/home/agent/projects/herdr-ade/.worktrees/review-r107` | `review/r107` | f002a67 | 0 | docs(tasks): t-0280 |

Last commits on the integration branch:

```
6d1a972 review(r107): verdict
564d3d8 review(tasks): keep withdrawal and verification mutually exclusive
02ccb83 Merge commit '6ffad5b0e0f6b5da568dfcd17816f7e14bcbcecb' into hp/adeherdr/t-0280-review-r107-one-goal-a-newer-choice-repl
f002a67 docs(tasks): t-0280
baf70e7 review(r107): brief for revision 1
6ffad5b feat(tasks): withdraw replaced acceptance conditions
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0280.md`, `tasks/review-r107.md`, `tasks/t-0279.md`, `tasks/t-0278.md`, `tasks/review-r106.md`, `tasks/t-0277.md`, `tasks/t-0276.md`, `tasks/t-0275.md`, `tasks/t-0274.md`, `tasks/review-r105.md`, `tasks/t-0273.md`, `tasks/t-0272.md`
- verdicts: `tasks/reviews/code-r107.md`, `tasks/reviews/code-r106.md`, `tasks/reviews/code-r105.md`, `tasks/reviews/code-r104.md`, `tasks/reviews/code-r100.md`, `tasks/reviews/code-r103.md`, `tasks/reviews/code-r102.md`, `tasks/reviews/code-r98.md`, `tasks/reviews/code-r99.md`, `tasks/reviews/code-r97.md`, `tasks/reviews/code-r96.md`, `tasks/reviews/code-r92.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r107` was merged into `main` at verdict commit `6d1a97260e3e9396066f893e667ff996603eaa7b`; this checkpoint is its child.

