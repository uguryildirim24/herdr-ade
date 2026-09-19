# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-19T14:23:30-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 4 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `6c22d8cf-cae6-47e2-a930-fd9207175aea`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0014 | pi | done | `w1G:pP` | `w1G:tJ` (t-0014) | `/home/agent/projects/herdr-ade/.worktrees/t-0014` | done=1 lane=t-0014 project=adeherdr rank=1 review=ready-for-review thread=t-0014 | π - t-0014 |
| hp-adeherdr-t-0015 | pi | working | `w1G:pS` | `w1G:tN` (t-0015) | `/home/agent/projects/herdr-ade/.worktrees/t-0015` | project=adeherdr rank=3 review=working thread=t-0015 | π - t-0015 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0014 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0015 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (13 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | f32aa85 | 0 | Merge commit '8bbc45191a635039dcc9c61eaed83953997ae1a0' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r1` | `review/r1` | fc61993 | 0 | docs(tasks): t-0004 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r2` | `review/r2` | b07aa74 | 0 | docs(tasks): t-0005 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r2-2` | `review/r2-2` | c191a33 | 0 | docs(tasks): t-0007 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r4` | `review/r4` | 689e079 | 0 | docs(tasks): t-0011 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r5` | `review/r5` | 008f9ac | 0 | docs(tasks): t-0012 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r6` | `review/r6` | 910649d | 0 | docs(tasks): t-0014 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r7` | `review/r7` | 7cb564e | 0 | docs(tasks): t-0015 |
| `/home/agent/projects/herdr-ade/.worktrees/t-0001` | `hp/adeherdr/t-0001-plain-language-check-everyday-words-pass` | c482aa9 | 0 | checkpoint(r1): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0003` | `hp/adeherdr/t-0003-herdr-pro-the-pro-bridge-plugin-spec-pro` | 2f603cf | 0 | checkpoint(r2): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0004` | `hp/adeherdr/t-0004-review-r1-word-check-fix` | 356c0fa | 0 | docs(review): round r1 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0005` | `hp/adeherdr/t-0005-review-r2-herdr-pro` | ae593fb | 0 | review(pro): round r2 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0007` | `hp/adeherdr/t-0007-review-r2-second-pass-re-apply-the-herdr` | 65ba6ca | 0 | docs(review): round r2 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0008` | `hp/adeherdr/t-0008-harness-fixes-resolve-closes-the-pane-me` | 29bf80c | 0 | fix(round): merge lands when the integration branch moved |
| `/home/agent/projects/herdr-ade/.worktrees/t-0009` | `hp/adeherdr/t-0009-herdr-pro-v2-a-dedicated-pro-codex-home` | fded870 | 0 | checkpoint(r5): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0010` | `hp/adeherdr/t-0010-drop-the-jev-lane-picker-fix-the-model-p` | 5f94b7a | 0 | checkpoint(r4): HANDOFF after merging the round |
| `/home/agent/projects/herdr-ade/.worktrees/t-0011` | `hp/adeherdr/t-0011-review-r4-the-picker-is-gone-models-fixe` | 712ff7b | 0 | docs(review): round r4 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0012` | `hp/adeherdr/t-0012-review-r5-the-worker-s-own-small-home` | 6a77dd2 | 0 | docs(review): round r5 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0013` | `hp/adeherdr/t-0013-gpt-image-gen-a-picture-profile-on-the-b` | e45bb7a | 0 | docs(skill): tell lanes how to ask for a picture |
| `/home/agent/projects/herdr-ade/.worktrees/t-0014` | `hp/adeherdr/t-0014-review-r6-the-four-harness-fixes` | 8bbc451 | 0 | docs(review): round r6 verdict |
| `/home/agent/projects/herdr-ade/.worktrees/t-0015` | `hp/adeherdr/t-0015-review-r7-the-picture-maker` | da5963c | 4 | Merge commit 'e45bb7a8bec36a133ccbbb823e0d82e9a49626e9' into hp/adeherdr/t-0015-review-r7-the-picture-maker |

Last commits on the integration branch:

```
f32aa85 Merge commit '8bbc45191a635039dcc9c61eaed83953997ae1a0'
8bbc451 docs(review): round r6 verdict
89d545d review(r7): brief for revision 1
b694958 review(harness): reject unrecorded merge descendants
04fa700 review(harness): retain the moved merge head across retries
14de1cc review(harness): close panes after automatic resolution
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0014.md`, `tasks/review-r7.md`, `tasks/review-r6.md`, `tasks/t-0013.md`, `tasks/t-0012.md`, `tasks/review-r5.md`, `tasks/t-0011.md`, `tasks/review-r4.md`, `tasks/t-0010.md`, `tasks/t-0009.md`, `tasks/t-0008.md`, `tasks/t-0007.md`
- verdicts: `tasks/reviews/code-r6.md`, `tasks/reviews/code-r5.md`, `tasks/reviews/code-r4.md`, `tasks/reviews/code-r2.md`, `tasks/reviews/code-r1.md`, `tasks/reviews/code-plugin-r2.md`, `tasks/reviews/code-plugin-r1.md`, `tasks/reviews/code-plugin-r1-b.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/projects/herdr-ade/.worktrees/t-0014/target/release/herdr-ade --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 6c22d8cf-cae6-47e2-a930-fd9207175aea
```

Round `r6` was merged into `main` at verdict commit `8bbc45191a635039dcc9c61eaed83953997ae1a0`; this checkpoint is its child.

