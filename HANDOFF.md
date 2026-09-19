# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-19T17:59:55-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 6 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `6c22d8cf-cae6-47e2-a930-fd9207175aea`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0042 | pi | done | `w1G:p1V` | `w1G:t1Q` (t-0042) | `/home/agent/projects/herdr/.worktrees/t-0042` | project=adeherdr rank=1 review=ready-for-review thread=t-0042 | π - t-0042 |
| hp-adeherdr-t-0046 | pi | done | `w1G:p1Z` | `w1G:t1V` (t-0046) | `/home/agent/projects/herdr-ade/.worktrees/t-0046` | project=adeherdr rank=1 review=ready-for-review thread=t-0046 | π - t-0046 |
| hp-adeherdr-t-0047 | pi | done | `w1G:p10` | `w1G:t1W` (t-0047) | `/home/agent/projects/herdr-ade/.worktrees/t-0047` | done=1 lane=t-0047 project=adeherdr rank=3 review=working thread=t-0047 | π - t-0047 |
| hp-adeherdr-t-0048 | pi | working | `w1G:p21` | `w1G:t1X` (t-0048) | `/home/agent/projects/herdr/.worktrees/t-0048` | project=adeherdr rank=3 review=working thread=t-0048 | π - t-0048 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0042 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0046 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0047 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0048 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (7 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | fdcb26b | 0 | docs(review): round r20 verdict |
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
| `/home/agent/projects/herdr-ade/.worktrees/t-0046` | `hp/adeherdr/t-0046-box-health-rows-bash-probe-on-linux-no-r` | 6e82cd7 | 0 | fix(pi): bash probe for the wrapper row and an absent pro relay row |
| `/home/agent/projects/herdr-ade/.worktrees/t-0047` | `hp/adeherdr/t-0047-review-r20-this-round-checks-the-health` | fdcb26b | 0 | docs(review): round r20 verdict |

Last commits on the integration branch:

```
fdcb26b docs(review): round r20 verdict
e6b7296 review(pi): keep doctor fakes honest under bash
d725fb8 Merge commit '6e82cd748fe54a7e37f93a091809da002d736d31' into hp/adeherdr/t-0047-review-r20-this-round-checks-the-health
babc448 docs(tasks): t-0047
f6f6f4e review(r20): brief for revision 1
6e82cd7 fix(pi): bash probe for the wrapper row and an absent pro relay row
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0047.md`, `tasks/review-r20.md`, `tasks/t-0046.md`, `tasks/t-0045.md`, `tasks/review-r19.md`, `tasks/t-0044.md`, `tasks/t-0043.md`, `tasks/review-r18.md`, `tasks/t-0041.md`, `tasks/t-0037.md`, `tasks/review-r15.md`, `tasks/t-0035.md`
- verdicts: `tasks/reviews/code-r20.md`, `tasks/reviews/code-r19.md`, `tasks/reviews/code-r18.md`, `tasks/reviews/code-r15.md`, `tasks/reviews/code-r14.md`, `tasks/reviews/code-r12.md`, `tasks/reviews/code-r13.md`, `tasks/reviews/code-r11.md`, `tasks/reviews/code-r9.md`, `tasks/reviews/code-r10.md`, `tasks/reviews/code-r7.md`, `tasks/reviews/code-r6.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 6c22d8cf-cae6-47e2-a930-fd9207175aea
```

Round `r20` was merged into `main` at verdict commit `fdcb26b28039a144a90f3fadd66210e654dcfb71`; this checkpoint is its child.

