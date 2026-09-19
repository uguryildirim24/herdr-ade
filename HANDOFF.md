# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-19T13:11:01-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 3 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `6c22d8cf-cae6-47e2-a930-fd9207175aea`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0007 | pi | done | `w1G:pD` | `w1G:t9` (t-0007) | `/home/agent/projects/herdr-ade/.worktrees/t-0007` | done=1 lane=t-0007 project=adeherdr rank=3 review=working thread=t-0007 | π - t-0007 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0007 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (idle), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (8 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 65ba6ca | 0 | docs(review): round r2 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/review-r1` | `review/r1` | fc61993 | 0 | docs(tasks): t-0004 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r2` | `review/r2` | b07aa74 | 0 | docs(tasks): t-0005 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r2-2` | `review/r2-2` | c191a33 | 0 | docs(tasks): t-0007 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0001` | `hp/adeherdr/t-0001-plain-language-check-everyday-words-pass` | c482aa9 | 0 | checkpoint(r1): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0003` | `hp/adeherdr/t-0003-herdr-pro-the-pro-bridge-plugin-spec-pro` | bb87891 | 0 | docs(pro): pro-bridge commands, state and first-turn needs |
| `/home/agent/projects/herdr-ade/.worktrees/t-0004` | `hp/adeherdr/t-0004-review-r1-word-check-fix` | 356c0fa | 0 | docs(review): round r1 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0005` | `hp/adeherdr/t-0005-review-r2-herdr-pro` | ae593fb | 0 | review(pro): round r2 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0007` | `hp/adeherdr/t-0007-review-r2-second-pass-re-apply-the-herdr` | 65ba6ca | 0 | docs(review): round r2 verdict |

Last commits on the integration branch:

```
65ba6ca docs(review): round r2 verdict
fe2fa9f review(pro): make turns and breaker fail closed
8073ca2 Merge commit 'bb878913c254b1edae0895b91747438b584cc7ed' into hp/adeherdr/t-0007-review-r2-second-pass-re-apply-the-herdr
c191a33 docs(tasks): t-0007
7efa98f review(r2): brief for revision 1
c482aa9 checkpoint(r1): HANDOFF after merging the round
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0007.md`, `tasks/t-0003.md`, `tasks/review-r2.md`, `tasks/t-0004.md`, `tasks/review-r1.md`, `tasks/t-0001.md`, `tasks/review-plugin-r2.md`, `tasks/ade-deepseek-go.md`, `tasks/review-plugin-r1-b.md`, `tasks/review-plugin-r1.md`, `tasks/ade-pi.md`, `tasks/ade-picker.md`
- verdicts: `tasks/reviews/code-r2.md`, `tasks/reviews/code-r1.md`, `tasks/reviews/code-plugin-r2.md`, `tasks/reviews/code-plugin-r1.md`, `tasks/reviews/code-plugin-r1-b.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 6c22d8cf-cae6-47e2-a930-fd9207175aea
```

Round `r2` was merged into `main` at verdict commit `65ba6ca40b78ee9d1ed5876719f1f41d88139761`; this checkpoint is its child.

