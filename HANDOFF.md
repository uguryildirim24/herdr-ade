# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-19T15:28:10-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 4 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `6c22d8cf-cae6-47e2-a930-fd9207175aea`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0025 | pi | done | `w1G:p14` | `w1G:t0` (t-0025) | `/home/agent/projects/herdr-ade/.worktrees/t-0025` | done=1 lane=t-0025 project=adeherdr rank=3 review=working thread=t-0025 | π - t-0025 |
| hp-adeherdr-t-0026 | pi | working | `w1G:p15` | `w1G:t11` (t-0026) | `/home/agent/projects/herdr-ade/.worktrees/t-0026` | project=adeherdr rank=3 review=working thread=t-0026 | π - t-0026 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0025 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0026 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (working), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (12 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | b7f4fe3 | 0 | Merge commit 'fed2f04b7fb95f491d0f3cb58c5a90208212f4eb' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r1` | `review/r1` | fc61993 | 0 | docs(tasks): t-0004 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r10` | `review/r10` | f9ae7d2 | 0 | docs(tasks): t-0024 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r11` | `review/r11` | 2c59e5d | 0 | docs(tasks): t-0025 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r12` | `review/r12` | a6de86a | 0 | docs(tasks): t-0026 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r2` | `review/r2` | b07aa74 | 0 | docs(tasks): t-0005 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r2-2` | `review/r2-2` | c191a33 | 0 | docs(tasks): t-0007 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r4` | `review/r4` | 689e079 | 0 | docs(tasks): t-0011 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r5` | `review/r5` | 008f9ac | 0 | docs(tasks): t-0012 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r6` | `review/r6` | 910649d | 0 | docs(tasks): t-0014 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r7` | `review/r7` | 7cb564e | 0 | docs(tasks): t-0015 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r9` | `review/r9` | ef62484 | 0 | docs(tasks): t-0022 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0001` | `hp/adeherdr/t-0001-plain-language-check-everyday-words-pass` | c482aa9 | 0 | checkpoint(r1): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0003` | `hp/adeherdr/t-0003-herdr-pro-the-pro-bridge-plugin-spec-pro` | 2f603cf | 0 | checkpoint(r2): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0004` | `hp/adeherdr/t-0004-review-r1-word-check-fix` | 356c0fa | 0 | docs(review): round r1 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0005` | `hp/adeherdr/t-0005-review-r2-herdr-pro` | ae593fb | 0 | review(pro): round r2 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0007` | `hp/adeherdr/t-0007-review-r2-second-pass-re-apply-the-herdr` | 65ba6ca | 0 | docs(review): round r2 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0008` | `hp/adeherdr/t-0008-harness-fixes-resolve-closes-the-pane-me` | 524b0f0 | 0 | checkpoint(r6): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0009` | `hp/adeherdr/t-0009-herdr-pro-v2-a-dedicated-pro-codex-home` | fded870 | 0 | checkpoint(r5): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0010` | `hp/adeherdr/t-0010-drop-the-jev-lane-picker-fix-the-model-p` | 5f94b7a | 0 | checkpoint(r4): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0011` | `hp/adeherdr/t-0011-review-r4-the-picker-is-gone-models-fixe` | 712ff7b | 0 | docs(review): round r4 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0012` | `hp/adeherdr/t-0012-review-r5-the-worker-s-own-small-home` | 6a77dd2 | 0 | docs(review): round r5 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0013` | `hp/adeherdr/t-0013-gpt-image-gen-a-picture-profile-on-the-b` | 35e92c2 | 0 | checkpoint(r7): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0014` | `hp/adeherdr/t-0014-review-r6-the-four-harness-fixes` | 8bbc451 | 0 | docs(review): round r6 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0015` | `hp/adeherdr/t-0015-review-r7-the-picture-maker` | 38d04f9 | 0 | docs(review): round r7 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0017` | `hp/adeherdr/t-0017-box-lanes-start-side-a-lane-starts-and-s` | d64488a | 0 | docs(tasks): t-0023 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0018` | `hp/adeherdr/t-0018-harness-the-context-tells-a-coordinator` | ca94e81 | 0 | checkpoint(r10): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0019` | `hp/adeherdr/t-0019-herdr-pro-pictures-from-screenshots-and` | ca94e81 | 0 | checkpoint(r10): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0020` | `hp/adeherdr/t-0020-herdr-pro-serve-pro-through-a-local-rela` | 8e55c49 | 0 | feat(pi): setup writes the pro provider and pi_pro joins the rows |
| `/home/agent/projects/herdr-ade/.worktrees/t-0022` | `hp/adeherdr/t-0022-review-r9-box-lanes-start-side` | d360c1f | 0 | docs(review): round r9 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0023` | `hp/adeherdr/t-0023-box-lanes-completion-side-the-courier-br` | 789bf54 | 0 | feat(remote): the courier imports box completions |
| `/home/agent/projects/herdr-ade/.worktrees/t-0024` | `hp/adeherdr/t-0024-review-r10-the-harness-starts-its-own-ch` | e8091fd | 0 | docs(review): round r10 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0025` | `hp/adeherdr/t-0025-review-r11-this-check-reads-the-relay-th` | fed2f04 | 0 | docs(review): round r11 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0026` | `hp/adeherdr/t-0026-review-r12-this-check-reads-the-piece-th` | c849cf3 | 0 | Merge commit '789bf54e5e056442a62f842841804ecdf82007b4' into hp/adeherdr/t-0026-review-r12-this-check-reads-the-piece-th |

Last commits on the integration branch:

```
b7f4fe3 Merge commit 'fed2f04b7fb95f491d0f3cb58c5a90208212f4eb'
fed2f04 docs(review): round r11 verdict
61adf51 review(pro): refuse relay startup drift
c082b97 review(r12): brief for revision 1
464aae0 checkpoint(r9): HANDOFF after merging the round
3cd3a9a Merge commit 'd360c1f0c66059aa048ec48f4aee01bafce9a216'
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0025.md`, `tasks/review-r12.md`, `tasks/t-0022.md`, `tasks/review-r11.md`, `tasks/t-0024.md`, `tasks/review-r10.md`, `tasks/review-r9.md`, `tasks/t-0020.md`, `tasks/t-0019.md`, `tasks/t-0018.md`, `tasks/t-0017.md`, `tasks/t-0015.md`
- verdicts: `tasks/reviews/code-r11.md`, `tasks/reviews/code-r9.md`, `tasks/reviews/code-r10.md`, `tasks/reviews/code-r7.md`, `tasks/reviews/code-r6.md`, `tasks/reviews/code-r5.md`, `tasks/reviews/code-r4.md`, `tasks/reviews/code-r2.md`, `tasks/reviews/code-r1.md`, `tasks/reviews/code-plugin-r2.md`, `tasks/reviews/code-plugin-r1.md`, `tasks/reviews/code-plugin-r1-b.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 6c22d8cf-cae6-47e2-a930-fd9207175aea
```

Round `r11` was merged into `main` at verdict commit `fed2f04b7fb95f491d0f3cb58c5a90208212f4eb`; this checkpoint is its child.

