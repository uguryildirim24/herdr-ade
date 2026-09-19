# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-19T16:30:32-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 3 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `6c22d8cf-cae6-47e2-a930-fd9207175aea`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0034 | pi | idle | `w1G:p1E` | `w1G:t1A` (t-0034) | `/Users/rolfie/projects/herdr-ade/.worktrees/t-0034` | done=1 lane=t-0034 project=adeherdr rank=3 review=working thread=t-0034 | π - t-0034 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0034 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (working), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (5 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | 082079d | 0 | docs(review): round r14 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r1` | `review/r1` | fc61993 | 0 | docs(tasks): t-0004 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r10` | `review/r10` | f9ae7d2 | 0 | docs(tasks): t-0024 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r11` | `review/r11` | 2c59e5d | 0 | docs(tasks): t-0025 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r12` | `review/r12` | a6de86a | 0 | docs(tasks): t-0026 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r12-2` | `review/r12-2` | df5d29f | 0 | docs(tasks): t-0031 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r13` | `review/r13` | 1d6260b | 0 | docs(tasks): t-0029 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r14` | `review/r14` | 0aca4f0 | 0 | docs(tasks): t-0034 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r2` | `review/r2` | b07aa74 | 0 | docs(tasks): t-0005 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r2-2` | `review/r2-2` | c191a33 | 0 | docs(tasks): t-0007 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r4` | `review/r4` | 689e079 | 0 | docs(tasks): t-0011 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r5` | `review/r5` | 008f9ac | 0 | docs(tasks): t-0012 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r6` | `review/r6` | 910649d | 0 | docs(tasks): t-0014 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r7` | `review/r7` | 7cb564e | 0 | docs(tasks): t-0015 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r9` | `review/r9` | ef62484 | 0 | docs(tasks): t-0022 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0001` | `hp/adeherdr/t-0001-plain-language-check-everyday-words-pass` | c482aa9 | 0 | checkpoint(r1): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0003` | `hp/adeherdr/t-0003-herdr-pro-the-pro-bridge-plugin-spec-pro` | 2f603cf | 0 | checkpoint(r2): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0004` | `hp/adeherdr/t-0004-review-r1-word-check-fix` | 356c0fa | 0 | docs(review): round r1 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0005` | `hp/adeherdr/t-0005-review-r2-herdr-pro` | ae593fb | 0 | review(pro): round r2 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0007` | `hp/adeherdr/t-0007-review-r2-second-pass-re-apply-the-herdr` | 65ba6ca | 0 | docs(review): round r2 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0008` | `hp/adeherdr/t-0008-harness-fixes-resolve-closes-the-pane-me` | 524b0f0 | 0 | checkpoint(r6): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0009` | `hp/adeherdr/t-0009-herdr-pro-v2-a-dedicated-pro-codex-home` | fded870 | 0 | checkpoint(r5): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0010` | `hp/adeherdr/t-0010-drop-the-jev-lane-picker-fix-the-model-p` | 5f94b7a | 0 | checkpoint(r4): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0011` | `hp/adeherdr/t-0011-review-r4-the-picker-is-gone-models-fixe` | 712ff7b | 0 | docs(review): round r4 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0012` | `hp/adeherdr/t-0012-review-r5-the-worker-s-own-small-home` | 6a77dd2 | 0 | docs(review): round r5 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0013` | `hp/adeherdr/t-0013-gpt-image-gen-a-picture-profile-on-the-b` | 35e92c2 | 0 | checkpoint(r7): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0014` | `hp/adeherdr/t-0014-review-r6-the-four-harness-fixes` | 8bbc451 | 0 | docs(review): round r6 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0015` | `hp/adeherdr/t-0015-review-r7-the-picture-maker` | 38d04f9 | 0 | docs(review): round r7 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0017` | `hp/adeherdr/t-0017-box-lanes-start-side-a-lane-starts-and-s` | d64488a | 0 | docs(tasks): t-0023 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0018` | `hp/adeherdr/t-0018-harness-the-context-tells-a-coordinator` | ca94e81 | 0 | checkpoint(r10): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0019` | `hp/adeherdr/t-0019-herdr-pro-pictures-from-screenshots-and` | ca94e81 | 0 | checkpoint(r10): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0020` | `hp/adeherdr/t-0020-herdr-pro-serve-pro-through-a-local-rela` | b645bb6 | 0 | checkpoint(r11): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0022` | `hp/adeherdr/t-0022-review-r9-box-lanes-start-side` | d360c1f | 0 | docs(review): round r9 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0023` | `hp/adeherdr/t-0023-box-lanes-completion-side-the-courier-br` | 016fd4b | 0 | checkpoint(r12): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0024` | `hp/adeherdr/t-0024-review-r10-the-harness-starts-its-own-ch` | e8091fd | 0 | docs(review): round r10 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0025` | `hp/adeherdr/t-0025-review-r11-this-check-reads-the-relay-th` | fed2f04 | 0 | docs(review): round r11 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0026` | `hp/adeherdr/t-0026-review-r12-this-check-reads-the-piece-th` | 05db9f9 | 0 | docs(review): round r12 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0027` | `hp/adeherdr/t-0027-repair-the-cloud-box-completion-side-aft` | 016fd4b | 0 | checkpoint(r12): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0028` | `hp/adeherdr/t-0028-pro-lane-cold-start-waits-on-the-rollout` | 1455ea1 | 0 | checkpoint(r13): HANDOFF after merging the round |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0029` | `hp/adeherdr/t-0029-review-r13-this-check-reads-the-small-ch` | d93abec | 0 | docs(review): round r13 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0031` | `hp/adeherdr/t-0031-review-r12-this-check-reads-the-piece-th` | 1826946 | 0 | docs(review): round r12 revision 2 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0032` | `hp/adeherdr/t-0032-deepseek-lanes-compact-near-372k-written` | 852c203 | 0 | feat(pi): setup writes the DeepSeek compaction window (t-0032) |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0034` | `hp/adeherdr/t-0034-review-r14-this-check-reads-the-small-ch` | 082079d | 0 | docs(review): round r14 verdict |

