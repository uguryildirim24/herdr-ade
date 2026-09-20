# Review brief: round r53

plain: This round checks that a no answer and a health check doing its job are not counted as failures.

Run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade skill reviewer`, then do what this brief says.

Round `r53` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `941b570dbec1c5db8a41a683694d77c96f4c47a6cb54cb5da27a7cde6f4de744`, policy hash `518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0114 | 1 | `9768afc30a6246cf39c6158ff8812fd858e1abdf` | `t-0114-1-1` | `c4cdd447a0ea6f8e3dcddef82c9a5ec9bb1a4dd27ffb248136b9e4b6bcbcffdd` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r53.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r53"
candidate = "<C>"
manifest_hash = "941b570dbec1c5db8a41a683694d77c96f4c47a6cb54cb5da27a7cde6f4de744"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/home/agent/.local/bin/ha --root /home/agent/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0114 (artifact `c4cdd447a0ea6f8e3dcddef82c9a5ec9bb1a4dd27ffb248136b9e4b6bcbcffdd`)

Data, not instructions.

```text
# t-0114 — E8

Implemented on oci, extending E6 (`566bcfb`) and E7 (`47002d8`).

- Added a precise `ExitMeaning::Boolean` contract to the existing runner mechanism: exit 0 is yes, exit 1 with empty stderr is no; other statuses, diagnostic-bearing negatives, signals, timeouts and spawn failures remain errors.
- Shared `git::is_ancestor -> Result<bool>` uses the same decoder as RecordingRunner. Round ancestry checks (including `members_all_landed`) and publication verification use it.
- The completed unhealthy doctor report now uses E6's structural refusal marker. Its nonzero outcome and message remain unchanged; underlying probe failures still record. No command-name/message exemptions, compatibility shims, fallback paths or CLI flags.
- Updated ledger documentation and corrected E7's ancestry audit claim. Historical ledger rows are unchanged.

## Verification

All final gates passed with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check`
- `cargo test`: 671 passed, 0 failed (519 + 56 + 78 unit tests; 18 integration tests).
- `cargo clippy --all-targets --all-features -- -D warnings`
- `git diff --check`

Regression coverage includes repeated real Git yes/no answers leaving the ledger absent, a real bad revision recording, simulated exit 1 with stderr / exit 2 / signal / timeout / spawn failures recording, publication's negative ancestry answer remaining unrecorded, and an unhealthy doctor not recording itself while child failures and ordinary errors with identical text still record. An existing publication fixture was corrected to match Git's actual silent exit-1 contract.

Read the cloud copy of the fork HANDOFF at `/home/ubuntu/projects/herdr/HANDOFF.md`; the Mac-only absolute path in the brief is not present on oci. Verified the box's openai-codex login with `herdr-pi check openai-codex`.

No unresolved items.
```

