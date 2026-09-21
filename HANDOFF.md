# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T18:47:18-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (9 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 8035351 | 0 | docs(review): round r79 repair verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r79` | `review/r79` | 6a73245 | 0 | docs(tasks): t-0189 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r79-2` | `review/r79-2` | dce5e00 | 0 | docs(tasks): t-0191 |

Last commits on the integration branch:

```
8035351 docs(review): round r79 repair verdict
ebd209a review(doctor): preserve unknown disk and build ownership
ce99313 Merge commit 'c3daa1bea72619abe7369204f4f93f4ab12bf6df' into hp/adeherdr/t-0191-review-r79-a-finished-lane-s-build-folde
dce5e00 docs(tasks): t-0191
06562c9 review(r79): brief for revision 1
64c73b3 docs(tasks): t-0190
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0191.md`, `tasks/t-0189.md`, `tasks/review-r79.md`, `tasks/t-0190.md`, `tasks/t-0188.md`, `tasks/review-r78.md`, `tasks/t-0187.md`, `tasks/t-0185.md`, `tasks/t-0186.md`, `tasks/review-r77.md`, `tasks/t-0184.md`, `tasks/t-0183.md`
- verdicts: `tasks/reviews/code-r79.md`, `tasks/reviews/code-r78.md`, `tasks/reviews/code-r77.md`, `tasks/reviews/code-r75.md`, `tasks/reviews/code-r76.md`, `tasks/reviews/code-r74.md`, `tasks/reviews/code-r73.md`, `tasks/reviews/code-r72.md`, `tasks/reviews/code-r71.md`, `tasks/reviews/code-r70.md`, `tasks/reviews/code-r69.md`, `tasks/reviews/code-r68.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r79` was merged into `main` at verdict commit `8035351597d7828d747ac65f5fc0434ee643c44d`; this checkpoint is its child.

