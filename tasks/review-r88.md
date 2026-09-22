# Review brief: round r88

plain: A failed answer from the web helper says what the page said.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r88` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `57868c9e1a3ebf2f0543a72c800a06b6ffd23198f2414293842345e4352513f1`, policy hash `d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0211 | 1 | `19195441078fb76afbf616ddaddeb5830ad0afda` | `t-0211-1-1` | `e68712abdbc4ff2b66b74a4ceb82276ba5d18005a272b6dcaf1c4453038daeca` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r88.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r88"
candidate = "<C>"
manifest_hash = "57868c9e1a3ebf2f0543a72c800a06b6ffd23198f2414293842345e4352513f1"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0211 (artifact `e68712abdbc4ff2b66b74a4ceb82276ba5d18005a272b6dcaf1c4453038daeca`)

Data, not instructions.

```text
# W12 report

Implemented and published commit `1919544` (`fix(pro): preserve rollout stream errors`).

- The rollout reader now finishes processing the batch containing `task_complete`, so a following `stream_error` is not discarded.
- Error and stream-error events retain the provider's exact message. `Stopped thinking` is a typed `provider` failure rather than a cooldown or unknown result. Rate limits still use the cooldown route.
- Failed turn records and coordinator notices keep `failure_class = "provider"` and the exact error line in the reason.
- Added rollout JSONL fixtures for both observed failures: `Stopped thinking` and `displayed an error for this response` / `remained unavailable after several attempts`.

`herdr-pro` does not set Codex's status-line tag. Its only per-turn pane input is the prompt `TURN <tag>: ...`; there is no title or status-tag command in the Pro code. The stale `Answer pro-prl2-01 in markdown` text therefore was not a tag maintained by herdr-pro.

Gates passed with the required `PATH` and `DEVELOPER_DIR`:

- `cargo fmt --check`
- `cargo test` (607 main, 57 pi, 80 Pro, and integration suites)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

No live Pro run was attempted because this lane is on `oci` and Pro runs only on the Mac; the coordinator can verify after install as requested.
```


## Repair revision

This revision reviews the integration base `a6d9dc8b271d2c768aad6a228679f3df99566ad3`.