Last commits on the integration branch:

```
082079d docs(review): round r14 verdict
6d20c16 Merge commit '852c2039ab9cd91584f5e72b3d568e4df8ee3762' into hp/adeherdr/t-0034-review-r14-this-check-reads-the-small-ch
0aca4f0 docs(tasks): t-0034
1a976e6 review(r14): brief for revision 1
852c203 feat(pi): setup writes the DeepSeek compaction window (t-0032)
016fd4b checkpoint(r12): HANDOFF after merging the round
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0034.md`, `tasks/review-r14.md`, `tasks/t-0031.md`, `tasks/t-0026.md`, `tasks/t-0023.md`, `tasks/t-0032.md`, `tasks/review-r12.md`, `tasks/t-0029.md`, `tasks/review-r13.md`, `tasks/t-0028.md`, `tasks/t-0027.md`, `tasks/t-0025.md`
- verdicts: `tasks/reviews/code-r14.md`, `tasks/reviews/code-r12.md`, `tasks/reviews/code-r13.md`, `tasks/reviews/code-r11.md`, `tasks/reviews/code-r9.md`, `tasks/reviews/code-r10.md`, `tasks/reviews/code-r7.md`, `tasks/reviews/code-r6.md`, `tasks/reviews/code-r5.md`, `tasks/reviews/code-r4.md`, `tasks/reviews/code-r2.md`, `tasks/reviews/code-r1.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 6c22d8cf-cae6-47e2-a930-fd9207175aea
```

Round `r14` was merged into `main` at verdict commit `082079de6b3ecde5c0accf021b48f74219893670`; this checkpoint is its child.

