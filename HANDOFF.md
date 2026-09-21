# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T19:24:08-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1M` flyonenomics (unknown), `w1N` PRL-8-53 (idle)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (8 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | a93aaad | 0 | Merge commit 'f4f3c7a513a741e27c40fe0e4bb4d25f7f0b4ff1' |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r80` | `review/r80` | 2f35ee2 | 0 | docs(tasks): t-0192 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r81` | `review/r81` | c50c4ea | 0 | docs(tasks): t-0193 |

Last commits on the integration branch:

```
a93aaad Merge commit 'f4f3c7a513a741e27c40fe0e4bb4d25f7f0b4ff1'
f4f3c7a docs(review): round r80 verdict
f042697 review(harness): make install evidence fail closed
7cdd855 review(r81): brief for revision 1
f4a5430 merge: admit t-0190 into r80
2f35ee2 docs(tasks): t-0192
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0192.md`, `tasks/review-r81.md`, `tasks/review-r80.md`, `tasks/t-0191.md`, `tasks/t-0189.md`, `tasks/review-r79.md`, `tasks/t-0190.md`, `tasks/t-0188.md`, `tasks/review-r78.md`, `tasks/t-0187.md`, `tasks/t-0185.md`, `tasks/t-0186.md`
- verdicts: `tasks/reviews/code-r80.md`, `tasks/reviews/code-r79.md`, `tasks/reviews/code-r78.md`, `tasks/reviews/code-r77.md`, `tasks/reviews/code-r75.md`, `tasks/reviews/code-r76.md`, `tasks/reviews/code-r74.md`, `tasks/reviews/code-r73.md`, `tasks/reviews/code-r72.md`, `tasks/reviews/code-r71.md`, `tasks/reviews/code-r70.md`, `tasks/reviews/code-r69.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r80` was merged into `main` at verdict commit `f4f3c7a513a741e27c40fe0e4bb4d25f7f0b4ff1`; this checkpoint is its child.

