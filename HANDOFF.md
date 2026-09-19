# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-19T15:13:37-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 6 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/Users/rolfie/.herdr-ade/adeherdr`.

Coordinator session id `6c22d8cf-cae6-47e2-a930-fd9207175aea`.

### Workers nested under the coordinator

| name | kind | status | pane | tab (label) | cwd | tokens | last title |
|---|---|---|---|---|---|---|---|
| hp-adeherdr-t-0020 | pi | done | `w1G:pZ` | `w1G:tV` (t-0020) | `/Users/rolfie/projects/herdr-ade/.worktrees/t-0020` | done=1 lane=t-0020 project=adeherdr rank=1 review=ready-for-review thread=t-0020 | π - t-0020 |
| hp-adeherdr-t-0022 | pi | working | `w1G:p11` | `w1G:tX` (t-0022) | `/Users/rolfie/projects/herdr-ade/.worktrees/t-0022` | project=adeherdr rank=3 review=working thread=t-0022 | π - t-0022 |
| hp-adeherdr-t-0023 | pi | working | `w1G:p12` | `w1G:tY` (t-0023) | `/Users/rolfie/projects/herdr-ade/.worktrees/t-0023` | project=adeherdr rank=3 review=working thread=t-0023 | π - t-0023 |
| hp-adeherdr-t-0024 | pi | done | `w1G:p13` | `w1G:tZ` (t-0024) | `/Users/rolfie/projects/herdr-ade/.worktrees/t-0024` | done=1 lane=t-0024 project=adeherdr rank=1 review=ready-for-review thread=t-0024 | π - t-0024 |

Start lines as they run now (from `pane process-info`), for restarting a worker that is gone:

```bash
herdr agent start hp-adeherdr-t-0020 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0022 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0023 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
herdr agent start hp-adeherdr-t-0024 --kind pi --pane <new pane> --parent "$HERDR_PANE_ID"
```

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (working), `w1J` Venator (idle), `w1K` Elicio (idle)

### Git

Repo `/Users/rolfie/projects/herdr-ade`, integration branch `main` (15 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/Users/rolfie/projects/herdr-ade` | `main` | e8091fd | 0 | docs(review): round r10 verdict |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r1` | `review/r1` | fc61993 | 0 | docs(tasks): t-0004 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/review-r10` | `review/r10` | f9ae7d2 | 0 | docs(tasks): t-0024 |
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
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0018` | `hp/adeherdr/t-0018-harness-the-context-tells-a-coordinator` | 1e9f356 | 0 | feat(round): advance starts the reviewer through a herdr hook |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0019` | `hp/adeherdr/t-0019-herdr-pro-pictures-from-screenshots-and` | 8e3ccf1 | 0 | feat(pro): nest lanes under the caller in herdr's agent tree |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0020` | `hp/adeherdr/t-0020-herdr-pro-serve-pro-through-a-local-rela` | 8e55c49 | 0 | feat(pi): setup writes the pro provider and pi_pro joins the rows |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0022` | `hp/adeherdr/t-0022-review-r9-box-lanes-start-side` | 6a99a3f | 3 | Merge commit '6d517cb2060b13fc90ec4b8642b63a0ae0cff4fd' into hp/adeherdr/t-0022-review-r9-box-lanes-start-side |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0023` | `hp/adeherdr/t-0023-box-lanes-completion-side-the-courier-br` | d64488a | 11 | docs(tasks): t-0023 |
| `/Users/rolfie/projects/herdr-ade/.worktrees/t-0024` | `hp/adeherdr/t-0024-review-r10-the-harness-starts-its-own-ch` | e8091fd | 0 | docs(review): round r10 verdict |

Last commits on the integration branch:

```
e8091fd docs(review): round r10 verdict
b41e9d8 review(pro): start a fresh lane for picture references
0a0958e review(round): keep one reviewer and read hook envelope
999ee37 Merge commit '8e3ccf1163ed758f513c5fd6992b20d8306bc5a2' into hp/adeherdr/t-0024-review-r10-the-harness-starts-its-own-ch
f9b455f Merge commit '1e9f356804f1e1907b4e73f4a64402772835b5d7' into hp/adeherdr/t-0024-review-r10-the-harness-starts-its-own-ch
f9ae7d2 docs(tasks): t-0024
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0024.md`, `tasks/review-r10.md`, `tasks/review-r9.md`, `tasks/t-0020.md`, `tasks/t-0019.md`, `tasks/t-0018.md`, `tasks/t-0017.md`, `tasks/t-0015.md`, `tasks/t-0014.md`, `tasks/review-r7.md`, `tasks/review-r6.md`, `tasks/t-0013.md`
- verdicts: `tasks/reviews/code-r10.md`, `tasks/reviews/code-r7.md`, `tasks/reviews/code-r6.md`, `tasks/reviews/code-r5.md`, `tasks/reviews/code-r4.md`, `tasks/reviews/code-r2.md`, `tasks/reviews/code-r1.md`, `tasks/reviews/code-plugin-r2.md`, `tasks/reviews/code-plugin-r1.md`, `tasks/reviews/code-plugin-r1-b.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /Users/rolfie/.herdr-ade/adeherdr && claude --resume 6c22d8cf-cae6-47e2-a930-fd9207175aea
```

Round `r10` was merged into `main` at verdict commit `e8091fd22760655cd0a1fd0c6f28b00c941b75b9`; this checkpoint is its child.

