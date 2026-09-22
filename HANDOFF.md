# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T00:52:37-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (15 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | caac1b7 | 0 | Merge commit 'b4d814010501d284e9bf68a950078999bd09fd67' |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r92` | `review/r92` | f4cbd52 | 0 | docs(tasks): t-0228 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r92-2` | `review/r92-2` | 8f3a91a | 0 | docs(tasks): t-0233 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r92-3` | `review/r92-3` | f8dd402 | 0 | docs(tasks): t-0236 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r93` | `review/r93` | 345f901 | 0 | docs(tasks): t-0229 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r93-2` | `review/r93-2` | e1016c0 | 0 | docs(tasks): t-0231 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r93-3` | `review/r93-3` | 7dca92d | 0 | docs(tasks): t-0234 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r95` | `review/r95` | a131cfe | 0 | docs(tasks): t-0232 |

Last commits on the integration branch:

```
caac1b7 Merge commit 'b4d814010501d284e9bf68a950078999bd09fd67'
2fd101e docs(tasks): t-0237
b4d8140 review(dispatch): approve r93 after cleanup merge
50b68c7 review(r92): brief for revision 1
4e5a3d9 docs(tasks): t-0235
b2e16ec Merge commit 'f9b4efe00ab008a3f448f2bd9b9ef1ac776a4102' into hp/adeherdr/t-0234-review-r93-jobs-for-the-two-sign-in-help
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0234.md`, `tasks/t-0231.md`, `tasks/t-0229.md`, `tasks/t-0237.md`, `tasks/review-r92.md`, `tasks/t-0235.md`, `tasks/review-r93.md`, `tasks/t-0230.md`, `tasks/review-r95.md`, `tasks/review-r94.md`, `tasks/t-0227.md`, `tasks/review-r91.md`
- verdicts: `tasks/reviews/code-r93.md`, `tasks/reviews/code-r94.md`, `tasks/reviews/code-r91.md`, `tasks/reviews/code-r90.md`, `tasks/reviews/code-r88.md`, `tasks/reviews/code-r89.md`, `tasks/reviews/code-r87.md`, `tasks/reviews/code-r86.md`, `tasks/reviews/code-r82.md`, `tasks/reviews/code-r85.md`, `tasks/reviews/code-r84.md`, `tasks/reviews/code-r83.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r93` was merged into `main` at verdict commit `b4d814010501d284e9bf68a950078999bd09fd67`; this checkpoint is its child.

