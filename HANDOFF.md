# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-21T18:24:57-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (9 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 26ad8f5 | 0 | Merge commit 'feba2cc54ff6debdc2a66db66ede13956a7b7880' |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r78` | `review/r78` | b8e18c9 | 0 | docs(tasks): t-0188 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r79` | `review/r79` | 6a73245 | 0 | docs(tasks): t-0189 |

Last commits on the integration branch:

```
26ad8f5 Merge commit 'feba2cc54ff6debdc2a66db66ede13956a7b7880'
feba2cc review(tasks): round r78 verdict
7131f53 review(tasks): preserve task evidence on migration
3764a7e review(r79): brief for revision 1
f4c3c82 Merge commit '742dda6f3ebddf9388eeb1d8454192a88dac37dd'
b8e18c9 docs(tasks): t-0188
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0188.md`, `tasks/review-r79.md`, `tasks/review-r78.md`, `tasks/t-0187.md`, `tasks/t-0185.md`, `tasks/t-0186.md`, `tasks/review-r77.md`, `tasks/t-0184.md`, `tasks/t-0183.md`, `tasks/t-0182.md`, `tasks/t-0180.md`, `tasks/t-0177.md`
- verdicts: `tasks/reviews/code-r78.md`, `tasks/reviews/code-r77.md`, `tasks/reviews/code-r75.md`, `tasks/reviews/code-r76.md`, `tasks/reviews/code-r74.md`, `tasks/reviews/code-r73.md`, `tasks/reviews/code-r72.md`, `tasks/reviews/code-r71.md`, `tasks/reviews/code-r70.md`, `tasks/reviews/code-r69.md`, `tasks/reviews/code-r68.md`, `tasks/reviews/code-r67.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r78` was merged into `main` at verdict commit `feba2cc54ff6debdc2a66db66ede13956a7b7880`; this checkpoint is its child.

