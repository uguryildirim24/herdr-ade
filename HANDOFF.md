# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-19T12:58:12-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 5 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `6c22d8cf-cae6-47e2-a930-fd9207175aea`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0004 | pi | idle | `w1G:pA` | `w1G:t6` (t-0004) | `/home/agent/projects/herdr-ade/.worktrees/t-0004` | done=1 lane=t-0004 project=adeherdr rank=1 review=ready-for-review thread=t-0004 | π - t-0004 |
| hp-adeherdr-t-0005 | pi | working | `w1G:pB` | `w1G:t7` (t-0005) | `/home/agent/projects/herdr-ade/.worktrees/t-0005` | project=adeherdr rank=3 review=working thread=t-0005 | π - t-0005 |
| hp-adeherdr-t-0006 | pi | working | `w1G:pC` | `w1G:t8` (t-0006) | `/home/agent/projects/herdr/.worktrees/t-0006` | project=adeherdr rank=3 review=working thread=t-0006 | π - t-0006 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0004 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0005 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0006 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (idle), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (8 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 356c0fa | 0 | docs(review): round r1 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/review-r1` | `review/r1` | fc61993 | 0 | docs(tasks): t-0004 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r2` | `review/r2` | b07aa74 | 0 | docs(tasks): t-0005 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0001` | `hp/adeherdr/t-0001-plain-language-check-everyday-words-pass` | 76da268 | 0 | fix(plain): everyday words pass and one block per turn |
| `/home/agent/projects/herdr-ade/.worktrees/t-0003` | `hp/adeherdr/t-0003-herdr-pro-the-pro-bridge-plugin-spec-pro` | bb87891 | 0 | docs(pro): pro-bridge commands, state and first-turn needs |
| `/home/agent/projects/herdr-ade/.worktrees/t-0004` | `hp/adeherdr/t-0004-review-r1-word-check-fix` | 356c0fa | 0 | docs(review): round r1 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0005` | `hp/adeherdr/t-0005-review-r2-herdr-pro` | 8d2f38f | 5 | merge: admit herdr-pro lane for review |

Last commits on the integration branch:

```
356c0fa docs(review): round r1 verdict
5dcf624 review(skill): remove coordinator approval restriction
1f52ea0 review(plain): clean current clippy lints
a785ceb Merge commit '76da26869bf4fe58790dc12397c9239adfcc2a22' into hp/adeherdr/t-0004-review-r1-word-check-fix
fc61993 docs(tasks): t-0004
b719725 review(r1): brief for revision 1
```

### Record files (newest first)

- briefs: `tasks/t-0004.md`, `tasks/review-r1.md`, `tasks/t-0001.md`, `tasks/review-plugin-r2.md`, `tasks/ade-deepseek-go.md`, `tasks/review-plugin-r1-b.md`, `tasks/review-plugin-r1.md`, `tasks/ade-pi.md`, `tasks/ade-picker.md`, `tasks/ade-rounds.md`, `tasks/ade-outbox.md`, `tasks/ade-core.md`
- verdicts: `tasks/reviews/code-r1.md`, `tasks/reviews/code-plugin-r2.md`, `tasks/reviews/code-plugin-r1.md`, `tasks/reviews/code-plugin-r1-b.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 6c22d8cf-cae6-47e2-a930-fd9207175aea
```

Round `r1` was merged into `main` at verdict commit `356c0fa90c6146dfac7b3c10fba5a5a6e58599b4`; this checkpoint is its child.
