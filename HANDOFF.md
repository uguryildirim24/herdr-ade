# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T09:08:32-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (working), `w1N` PRL-8-53 (blocked)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (13 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | f84fffa | 0 | review(r106): verdict |
| `/home/agent/projects/herdr-ade/.worktrees/review-r106` | `review/r106` | 94f53f0 | 0 | docs(tasks): t-0278 |

Last commits on the integration branch:

```
f84fffa review(r106): verdict
524d313 review(threads): fail closed on attestation evidence
766bd2b Merge commit '4ae9e5c1af0875c3480b393313c155fc522a4e54' into hp/adeherdr/t-0278-review-r106-helper-notes-stay-out-of-sha
3da8008 Merge commit '99aea5062ace38a483ca1f254c80e7508c667d3a' into hp/adeherdr/t-0278-review-r106-helper-notes-stay-out-of-sha
5068a85 Merge commit 'dfacff1b344988a722df090a19492e1f8776ead0' into hp/adeherdr/t-0278-review-r106-helper-notes-stay-out-of-sha
94f53f0 docs(tasks): t-0278
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0278.md`, `tasks/review-r106.md`, `tasks/t-0277.md`, `tasks/t-0276.md`, `tasks/t-0275.md`, `tasks/t-0274.md`, `tasks/review-r105.md`, `tasks/t-0273.md`, `tasks/t-0272.md`, `tasks/review-r104.md`, `tasks/t-0271.md`, `tasks/t-0270.md`
- verdicts: `tasks/reviews/code-r106.md`, `tasks/reviews/code-r105.md`, `tasks/reviews/code-r104.md`, `tasks/reviews/code-r100.md`, `tasks/reviews/code-r103.md`, `tasks/reviews/code-r102.md`, `tasks/reviews/code-r98.md`, `tasks/reviews/code-r99.md`, `tasks/reviews/code-r97.md`, `tasks/reviews/code-r96.md`, `tasks/reviews/code-r92.md`, `tasks/reviews/code-r95.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r106` was merged into `main` at verdict commit `f84fffaac54db97d2398c47ab85e2205541e2d87`; this checkpoint is its child.

