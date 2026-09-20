# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-20T07:58:01-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 3 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `UNNAMED`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `936438b4-6acf-4417-9513-2bb938fe60e0`.

The coordinator has no agent name, so workers cannot `herdr agent prompt` it. Name it: `herdr agent rename "$HERDR_PANE_ID" coordinator`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0112 | pi | done | `w1G:p33` | `w1G:t2Z` (t-0112) | `/home/agent/projects/herdr-ade/.worktrees/t-0112` | done=1 lane=t-0112 project=adeherdr rank=1 review=ready-for-review thread=t-0112 | π - t-0112 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0112 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (5 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 594793f | 0 | docs(review): round r52 verdict |
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
| `/home/agent/projects/herdr-ade/.worktrees/review-r25` | `review/r25` | fe84ff3 | 0 | docs(tasks): t-0064 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r26` | `review/r26` | bf51381 | 0 | docs(tasks): t-0061 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r28` | `review/r28` | 10ad3c3 | 0 | docs(tasks): t-0066 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r29` | `review/r29` | 4378dda | 0 | docs(tasks): t-0067 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r30` | `review/r30` | b63449f | 0 | docs(tasks): t-0068 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r32` | `review/r32` | a215f16 | 0 | docs(tasks): t-0073 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r33` | `review/r33` | fa0436a | 0 | docs(tasks): t-0074 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r34` | `review/r34` | cfee43f | 0 | docs(tasks): t-0075 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r35` | `review/r35` | 15960a3 | 0 | docs(tasks): t-0084 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r36` | `review/r36` | 41a4d7b | 0 | docs(tasks): t-0087 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r37` | `review/r37` | af39a21 | 0 | docs(tasks): t-0088 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r38` | `review/r38` | 66f96e4 | 0 | docs(tasks): t-0090 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r39` | `review/r39` | 1f8d2f9 | 0 | docs(tasks): t-0091 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r4` | `review/r4` | 689e079 | 0 | docs(tasks): t-0011 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r41` | `review/r41` | 4e12822 | 0 | docs(tasks): t-0093 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r42` | `review/r42` | f20dd08 | 0 | docs(tasks): t-0095 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r43` | `review/r43` | 24aff4a | 0 | docs(tasks): t-0096 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r44` | `review/r44` | 121aa10 | 0 | review(r44): brief for revision 1 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r45` | `review/r45` | ee6a42f | 0 | docs(tasks): t-0100 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r46` | `review/r46` | 43beada | 0 | docs(tasks): t-0101 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r47` | `review/r47` | ca5d598 | 0 | docs(tasks): t-0103 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r48` | `review/r48` | e681ae4 | 0 | docs(tasks): t-0104 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r49` | `review/r49` | 5796465 | 0 | docs(tasks): t-0107 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r5` | `review/r5` | 008f9ac | 0 | docs(tasks): t-0012 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r50` | `review/r50` | a00f467 | 0 | docs(tasks): t-0109 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r51` | `review/r51` | 7a5d5f6 | 0 | docs(tasks): t-0111 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r52` | `review/r52` | a485e93 | 0 | docs(tasks): t-0113 |
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
| `/home/agent/projects/herdr-ade/.worktrees/t-0050` | `hp/adeherdr/t-0050-lanes-and-reviews-go-to-the-box-by-defau` | 6c4e315 | 0 | checkpoint(r24): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0051` | `hp/adeherdr/t-0051-a-finished-lane-s-line-always-reaches-th` | 3c979e2 | 0 | checkpoint(r22): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0052` | `hp/adeherdr/t-0052-review-r22-this-round-checks-the-rule-th` | f934860 | 0 | docs(review): round r22 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0054` | `hp/adeherdr/t-0054-every-coordinator-may-evolve-the-harness` | 4177ce0 | 0 | checkpoint(r25): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0055` | `hp/adeherdr/t-0055-a-new-session-picks-up-box-lanes-and-can` | 38ab23d | 0 | checkpoint(r26): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0056` | `hp/adeherdr/t-0056-review-r23-this-round-checks-the-fix-tha` | 7e07548 | 0 | docs(review): round r23 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0057` | `hp/adeherdr/t-0057-project-screen-lane-2-the-overview-above` | 333cca6 | 0 | checkpoint(r29): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0059` | `hp/adeherdr/t-0059-review-r24-the-rule-that-sends-new-lanes` | 3938a05 | 0 | docs(review): round r24 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0060` | `hp/adeherdr/t-0060-review-r25-this-round-checks-the-rule-th` | 9a19ab6 | 0 | docs(review): refresh round r25 verdict after r26 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0061` | `hp/adeherdr/t-0061-review-r26-this-round-checks-the-fresh-s` | 364f963 | 0 | docs(review): refresh round r26 verdict after r24 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0064` | `hp/adeherdr/t-0064-repair-r25-merge-the-placement-round-int` | be7409b | 0 | Merge r24 box placement with r25 repository policy |
| `/home/agent/projects/herdr-ade/.worktrees/t-0066` | `hp/adeherdr/t-0066-review-r28-a-rejected-or-failed-review-s` | f9d1f3c | 0 | docs(review): round r28 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0067` | `hp/adeherdr/t-0067-review-r29-this-round-checks-the-project` | ad94e70 | 0 | review(talk): round r29 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0068` | `hp/adeherdr/t-0068-review-r30-this-round-checks-the-way-a-f` | 751bdd9 | 0 | docs(review): round r30 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0073` | `hp/adeherdr/t-0073-review-r32-this-round-checks-that-every` | d775cd4 | 0 | docs(review): round r32 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0074` | `hp/adeherdr/t-0074-review-r33-this-round-checks-that-the-cl` | 00883cb | 0 | docs(review): round r33 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0075` | `hp/adeherdr/t-0075-review-r34-this-round-checks-that-a-roun` | ded4348 | 0 | docs(review): round r34 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0083` | `hp/adeherdr/t-0083-research-how-the-picker-should-classify` | 9a7793e | 0 | docs(tasks): t-0083 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0084` | `hp/adeherdr/t-0084-review-r35-this-round-checks-the-cost-th` | 7767878 | 0 | docs(review): round r35 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0087` | `hp/adeherdr/t-0087-review-r36-this-round-checks-that-a-chec` | fdbdfdd | 0 | docs(review): round r36 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0088` | `hp/adeherdr/t-0088-review-r37-this-round-checks-four-small` | dc942f3 | 0 | docs(review): round r37 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0090` | `hp/adeherdr/t-0090-review-r38-this-round-checks-the-record` | 6bff331 | 0 | docs(review): round r38 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0091` | `hp/adeherdr/t-0091-review-r39-this-round-checks-the-box-rea` | 51b756a | 0 | docs(review): round r39 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0093` | `hp/adeherdr/t-0093-review-r41-this-round-checks-that-you-ca` | 330cb37 | 0 | docs(review): round r41 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0095` | `hp/adeherdr/t-0095-review-r42-this-round-checks-that-one-re` | 8fbfef7 | 0 | docs(review): round r42 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0096` | `hp/adeherdr/t-0096-review-r43-this-round-checks-that-the-ta` | 0b5a0f7 | 0 | docs(review): round r43 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0098` | `hp/adeherdr/t-0098-calibrate-the-picker-on-this-project-s-o` | 69cc1b0 | 0 | checkpoint(r47): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0112` | `hp/adeherdr/t-0112-d12-a-lane-goes-to-a-machine-that-can-ac` | 657256f | 0 | fix(dispatch): place picks only on ready machines |

Last commits on the integration branch:

```
594793f docs(review): round r52 verdict
e749017 Merge commit '657256fee7ba3931856c43ecab4f75b5fefc7c6a' into hp/adeherdr/t-0113-review-r52-this-round-checks-that-work-g
a485e93 docs(tasks): t-0113
cf632d0 review(r52): brief for revision 1
dbe088e checkpoint(r51): HANDOFF after merging the round
3c0afaf Merge commit '837f62a4d9b15360eaa77192a86a50188a8d6669'
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0113.md`, `tasks/review-r52.md`, `tasks/t-0111.md`, `tasks/t-0112.md`, `tasks/review-r51.md`, `tasks/t-0110.md`, `tasks/t-0109.md`, `tasks/review-r50.md`, `tasks/t-0108.md`, `tasks/t-0107.md`, `tasks/review-r49.md`, `tasks/t-0106.md`
- verdicts: `tasks/reviews/code-r52.md`, `tasks/reviews/code-r51.md`, `tasks/reviews/code-r50.md`, `tasks/reviews/code-r49.md`, `tasks/reviews/code-r48.md`, `tasks/reviews/code-r47.md`, `tasks/reviews/code-r46.md`, `tasks/reviews/code-r45.md`, `tasks/reviews/code-r43.md`, `tasks/reviews/code-r42.md`, `tasks/reviews/code-r41.md`, `tasks/reviews/code-r38.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 936438b4-6acf-4417-9513-2bb938fe60e0
```

Round `r52` was merged into `main` at verdict commit `594793f95d30ad6b1e8c34572b68afaf9a9cef18`; this checkpoint is its child.

