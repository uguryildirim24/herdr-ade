# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T20:29:25-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 6 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0200 | pi | working | `w1G:p3E` | `w1G:t3A` (t-0200) | `/Users/rolfie/projects/herdr-ade/.worktrees/t-0200` | project=adeherdr rank=4 review=working thread=t-0200 | π - t-0200 |
| hp-adeherdr-t-0201 | pi | done | `w1G:p3F` | `w1G:t3B` (t-0201) | `/Users/rolfie/projects/herdr-ade/.worktrees/t-0201` | done=1 lane=t-0201 project=adeherdr rank=1 review=ready-for-review thread=t-0201 | π - t-0201 |
| hp-adeherdr-t-0202 | pi | done | `w1G:p3G` | `w1G:t3C` (t-0202) | `/Users/rolfie/projects/herdr-ade/.worktrees/t-0202` | done=1 lane=t-0202 project=adeherdr rank=1 review=ready-for-review thread=t-0202 | π - t-0202 |
| hp-adeherdr-t-0203 | pi | working | `w1G:p3H` | `w1G:t3D` (t-0203) | `/Users/rolfie/projects/herdr-ade/.worktrees/t-0203` | project=adeherdr rank=4 review=working thread=t-0203 | π - t-0203 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0200 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0201 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0202 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0203 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (working), `w1N` PRL-8-53 (working)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (11 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | af455f3 | 0 | Merge commit '4eea06da7993654d71e8acf2ef55c8f98331b29e' |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r82` | `review/r82` | 8ba36e1 | 0 | docs(tasks): t-0197 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r82-2` | `review/r82-2` | dda99f9 | 0 | docs(tasks): t-0199 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r83` | `review/r83` | e4ac703 | 0 | docs(tasks): t-0201 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r84` | `review/r84` | 2063242 | 0 | docs(tasks): t-0203 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0200` | `hp/adeherdr/t-0200-w8-install-finds-the-box` | 3aa671e | 7 | docs(tasks): t-0200 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0201` | `hp/adeherdr/t-0201-review-r83-a-job-with-no-code-folder-get` | 4eea06d | 0 | docs(review): round r83 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0202` | `hp/adeherdr/t-0202-w9-no-workarounds-rule` | 068c020 | 0 | docs(skills): forbid harness workarounds |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0203` | `hp/adeherdr/t-0203-review-r84-every-coordinator-waits-for-t` | ef36719 | 1 | Merge commit '068c0200ea5a0f23e9a8eb57ae9685055be736d6' into hp/adeherdr/t-0203-review-r84-every-coordinator-waits-for-t |

Last commits on the integration branch:

```
af455f3 Merge commit '4eea06da7993654d71e8acf2ef55c8f98331b29e'
4eea06d docs(review): round r83 verdict
ff89cc2 review(threads): preserve adopted cwd during rebind
76dd652 review(r84): brief for revision 1
0c1dbdb review(threads): compare managed folders canonically
81fc62f Merge pinned lane t-0195 for r83
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0201.md`, `tasks/review-r84.md`, `tasks/t-0202.md`, `tasks/review-r83.md`, `tasks/t-0200.md`, `tasks/review-r82.md`, `tasks/t-0196.md`, `tasks/t-0193.md`, `tasks/t-0198.md`, `tasks/review-r81.md`, `tasks/t-0195.md`, `tasks/t-0194.md`
- verdicts: `tasks/reviews/code-r83.md`, `tasks/reviews/code-r81.md`, `tasks/reviews/code-r80.md`, `tasks/reviews/code-r79.md`, `tasks/reviews/code-r78.md`, `tasks/reviews/code-r77.md`, `tasks/reviews/code-r75.md`, `tasks/reviews/code-r76.md`, `tasks/reviews/code-r74.md`, `tasks/reviews/code-r73.md`, `tasks/reviews/code-r72.md`, `tasks/reviews/code-r71.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r83` was merged into `main` at verdict commit `4eea06da7993654d71e8acf2ef55c8f98331b29e`; this checkpoint is its child.

