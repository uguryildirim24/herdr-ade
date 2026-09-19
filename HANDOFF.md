# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-19T18:46:33-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 11 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `6c22d8cf-cae6-47e2-a930-fd9207175aea`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0050 | pi | done | `w1G:p23` | `w1G:t1Z` (t-0050) | `/home/agent/projects/herdr-ade/.worktrees/t-0050` | project=adeherdr rank=1 review=ready-for-review thread=t-0050 | π - t-0050 |
| hp-adeherdr-t-0053 | pi | done | `w1G:p26` | `w1G:t22` (t-0053) | `/home/agent/projects/herdr/.worktrees/t-0053` | done=1 lane=t-0053 project=adeherdr rank=1 review=ready-for-review thread=t-0053 | π - t-0053 |
| hp-adeherdr-t-0054 | pi | done | `w1G:p27` | `w1G:t23` (t-0054) | `/home/agent/projects/herdr-ade/.worktrees/t-0054` | done=1 lane=t-0054 project=adeherdr rank=1 review=ready-for-review thread=t-0054 | π - t-0054 |
| hp-adeherdr-t-0055 | pi | idle | `w1G:p28` | `w1G:t24` (t-0055) | `/home/agent/projects/herdr-ade/.worktrees/t-0055` | done=1 lane=t-0055 project=adeherdr rank=1 review=ready-for-review thread=t-0055 | π - t-0055 |
| hp-adeherdr-t-0057 | pi | working | `w1G:p2A` | `w1G:t26` (t-0057) | `/home/agent/projects/herdr-ade/.worktrees/t-0057` | project=adeherdr rank=3 review=working thread=t-0057 | π - t-0057 |
| hp-adeherdr-t-0059 | pi | done | `w1G:p2B` | `w1G:t27` (t-0059) | `/home/agent/projects/herdr-ade/.worktrees/t-0059` | done=1 lane=t-0059 project=adeherdr rank=1 review=ready-for-review thread=t-0059 | π - t-0059 |
| hp-adeherdr-t-0060 | pi | working | `w1G:p2C` | `w1G:t28` (t-0060) | `/home/agent/projects/herdr-ade/.worktrees/t-0060` | project=adeherdr rank=3 review=working thread=t-0060 | π - t-0060 |
| hp-adeherdr-t-0061 | pi | working | `w1G:p2D` | `w1G:t29` (t-0061) | `/home/agent/projects/herdr-ade/.worktrees/t-0061` | project=adeherdr rank=3 review=working thread=t-0061 | π - t-0061 |
| hp-adeherdr-t-0062 | pi | working | `w1G:p2E` | `w1G:t2A` (t-0062) | `/home/agent/projects/herdr/.worktrees/t-0062` | project=adeherdr rank=3 review=working thread=t-0062 | π - t-0062 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0050 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0053 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0054 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0055 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0057 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0059 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0060 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0061 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0062 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (working), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (11 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | bfc5771 | 0 | Merge commit '3938a05f9ecbcb1343d60286fbe3c3989c9cb164' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r1` | `review/r1` | fc61993 | 0 | docs(tasks): t-0004 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r10` | `review/r10` | f9ae7d2 | 0 | docs(tasks): t-0024 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r11` | `review/r11` | 2c59e5d | 0 | docs(tasks): t-0025 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r12` | `review/r12` | a6de86a | 0 | docs(tasks): t-0026 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r12-2` | `review/r12-2` | df5d29f | 0 | docs(tasks): t-0031 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r13` | `review/r13` | 1d6260b | 0 | docs(tasks): t-0029 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r14` | `review/r14` | 0aca4f0 | 0 | docs(tasks): t-0034 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r15` | `review/r15` | 7790b46 | 0 | docs(tasks): t-0037 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r18` | `review/r18` | 03e0873 | 0 | docs(tasks): t-0043 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r19` | `review/r19` | de89ab7 | 0 | docs(tasks): t-0045 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r2` | `review/r2` | b07aa74 | 0 | docs(tasks): t-0005 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r2-2` | `review/r2-2` | c191a33 | 0 | docs(tasks): t-0007 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r20` | `review/r20` | babc448 | 0 | docs(tasks): t-0047 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r22` | `review/r22` | 5868178 | 0 | docs(tasks): t-0052 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r23` | `review/r23` | b6f7453 | 0 | docs(tasks): t-0056 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r24` | `review/r24` | fd3f37a | 0 | docs(tasks): t-0059 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r25` | `review/r25` | d564494 | 0 | docs(tasks): t-0060 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r26` | `review/r26` | bf51381 | 0 | docs(tasks): t-0061 |
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
| `/home/agent/projects/herdr-ade/.worktrees/t-0020` | `hp/adeherdr/t-0020-herdr-pro-serve-pro-through-a-local-rela` | b645bb6 | 0 | checkpoint(r11): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0022` | `hp/adeherdr/t-0022-review-r9-box-lanes-start-side` | d360c1f | 0 | docs(review): round r9 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0023` | `hp/adeherdr/t-0023-box-lanes-completion-side-the-courier-br` | 016fd4b | 0 | checkpoint(r12): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0024` | `hp/adeherdr/t-0024-review-r10-the-harness-starts-its-own-ch` | e8091fd | 0 | docs(review): round r10 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0025` | `hp/adeherdr/t-0025-review-r11-this-check-reads-the-relay-th` | fed2f04 | 0 | docs(review): round r11 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0026` | `hp/adeherdr/t-0026-review-r12-this-check-reads-the-piece-th` | 05db9f9 | 0 | docs(review): round r12 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0027` | `hp/adeherdr/t-0027-repair-the-cloud-box-completion-side-aft` | 016fd4b | 0 | checkpoint(r12): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0028` | `hp/adeherdr/t-0028-pro-lane-cold-start-waits-on-the-rollout` | 1455ea1 | 0 | checkpoint(r13): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0029` | `hp/adeherdr/t-0029-review-r13-this-check-reads-the-small-ch` | d93abec | 0 | docs(review): round r13 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0031` | `hp/adeherdr/t-0031-review-r12-this-check-reads-the-piece-th` | 1826946 | 0 | docs(review): round r12 revision 2 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0032` | `hp/adeherdr/t-0032-deepseek-lanes-compact-near-372k-written` | e79bd41 | 0 | checkpoint(r14): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0034` | `hp/adeherdr/t-0034-review-r14-this-check-reads-the-small-ch` | 082079d | 0 | docs(review): round r14 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0035` | `hp/adeherdr/t-0035-relay-lets-pro-ask-for-files-read-and-li` | c583004 | 0 | checkpoint(r15): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0037` | `hp/adeherdr/t-0037-review-r15-this-check-reads-the-change-t` | 2c394d4 | 0 | docs(review): round r15 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0041` | `hp/adeherdr/t-0041-project-screen-lane-1-plan-card-decision` | ff87a6a | 0 | checkpoint(r18): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0043` | `hp/adeherdr/t-0043-review-r18-this-round-checks-the-records` | a4b9685 | 0 | docs(review): round r18 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0044` | `hp/adeherdr/t-0044-box-readiness-check-calls-the-box-s-pi-h` | 243c609 | 0 | checkpoint(r19): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0045` | `hp/adeherdr/t-0045-review-r19-this-round-checks-the-fix-tha` | 388ff21 | 0 | docs(review): round r19 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0046` | `hp/adeherdr/t-0046-box-health-rows-bash-probe-on-linux-no-r` | 6e09b02 | 0 | checkpoint(r20): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0047` | `hp/adeherdr/t-0047-review-r20-this-round-checks-the-health` | fdcb26b | 0 | docs(review): round r20 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0049` | `hp/adeherdr/t-0049-every-box-command-runs-with-the-fixed-bo` | fe436d2 | 0 | checkpoint(r23): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0050` | `hp/adeherdr/t-0050-lanes-and-reviews-go-to-the-box-by-defau` | c771b89 | 0 | feat(threads): lanes and reviewers run on the box by default |
| `/home/agent/projects/herdr-ade/.worktrees/t-0051` | `hp/adeherdr/t-0051-a-finished-lane-s-line-always-reaches-th` | 3c979e2 | 0 | checkpoint(r22): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0052` | `hp/adeherdr/t-0052-review-r22-this-round-checks-the-rule-th` | f934860 | 0 | docs(review): round r22 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0054` | `hp/adeherdr/t-0054-every-coordinator-may-evolve-the-harness` | d7776e7 | 0 | feat(harness): any coordinator may evolve the harness |
| `/home/agent/projects/herdr-ade/.worktrees/t-0055` | `hp/adeherdr/t-0055-a-new-session-picks-up-box-lanes-and-can` | c41b270 | 0 | feat(pickup): pick up box lanes and start gone ones |
| `/home/agent/projects/herdr-ade/.worktrees/t-0056` | `hp/adeherdr/t-0056-review-r23-this-round-checks-the-fix-tha` | 7e07548 | 0 | docs(review): round r23 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0057` | `hp/adeherdr/t-0057-project-screen-lane-2-the-overview-above` | b2c609e | 9 | docs(tasks): t-0057 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0059` | `hp/adeherdr/t-0059-review-r24-the-rule-that-sends-new-lanes` | 3938a05 | 0 | docs(review): round r24 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0060` | `hp/adeherdr/t-0060-review-r25-this-round-checks-the-rule-th` | de8cc60 | 3 | Merge commit 'd7776e7f025ad9f6914bbba3e1ffc0725679278d' into review/r25 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0061` | `hp/adeherdr/t-0061-review-r26-this-round-checks-the-fresh-s` | 0258cef | 0 | Merge commit 'c41b270cf910e48771a9b5389f7eb2ce777a6231' into hp/adeherdr/t-0061-review-r26-this-round-checks-the-fresh-s |

Last commits on the integration branch:

```
bfc5771 Merge commit '3938a05f9ecbcb1343d60286fbe3c3989c9cb164'
3938a05 docs(review): round r24 verdict
b13ab88 review(threads): fall back when the box is unreachable
b20ccca review(launch): keep machine choice on the role row
8cef3fa review(r26): brief for revision 1
4b4588b review(r25): brief for revision 1
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0059.md`, `tasks/review-r26.md`, `tasks/review-r25.md`, `tasks/t-0058.md`, `tasks/t-0057.md`, `tasks/t-0056.md`, `tasks/review-r24.md`, `tasks/review-r23.md`, `tasks/t-0055.md`, `tasks/t-0054.md`, `tasks/t-0052.md`, `tasks/review-r22.md`
- verdicts: `tasks/reviews/code-r24.md`, `tasks/reviews/code-r23.md`, `tasks/reviews/code-r22.md`, `tasks/reviews/code-r20.md`, `tasks/reviews/code-r19.md`, `tasks/reviews/code-r18.md`, `tasks/reviews/code-r15.md`, `tasks/reviews/code-r14.md`, `tasks/reviews/code-r12.md`, `tasks/reviews/code-r13.md`, `tasks/reviews/code-r11.md`, `tasks/reviews/code-r9.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 6c22d8cf-cae6-47e2-a930-fd9207175aea
```

Round `r24` was merged into `main` at verdict commit `3938a05f9ecbcb1343d60286fbe3c3989c9cb164`; this checkpoint is its child.

