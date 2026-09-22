# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T02:19:00-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (blocked)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (14 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 41aa603 | 0 | Merge commit 'e3bad0ea6d7218f104eaa770832c7a1000c02f1b' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r100` | `review/r100` | 97f81cd | 0 | docs(tasks): t-0256 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r97` | `review/r97` | 1189bfa | 0 | docs(tasks): t-0244 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r97-2` | `review/r97-2` | 7466be5 | 0 | docs(tasks): t-0251 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r97-3` | `review/r97-3` | 07a2a0d | 0 | docs(tasks): t-0254 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r98` | `review/r98` | 5e45996 | 0 | docs(tasks): t-0245 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r98-2` | `review/r98-2` | 63bb915 | 0 | docs(tasks): t-0252 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r98-3` | `review/r98-3` | d7ab258 | 0 | docs(tasks): t-0255 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r99` | `review/r99` | 72a6ea3 | 0 | docs(tasks): t-0253 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r99-2` | `review/r99-2` | ecba0b8 | 0 | docs(tasks): t-0261 |

Last commits on the integration branch:

```
41aa603 Merge commit 'e3bad0ea6d7218f104eaa770832c7a1000c02f1b'
e3d9223 docs(tasks): t-0260
e3bad0e docs(review): round r97 repair verdict
3d6ec0c review(r99): brief for revision 1
ce57527 Merge commit 'f54303e3dd7d5f7f14261d5b87e24702499575ea' into hp/adeherdr/t-0254-review-r97-a-job-that-should-never-have
cddfe97 review(r100): brief for revision 2
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0254.md`, `tasks/t-0251.md`, `tasks/t-0244.md`, `tasks/t-0260.md`, `tasks/review-r99.md`, `tasks/review-r100.md`, `tasks/review-r98.md`, `tasks/review-r97.md`, `tasks/t-0250.md`, `tasks/t-0242.md`, `tasks/review-r96.md`, `tasks/t-0249.md`
- verdicts: `tasks/reviews/code-r97.md`, `tasks/reviews/code-r96.md`, `tasks/reviews/code-r92.md`, `tasks/reviews/code-r95.md`, `tasks/reviews/code-r93.md`, `tasks/reviews/code-r94.md`, `tasks/reviews/code-r91.md`, `tasks/reviews/code-r90.md`, `tasks/reviews/code-r88.md`, `tasks/reviews/code-r89.md`, `tasks/reviews/code-r87.md`, `tasks/reviews/code-r86.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r97` was merged into `main` at verdict commit `e3bad0ea6d7218f104eaa770832c7a1000c02f1b`; this checkpoint is its child.

