+++
verdict = "MERGE"
round = "r59"
candidate = "31ffea0487f4448e8ec19957f8012b283dded858"
manifest_hash = "da4963233d66d4a8a0d1d40764fd1ca1f7508f9c1c7a84990a5ed37f39b1c7be"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++

# r59 review — MERGE

Reviewed on `oci`. Merged all four exact pinned commits into my own lane branch. B (`320fd6a`) and every pin are ancestors of C. No integration branch was moved.

## Per lane

- **t-0129 — talk screen:** accept with fixes. The pathname comparison could skip an atomic update on macOS; the permanent re-exec flag blocked later installs; inheriting raw mode caused the replacement to save the wrong terminal state. Updates now compare builds, guard only the attempted version, restore the terminal before exec, and recover the current screen if exec fails. A split mouse report now preserves the draft and cursor instead of clearing them. Running cost excludes failed/finished lanes. Dedupe now requires a real reviewer/member relationship instead of hiding independent tasks with equal prose; a blocked reviewer stays visible. A JSON server response without status flags is unknown, not current. The lane's abandoned-round, question-card, count, cost, date-divider and repeated-chat fixes pass together.
- **t-0131 — manual reviewer:** accept with fixes. Resolved the r58 conflicts by keeping automatic moved-base repair and its shared start/bind effect. The manual path now takes the same advance lock, refuses closed rounds or accepted verdicts before launching, and records failed starts through the shared retry path. Regression tests cover the orphan-launch and missing-failure-record defects. Existing placement already tries local after an unready default box; this lane need not add another fallback.
- **t-0134 — coordinator reachability:** accept. Restoration checks pane/workspace/tab/cwd, uses the recorded socket, and persists the repair count. Both open and ticker use it. Prompting defaults on while explicit `nudge = false` remains honored. Doctor checks the bound name and recomputes unread announcements against the live inbox. These changes pass alongside the prompt-capture changes.
- **t-0135 — citable human requests:** accept with provenance fixes. Expiring a delivery marker previously converted delayed harness text into a human request; expired markers now fail closed. Herdr can submit a native draft together with an automated line, so composite prompts containing a marked delivery now mint no authority and request a separate human message. Exact talk deliveries still reuse their ids, native Claude messages retain their text, and consequential decisions still require an existing basis. Documentation now accurately scopes native capture to Claude instead of promising it for every provider.

## Operational limits

- Run `ha open` after installation to refresh the existing Claude coordinator's hooks. Other coordinator kinds must use talk for citable requests; no unsupported hook event was invented.
- Use talk or `!native` while composing: prompting can submit a native half-draft. A mixed prompt is deliberately not permission; resend the human message separately.
- The header now counts open task-list entries and open questions, as requested by t-0129, rather than all active lanes.
- The canonical round record is not mirrored into this cloud bootstrap root: `ha round show adeherdr r59` returned `round_manifest_unavailable` (missing `.state/rounds/r59.toml`). Hashes above are from B; I could not independently check the current coordinator manifest. There was no evidence of a changed manifest. The coordinator's merge must validate it and arrange the normal repair review if the integration base has moved.
- No live coordinator, live install, or authenticated Claude session was changed or tested. The terminal probe used only a scratch root, a PTY, and a disabled herdr executable.

## Checks

The brief lists no project gates, hence `gates = []`. Additional checks run on C, Linux, with their final output:

```text
$ cargo fmt --check
(no output)
exit=0

$ cargo test
running 3 tests
test workflow_help_describes_policy_floor ... ok
test shipped_cases_run_offline_and_report_both_error_directions ... ok
test coordinator_cannot_select_a_role_recipe_or_model ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
exit=0
```

Full suite: **555 + 55 + 78 + 6 + 8 + 2 + 3 = 707 passed**, zero failures.

```text
$ cargo clippy --all-targets -- -D warnings
    Checking ratatui v0.30.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 29.34s
exit=0

$ git diff --check
(no output)
exit=0

$ cargo build --bin herdr-ade
   Compiling herdr-ade v0.1.0 (/home/ubuntu/projects/herdr-ade/.worktrees/t-0137)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 13.15s
exit=0

$ python3 /tmp/t0137-review/reexec_probe.py
PASS: first atomic install re-execed in the same process
PASS: second install re-execed; draft survived both updates
PASS: replacement inherited cooked mode; terminal echo and canonical input restored
exit=0
```

The PTY probe launched a review build from its installed pathname, typed a draft, atomically replaced it with the candidate build, then replaced it again with an instrumented executable. It checked the unchanged process id, draft across both updates, and ICANON/ECHO before and after exit. Its first attempt timed out because the probe expected the entire draft as contiguous output; ratatui paints per-key deltas. Corrected that probe assertion and reran successfully; no production change was needed.

Before the final suite I also ran `cargo test --bin herdr-ade talk::`: `52 passed; 0 failed; 503 filtered out`, exit 0. All logs and the isolated probe script are under `/tmp/t0137-review/` on this box.
