# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T23:35:49-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (12 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | e6ad477 | 0 | review(r88): approve candidate on W11 base |
| `/home/agent/projects/herdr-ade/.worktrees/review-r88` | `review/r88` | a502f5c | 0 | docs(tasks): t-0214 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r88-2` | `review/r88-2` | 352e264 | 0 | docs(tasks): t-0217 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r88-3` | `review/r88-3` | c4a5b5d | 0 | docs(tasks): t-0220 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r90` | `review/r90` | f3e6464 | 0 | docs(tasks): t-0216 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r90-2` | `review/r90-2` | bab4c62 | 0 | docs(tasks): t-0219 |

Last commits on the integration branch:

```
e6ad477 review(r88): approve candidate on W11 base
89655af review(r88): merge earlier candidate onto revision 1
c4a5b5d docs(tasks): t-0220
fc2cb18 review(r88): brief for revision 1
b3cb4c4 review(r90): brief for revision 1
6b3b869 checkpoint(r89): HANDOFF after merging the round
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0220.md`, `tasks/t-0217.md`, `tasks/t-0214.md`, `tasks/review-r88.md`, `tasks/review-r90.md`, `tasks/t-0218.md`, `tasks/t-0215.md`, `tasks/review-r89.md`, `tasks/t-0212.md`, `tasks/t-0213.md`, `tasks/review-r87.md`, `tasks/t-0211.md`
- verdicts: `tasks/reviews/code-r88.md`, `tasks/reviews/code-r89.md`, `tasks/reviews/code-r87.md`, `tasks/reviews/code-r86.md`, `tasks/reviews/code-r82.md`, `tasks/reviews/code-r85.md`, `tasks/reviews/code-r84.md`, `tasks/reviews/code-r83.md`, `tasks/reviews/code-r81.md`, `tasks/reviews/code-r80.md`, `tasks/reviews/code-r79.md`, `tasks/reviews/code-r78.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r88` was merged into `main` at verdict commit `e6ad477cb929cf74ded077c6c8ba284644f7b6d8`; this checkpoint is its child.

