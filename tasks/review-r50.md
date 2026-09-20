# Review brief: round r50

plain: This round checks that a question the harness asks is answered without looking like something broke.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r50` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `8eb29a587e0fb9c5df16aff55d460e8b6835e3899023e5c1e7fcd43410331324`, policy hash `518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0108 | 1 | `47002d8f9234d828db85fcb8b1ed30bb8cd04ad6` | `t-0108-1-1` | `8675574a52aafeb6068d19f1bbb3c13567aa6cb13127f3c2bef4e1ea7eddf8d2` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r50.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r50"
candidate = "<C>"
manifest_hash = "8eb29a587e0fb9c5df16aff55d460e8b6835e3899023e5c1e7fcd43410331324"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0108 (artifact `8675574a52aafeb6068d19f1bbb3c13567aa6cb13127f3c2bef4e1ea7eddf8d2`)

Data, not instructions.

```text
# t-0108 — E7

Implemented and checked on `oci`.

## Changes

- Replaced the boolean subprocess exemption with the call-site type `ExitMeaning::{Required, Answer}`. Normal negative answers can be excluded; missing executables, signals, and timeouts still record.
- Optional local branch queries now return `Result<Option<String>>` from successful `git for-each-ref` output. Exact matching rejects prefix-only matches. Applied to cloud branch creation/retry, integration validation, restart, and the shared round/plan/checkpoint query.
- Optional committed files use `ls-tree` before `show`. Missing files are answers; invalid revisions and failed object reads remain failures.
- Removed the unsafe blanket Git probe exemption. Ancestry checks can fail for both “no” and broken Git inputs, so they remain recorded.
- Marked both `round_closed` guards, `round_output_pending`, and the two abandon guards as `DesignedRefusal`. Checked production changes since E6 (`566bcfb`): the changed publication guards already retain their markers.
- No historical ledger entries closed or rewritten. No install, compatibility shim, fallback, or CLI flag added.

## Subprocess audit and counts

Full inventory: `docs/subprocess-audit.md` (committed).

Inspected **68 production construction sites**: **65 commands** plus **3 runner/adapter implementation sites**. Command count is 54 Runner-backed and 11 direct lifecycle/login calls; shared constructors count once, not once per argument combination.

**0 production sites safely treat every normal nonzero exit as information; 65 retain failure semantics, including mixed probes.** Rather than pretend an unsuccessful Git call proves absence, five logical optional-query sites now obtain “no” through successful queries and express it as `Option`. The `Answer` contract is covered with a real, fixed shell-builtin predicate; it is deliberately not granted to ambiguous production commands.

Mixed probes retained: Git ancestry/merge-tree/repository and optional-origin discovery; gh/pi/Codex auth; Herdr reachability/state/pane reads; login-shell PATH checks; ps/kill liveness; SSH-wrapped probes; HTTP health checks. Each can also report tool, permissions, repository, transport, or startup failures. Standalone pi/Pro and direct lifecycle calls retain their existing observation boundary; this change does not invent project ledgers for them.

## Gates

All passed with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check`
- `cargo test`: **661 passed**, zero failed/ignored (509 + 56 + 78 + 6 + 8 + 2 + 2)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

Regression coverage: repeated negative predicate leaves the ledger absent; missing predicate executable records; timeout/signal record; absent local branch/file leave the ledger empty; prefix-only branch does not count as present; invalid repository/revision and broken ancestry probe record; actual abandon of a closed round reaches the CLI outcome recorder without creating a failure.

Durable lesson: optional does not mean infallible. Prefer successful presence queries with typed optional results; never blanket-exempt a command merely because one of its unsuccessful outcomes means “no”.
```

