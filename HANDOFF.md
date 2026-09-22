# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T13:21:51-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 3 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0288 | pi | idle | `w1G:p3N` | `w1G:t3H` (t-0288) | `/home/agent/projects/herdr-ade/.worktrees/t-0288` | project=adeherdr rank=4 review=working thread=t-0288 | π - t-0288 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0288 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (11 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 94ee12b | 0 | Merge commit '103bd1405817facd84bb50c3ad44a71d6a1bc88d' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r109` | `review/r109` | eb4554d | 0 | docs(tasks): t-0285 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0288` | `hp/adeherdr/t-0288-astra-max-more-excess-easier-to-use` | 2976832 | 0 | docs(tasks): t-0288 |

Last commits on the integration branch:

```
94ee12b Merge commit '103bd1405817facd84bb50c3ad44a71d6a1bc88d'
2976832 docs(tasks): t-0288
b8ba5f9 docs(tasks): t-0287
0197096 docs(tasks): t-0286
103bd14 docs(review): round r109 verdict
3199b43 review(talk): cover words mixed with idle notices
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0285.md`, `tasks/t-0288.md`, `tasks/t-0287.md`, `tasks/t-0286.md`, `tasks/review-r109.md`, `tasks/t-0284.md`, `tasks/t-0283.md`, `tasks/review-r108.md`, `tasks/t-0282.md`, `tasks/t-0281.md`, `tasks/t-0280.md`, `tasks/review-r107.md`
- verdicts: `tasks/reviews/code-r109.md`, `tasks/reviews/code-r108.md`, `tasks/reviews/code-r107.md`, `tasks/reviews/code-r106.md`, `tasks/reviews/code-r105.md`, `tasks/reviews/code-r104.md`, `tasks/reviews/code-r100.md`, `tasks/reviews/code-r103.md`, `tasks/reviews/code-r102.md`, `tasks/reviews/code-r98.md`, `tasks/reviews/code-r99.md`, `tasks/reviews/code-r97.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r109` was merged into `main` at verdict commit `103bd1405817facd84bb50c3ad44a71d6a1bc88d`; this checkpoint is its child.

