# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T01:10:43-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (13 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | a7dd0d6 | 0 | Merge commit '7c87096409b738a48fe55a6cd493cdbb424468c7' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r92` | `review/r92` | f4cbd52 | 0 | docs(tasks): t-0228 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r92-2` | `review/r92-2` | 8f3a91a | 0 | docs(tasks): t-0233 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r92-3` | `review/r92-3` | f8dd402 | 0 | docs(tasks): t-0236 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r92-4` | `review/r92-4` | 33fc226 | 0 | docs(tasks): t-0241 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r95` | `review/r95` | a131cfe | 0 | docs(tasks): t-0232 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r95-2` | `review/r95-2` | e256dd0 | 0 | docs(tasks): t-0240 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r96` | `review/r96` | eb52279 | 0 | docs(tasks): t-0242 |

Last commits on the integration branch:

```
a7dd0d6 Merge commit '7c87096409b738a48fe55a6cd493cdbb424468c7'
7c87096 docs(review): round r95 repair verdict
a53217a review(r96): brief for revision 1
feb103b Merge commit '2ceea19fbbfcdb28f71aee6bd8c0232e138714ef' into hp/adeherdr/t-0240-review-r95-rolf-s-newer-choices-always-c
6467078 review(r92): brief for revision 1
e256dd0 docs(tasks): t-0240
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0240.md`, `tasks/t-0232.md`, `tasks/review-r96.md`, `tasks/review-r92.md`, `tasks/review-r95.md`, `tasks/t-0239.md`, `tasks/t-0238.md`, `tasks/t-0234.md`, `tasks/t-0231.md`, `tasks/t-0229.md`, `tasks/t-0237.md`, `tasks/t-0235.md`
- verdicts: `tasks/reviews/code-r95.md`, `tasks/reviews/code-r93.md`, `tasks/reviews/code-r94.md`, `tasks/reviews/code-r91.md`, `tasks/reviews/code-r90.md`, `tasks/reviews/code-r88.md`, `tasks/reviews/code-r89.md`, `tasks/reviews/code-r87.md`, `tasks/reviews/code-r86.md`, `tasks/reviews/code-r82.md`, `tasks/reviews/code-r85.md`, `tasks/reviews/code-r84.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r95` was merged into `main` at verdict commit `7c87096409b738a48fe55a6cd493cdbb424468c7`; this checkpoint is its child.

