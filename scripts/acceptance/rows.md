# Required-row list (SPEC-ADE §4.3)

Every row below is required except optional kind rows inside item 9 (agy,
opencode, dsh, and unverified cursor/codex hook labels). A required row
that cannot run is FAIL, not NOT-RUN. This A0 skeleton marks every row
NOT-RUN because the verbs are not implemented yet.

| id | required | title |
|---|---|---|
| 1 | yes | ha new and open with claude coordinator: hook before launch, bootstrap acknowledged |
| 2 | yes | round open r1; claude and cursor lanes admitted: tabs, parent, briefs committed first |
| 3 | yes | each lane ha done seals events; peek does not acknowledge; unadmitted ha waiting from a dirty tree |
| 4 | yes | representative-topology handoff continuity and blocked-then-submitted delivery |
| 5 | yes | ha round review: brief commit B precedes review worktree; verdict V; ha done --sha V |
| 6 | yes | ha round merge stop after merged; resume to H; item 32-34 fixtures |
| 7 | yes | negative rows: dirty done, wrong sha, unstable report, incomplete review, stale merge verdicts |
| 8 | yes | pro-mcp start, passive adopt, one TURN, one artifact event, ha dialogue commit |
| 9 | yes (claude, cursor) | kind rows: claude and cursor required; others NOT-RUN when unavailable |
| 10 | yes | plain-language rows: --plain, hook rewrite, talk captures, ha ask, board tokens, GLOSSARY.md |
| 11 | yes | throwaway server stop; ha doctor reports unreachable; records intact |

Print format: `STATUS<TAB>id<TAB>title`, then one `REQUIRED` summary line.
Exit 0 only when every required row is PASS.
