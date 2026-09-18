# Required-row list (SPEC-ADE §4.3)

Every row is required. Inside row 9 the unqualified kinds (codex, opencode,
agy, dsh) are optional parts: they may be NOT-RUN without failing the row.
A required row with any part that did not run is FAIL, not NOT-RUN.

Each row is assembled from parts (see the header of `run`):

- `det`: named unit tests of this checkout (FakeRunner, fault injection), read from one `cargo test --locked --bin herdr-ade` run.
- `mod`: tests another lane owns, matched by a name pattern for the new behaviour; zero matches is FAIL naming the lane.
- `cli`: the release binary against a throwaway root and a fixture repository; no herdr server.
- `live`: the throwaway herdr session (`ACC_LIVE=1`, `HERDR_BIN_PATH` = the qualified `C`) and real agent CLIs.

| id | required | title | det / mod / cli parts | live parts |
|---|---|---|---|---|
| 1 | yes | ha new and open with claude coordinator: hook before launch, bootstrap acknowledged | mod A2 receipt and hook install | `new --plain`, `open --session`, hook entry, `bootstrap = acknowledged` within 120 s |
| 2 | yes | round open r1; claude and cursor lanes admitted: tabs, parent, briefs committed first | cli round open refusal and record, GLOSSARY.md; mod A1 brief and launch record | `thread start --role` for a claude and a cursor lane, admit, `parent` tokens, briefs on `main` |
| 3 | yes | each lane ha done seals events; peek does not acknowledge; unadmitted ha waiting from a dirty tree | det pins from sealed done events of the current attempt; mod A2 done, waiting, seal | sealed done events per lane, `context --peek` acknowledges nothing |
| 4 | yes | representative-topology handoff continuity and blocked-then-submitted delivery | det talk delivery while blocked, uncertain never re-sent; mod A2 duplicate consumed once | NOT-RUN until `herdr server restart` (lane/restart-core) is in `C`; the 21-pane topology is scripted by the reviewer |
| 5 | yes | ha round review: brief commit B precedes review worktree; verdict V; ha done --sha V | det B before `review/r1`, brief contents, fence | `round review` on the live project; the reviewer thread produces `C` and `V` |
| 6 | yes | ha round merge stop after merged; resume to H; item 32-34 fixtures | det stop after merged, resume to H, no-op, item 33, item 34, D9 mechanics; mod A2 item 32 | none (deterministic row) |
| 7 | yes | negative rows: dirty done, wrong sha, unstable report, incomplete review, stale merge verdicts | det incomplete review, every bad verdict on its own fixture; mod A2 dirty/wrong sha/unstable report | none (deterministic row) |
| 8 | yes | pro-mcp start, passive adopt, one TURN, one artifact event, ha dialogue commit | det dialogue pinning, Pro recipient check, turn commit | needs a logged-in ChatGPT Pro pane and A1's `thread adopt --passive` |
| 9 | yes (claude, cursor) | kind rows: claude and cursor required; others NOT-RUN when unavailable | det talk header labels; mod A2 adapters | claude and cursor lanes from row 2; the other kinds optional |
| 10 | yes | plain-language rows: --plain, hook rewrite, talk captures, ha ask, board tokens, GLOSSARY.md | det checker R1-R7 and adversarial fixtures, ask, board, talk, glossary; cli ask refusal, record first, stale revision, term add refusal, board print; mod A1 thread plain, A2 hook | hook rewrite captures, board readback from the live workspace |
| 11 | yes | throwaway server stop; ha doctor reports unreachable; records intact | none | stop only the validated `acc-*` session, `doctor`, records intact |

Print format: `STATUS<TAB>id<TAB>title`, the parts indented below, then one
`REQUIRED` summary line. Exit 0 only when every required row is PASS.

The live parts were written against the verbs and flags SPEC-ADE names. On
the ade-rounds lane they were never executed: A1's `thread start --role`, A2's
`open` hook, receipt, `done` and `waiting` did not exist there. Each live part
checks its verb first and reports NOT-RUN naming the missing lane, so the
reviewer sees which part still needs the merged candidate.
