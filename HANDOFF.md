# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T03:35:42-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (9 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 22e8b45 | 0 | review(ade): repair verdict for round r100 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r100` | `review/r100` | 97f81cd | 0 | docs(tasks): t-0256 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r100-2` | `review/r100-2` | 0713f8c | 0 | docs(tasks): t-0270 |

Last commits on the integration branch:

```
22e8b45 review(ade): repair verdict for round r100
8261bd5 Merge commit '62525575bd12bc368ac65db66f3ad99eb837c475' into hp/adeherdr/t-0270-review-r100-helpers-get-their-first-mess
0713f8c docs(tasks): t-0270
8cfbda7 review(r100): brief for revision 2
d67b3cb checkpoint(r103): HANDOFF after merging the round
4f619d9 docs(review): round r103 verdict
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0270.md`, `tasks/t-0256.md`, `tasks/review-r100.md`, `tasks/t-0269.md`, `tasks/review-r103.md`, `tasks/t-0268.md`, `tasks/t-0267.md`, `tasks/review-r102.md`, `tasks/t-0265.md`, `tasks/t-0263.md`, `tasks/t-0255.md`, `tasks/t-0252.md`
- verdicts: `tasks/reviews/code-r100.md`, `tasks/reviews/code-r103.md`, `tasks/reviews/code-r102.md`, `tasks/reviews/code-r98.md`, `tasks/reviews/code-r99.md`, `tasks/reviews/code-r97.md`, `tasks/reviews/code-r96.md`, `tasks/reviews/code-r92.md`, `tasks/reviews/code-r95.md`, `tasks/reviews/code-r93.md`, `tasks/reviews/code-r94.md`, `tasks/reviews/code-r91.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r100` was merged into `main` at verdict commit `22e8b4547b5bbf9939e1c23dc5c875d7efa4b78f`; this checkpoint is its child.

