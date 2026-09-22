# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T00:37:21-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (10 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | ca71b8d | 0 | Merge commit 'bb2f93b8e0768aa456c9538d5a8e3e75fc1c19e9' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r92` | `review/r92` | f4cbd52 | 0 | docs(tasks): t-0228 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r92-2` | `review/r92-2` | 8f3a91a | 0 | docs(tasks): t-0233 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r93` | `review/r93` | 345f901 | 0 | docs(tasks): t-0229 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r93-2` | `review/r93-2` | e1016c0 | 0 | docs(tasks): t-0231 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r94` | `review/r94` | 25519e9 | 0 | docs(tasks): t-0230 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r95` | `review/r95` | a131cfe | 0 | docs(tasks): t-0232 |

Last commits on the integration branch:

```
ca71b8d Merge commit 'bb2f93b8e0768aa456c9538d5a8e3e75fc1c19e9'
194c4b2 review(r92): brief for revision 1
bb2f93b docs(review): round r94 verdict
a8066db review(r95): brief for revision 1
87de9e0 review(cleanup): tolerate removal races
0c656e2 review(r93): brief for revision 1
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0230.md`, `tasks/review-r92.md`, `tasks/review-r95.md`, `tasks/review-r93.md`, `tasks/review-r94.md`, `tasks/t-0227.md`, `tasks/review-r91.md`, `tasks/t-0226.md`, `tasks/t-0225.md`, `tasks/t-0224.md`, `tasks/t-0223.md`, `tasks/t-0222.md`
- verdicts: `tasks/reviews/code-r94.md`, `tasks/reviews/code-r91.md`, `tasks/reviews/code-r90.md`, `tasks/reviews/code-r88.md`, `tasks/reviews/code-r89.md`, `tasks/reviews/code-r87.md`, `tasks/reviews/code-r86.md`, `tasks/reviews/code-r82.md`, `tasks/reviews/code-r85.md`, `tasks/reviews/code-r84.md`, `tasks/reviews/code-r83.md`, `tasks/reviews/code-r81.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r94` was merged into `main` at verdict commit `bb2f93b8e0768aa456c9538d5a8e3e75fc1c19e9`; this checkpoint is its child.

