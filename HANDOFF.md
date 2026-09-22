# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T21:56:40-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 4 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0200 | pi | done | `w1G:p3E` | `w1G:t3A` (t-0200) | `/Users/rolfie/projects/herdr-ade/.worktrees/t-0200` | project=adeherdr rank=1 review=ready-for-review thread=t-0200 | π - t-0200 |
| hp-adeherdr-t-0205 | pi | done | `w1G:p3K` | `w1G:t3F` (t-0205) | `/Users/rolfie/projects/herdr-ade/.worktrees/t-0205` | project=adeherdr rank=1 review=ready-for-review thread=t-0205 | π - t-0205 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0200 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0205 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (working)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (6 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | cbf1457 | 0 | docs(review): round r85 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r82` | `review/r82` | 8ba36e1 | 0 | docs(tasks): t-0197 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r82-2` | `review/r82-2` | dda99f9 | 0 | docs(tasks): t-0199 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r85` | `review/r85` | 0c8fc37 | 0 | docs(tasks): t-0205 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0200` | `hp/adeherdr/t-0200-w8-install-finds-the-box` | d15a664 | 0 | fix(remote): resolve box declarations by label |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0205` | `hp/adeherdr/t-0205-review-r85-one-install-run-reaches-the-b` | cbf1457 | 0 | docs(review): round r85 verdict |

Last commits on the integration branch:

```
cbf1457 docs(review): round r85 verdict
defffa3 review(ade): finish stable machine recovery
f9016b0 Merge commit 'd15a6640330f9e63d136a64b32887461d1a5d06b' into hp/adeherdr/t-0205-review-r85-one-install-run-reaches-the-b
0c8fc37 docs(tasks): t-0205
e22c233 review(r85): brief for revision 1
d15a664 fix(remote): resolve box declarations by label
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0205.md`, `tasks/review-r85.md`, `tasks/t-0204.md`, `tasks/t-0203.md`, `tasks/review-r84.md`, `tasks/t-0201.md`, `tasks/t-0202.md`, `tasks/review-r83.md`, `tasks/t-0200.md`, `tasks/review-r82.md`, `tasks/t-0196.md`, `tasks/t-0193.md`
- verdicts: `tasks/reviews/code-r85.md`, `tasks/reviews/code-r84.md`, `tasks/reviews/code-r83.md`, `tasks/reviews/code-r81.md`, `tasks/reviews/code-r80.md`, `tasks/reviews/code-r79.md`, `tasks/reviews/code-r78.md`, `tasks/reviews/code-r77.md`, `tasks/reviews/code-r75.md`, `tasks/reviews/code-r76.md`, `tasks/reviews/code-r74.md`, `tasks/reviews/code-r73.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r85` was merged into `main` at verdict commit `cbf1457e9f4b729e1e5c6e0196353b7e03a99c2b`; this checkpoint is its child.

