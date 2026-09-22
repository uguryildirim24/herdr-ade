# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T20:37:25-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 5 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0200 | pi | working | `w1G:p3E` | `w1G:t3A` (t-0200) | `/home/agent/projects/herdr-ade/.worktrees/t-0200` | project=adeherdr rank=4 review=working thread=t-0200 | π - t-0200 |
| hp-adeherdr-t-0202 | pi | done | `w1G:p3G` | `w1G:t3C` (t-0202) | `/home/agent/projects/herdr-ade/.worktrees/t-0202` | project=adeherdr rank=1 review=ready-for-review thread=t-0202 | π - t-0202 |
| hp-adeherdr-t-0204 | pi | working | `w1G:p3J` | `w1G:t3E` (t-0204) | `/home/agent/projects/herdr-ade/.worktrees/t-0204` | done=1 lane=t-0204 project=adeherdr rank=4 review=working thread=t-0204 | π - t-0204 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0200 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0202 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0204 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (working), `w1N` PRL-8-53 (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (8 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 3b0c76a | 0 | docs(review): round r84 repair verdict |
| `/home/agent/projects/herdr-ade/.worktrees/review-r82` | `review/r82` | 8ba36e1 | 0 | docs(tasks): t-0197 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r82-2` | `review/r82-2` | dda99f9 | 0 | docs(tasks): t-0199 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r84` | `review/r84` | 2063242 | 0 | docs(tasks): t-0203 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r84-2` | `review/r84-2` | 1aaa8b6 | 0 | docs(tasks): t-0204 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0200` | `hp/adeherdr/t-0200-w8-install-finds-the-box` | 3aa671e | 10 | docs(tasks): t-0200 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0202` | `hp/adeherdr/t-0202-w9-no-workarounds-rule` | 068c020 | 0 | docs(skills): forbid harness workarounds |
| `/home/agent/projects/herdr-ade/.worktrees/t-0204` | `hp/adeherdr/t-0204-review-r84-every-coordinator-waits-for-t` | 3b0c76a | 0 | docs(review): round r84 repair verdict |

Last commits on the integration branch:

```
3b0c76a docs(review): round r84 repair verdict
f87c278 Merge commit '80a5f2d68dd5416a778e6f05001a6dd1a3416e9e' into hp/adeherdr/t-0204-review-r84-every-coordinator-waits-for-t
1aaa8b6 docs(tasks): t-0204
4c5870c review(r84): brief for revision 1
80a5f2d review(skills): describe typed pi recovery
4849014 checkpoint(r83): HANDOFF after merging the round
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0204.md`, `tasks/t-0203.md`, `tasks/review-r84.md`, `tasks/t-0201.md`, `tasks/t-0202.md`, `tasks/review-r83.md`, `tasks/t-0200.md`, `tasks/review-r82.md`, `tasks/t-0196.md`, `tasks/t-0193.md`, `tasks/t-0198.md`, `tasks/review-r81.md`
- verdicts: `tasks/reviews/code-r84.md`, `tasks/reviews/code-r83.md`, `tasks/reviews/code-r81.md`, `tasks/reviews/code-r80.md`, `tasks/reviews/code-r79.md`, `tasks/reviews/code-r78.md`, `tasks/reviews/code-r77.md`, `tasks/reviews/code-r75.md`, `tasks/reviews/code-r76.md`, `tasks/reviews/code-r74.md`, `tasks/reviews/code-r73.md`, `tasks/reviews/code-r72.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r84` was merged into `main` at verdict commit `3b0c76ae45f442fcc9cebaf33bdee432ab69478e`; this checkpoint is its child.

