# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T04:34:50-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (8 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 5aed547 | 0 | docs(review): round r104 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r104` | `review/r104` | 2a6b1ab | 0 | docs(tasks): t-0272 |

Last commits on the integration branch:

```
5aed547 docs(review): round r104 verdict
514a315 review(ade): keep omitted project forms working
34de33a Merge commit 'b9b9e9a1c3c40fcc941a50dff43c77a549bc54f2' into review/r104
2a6b1ab docs(tasks): t-0272
fb2a289 review(r104): brief for revision 1
b9b9e9a fix(hook): preserve installed project flag
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0272.md`, `tasks/review-r104.md`, `tasks/t-0271.md`, `tasks/t-0270.md`, `tasks/t-0256.md`, `tasks/review-r100.md`, `tasks/t-0269.md`, `tasks/review-r103.md`, `tasks/t-0268.md`, `tasks/t-0267.md`, `tasks/review-r102.md`, `tasks/t-0265.md`
- verdicts: `tasks/reviews/code-r104.md`, `tasks/reviews/code-r100.md`, `tasks/reviews/code-r103.md`, `tasks/reviews/code-r102.md`, `tasks/reviews/code-r98.md`, `tasks/reviews/code-r99.md`, `tasks/reviews/code-r97.md`, `tasks/reviews/code-r96.md`, `tasks/reviews/code-r92.md`, `tasks/reviews/code-r95.md`, `tasks/reviews/code-r93.md`, `tasks/reviews/code-r94.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r104` was merged into `main` at verdict commit `5aed547007b3272ed671789da17767036a5494fe`; this checkpoint is its child.

