# HANDOFF

## Goal

## Authority

## Settled

## In flight

## Open

## Next

## Traps

## Herdr (generated 2026-09-22T01:30:39-04:00 by herdr-ade checkpoint, herdr 0.9.1, session `default`)

Workspace `w1G` (Adeherdr), 2 tabs. Coordinator: pane `w1G:p1` in tab `w1G:t1`, agent name `hp-adeherdr-coordinator`, kind claude, status working, cwd `/home/agent/.herdr-ade/adeherdr`.

Coordinator session id `22e30006-6abc-4f92-9bcc-c002ce0ce442`.

### Workers nested under the coordinator

_none_

Other workspaces on this server (not yours to touch): `w1H` Flyonenomics (done), `w1N` PRL-8-53 (done)

### Git

Repo `/home/agent/projects/herdr-ade`, integration branch `main` (19 ahead, 0 behind origin/main).

| worktree | branch | head | dirty files | last commit |
|---|---|---|---|---|
| `/home/agent/projects/herdr-ade` | `main` | 21be177 | 0 | Merge commit '5fcd612f4eeb1d15de7170fae424668221c62ffb' |
| `/home/agent/projects/herdr-ade/.worktrees/review-r92` | `review/r92` | f4cbd52 | 0 | docs(tasks): t-0228 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r92-2` | `review/r92-2` | 8f3a91a | 0 | docs(tasks): t-0233 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r92-3` | `review/r92-3` | f8dd402 | 0 | docs(tasks): t-0236 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r92-4` | `review/r92-4` | 33fc226 | 0 | docs(tasks): t-0241 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r92-5` | `review/r92-5` | 7ecf091 | 0 | docs(tasks): t-0243 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r96` | `review/r96` | eb52279 | 0 | docs(tasks): t-0242 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r97` | `review/r97` | 1189bfa | 0 | docs(tasks): t-0244 |
| `/home/agent/projects/herdr-ade/.worktrees/review-r98` | `review/r98` | 5e45996 | 0 | docs(tasks): t-0245 |

Last commits on the integration branch:

```
21be177 Merge commit '5fcd612f4eeb1d15de7170fae424668221c62ffb'
7956959 docs(tasks): t-0247
c53deb0 docs(tasks): t-0246
db72c8a review(r98): brief for revision 1
5fcd612 review(harness): verdict for r92 revision 5
e60be73 Merge commit '41cc0a0e67eea2b15848093b07eaf1621bd860e3' into hp/adeherdr/t-0243-review-r92-installing-twice-on-the-same
```

### Record files (newest first)

- handoff: `HANDOFF.md`
- briefs: `tasks/t-0243.md`, `tasks/t-0241.md`, `tasks/t-0236.md`, `tasks/t-0233.md`, `tasks/t-0228.md`, `tasks/t-0247.md`, `tasks/t-0246.md`, `tasks/review-r98.md`, `tasks/review-r97.md`, `tasks/review-r92.md`, `tasks/t-0240.md`, `tasks/t-0232.md`
- verdicts: `tasks/reviews/code-r92.md`, `tasks/reviews/code-r95.md`, `tasks/reviews/code-r93.md`, `tasks/reviews/code-r94.md`, `tasks/reviews/code-r91.md`, `tasks/reviews/code-r90.md`, `tasks/reviews/code-r88.md`, `tasks/reviews/code-r89.md`, `tasks/reviews/code-r87.md`, `tasks/reviews/code-r86.md`, `tasks/reviews/code-r82.md`, `tasks/reviews/code-r85.md`

### Pickup

Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:

```bash
/home/agent/.local/bin/ha --root /home/agent/.herdr-ade pickup <slug>
# the old coordinator conversation, if herdr did not resume it in the pane:
# cd /home/agent/.herdr-ade/adeherdr && claude --resume 22e30006-6abc-4f92-9bcc-c002ce0ce442
```

Round `r92` was merged into `main` at verdict commit `5fcd612f4eeb1d15de7170fae424668221c62ffb`; this checkpoint is its child.

