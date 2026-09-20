# E7 subprocess audit

E8 correction: Git `merge-base --is-ancestor` has distinguishable outcomes.
Rounds and publication verification now share `git::is_ancestor`, returning
`Result<bool>` under `ExitMeaning::Boolean`: 0 means yes, 1 with empty stderr
means no, and every other outcome remains a failure. This adds one shared
construction site to the inventory below; it does not exempt ambiguous probes.
The counts below are the original E7 audit, not a new census. The completed
`ha doctor` nonzero report now carries E6's structural refusal marker; failures
of its child probes remain eligible for recording.

Count unit: a production command-construction site (`Cmd::new` or
`Command::new`), not each invocation, loop iteration, or argument combination.
Tests are excluded. Shared command constructors are counted once; their callers
were also inspected. This includes ADE, pi, and Pro sources.

**68 sites inspected: 65 command sites and 3 transport implementations.**
Of the 65, **0 can safely treat every normal nonzero exit as information;
65 retain failure semantics** (including ambiguous probes). Of these, 54 use a
Runner and 11 are direct process lifecycle/login calls. The 3 implementation
sites are ADE's RealRunner, pi's RealRunner, and pi's ADE adapter; they do not
constitute additional commands.

This is intentionally not an exit-code allowlist. Calling something a probe
cannot make a broken repository, authentication store, shell, or SSH connection
an answered question. `ExitMeaning::Answer` is the explicit contract for a caller
that really knows all normal statuses are answers; spawning errors, signals,
and timeouts remain failures even under that contract. Regression tests exercise
that contract using a fixed shell builtin predicate, including a missing tool.
No current production caller is given that blanket exemption.

## Negative answers without failed commands

Five logical query sites formerly used failure as absence:

- `threads::ensure_branch` (including cloud dispatch/retries)
- `threads::place_ade_worktree` (integration branch validation)
- `threads::restart` (branch-existence decision)
- `round::repo::Git::branch_head` (shared by round, plan, and checkpoint reads)
- `round::repo::Git::show_file` (optional committed file)

The first four now share `git::branch_head`: `for-each-ref` returns a successful
empty result for absence, represented as `Result<Option<String>>`. Exact full
ref matching prevents a child branch from satisfying a prefix query. Nonzero
still records a failed query and propagates the error; it no longer authorizes
creating a branch after an arbitrary git failure.

The fifth uses `ls-tree` to ask presence, then `show` only for a present file.
A missing file is `Ok(None)`; invalid revisions, unreadable objects, missing git,
and timeouts are errors, not absent files. This also removes the old blanket
`probe_in` exemption, which hid failures of `merge-base`, `rev-parse`, and `show`.

## Complete construction-site inventory

All rows retain failure semantics, including the mixed probes explained below.
Line numbers are deliberately omitted; function names survive formatting.

| File | Sites | Commands / constructors |
|---|---:|---|
| `adopt.rs` | 2 | optional pane git facts; repository detection |
| `bin/herdr-pi.rs` | 1 | interactive wrapper login |
| `doctor.rs` | 2 | tool versions; gh auth status |
| `git.rs` | 2 | shared git helper; temporary-index writes |
| `harness.rs` | 4 | cargo build; cp; mv; installed version |
| `herdr.rs` | 2 | bare CLI; session/machine-bound CLI |
| `jev.rs` | 1 | curl request |
| `launch.rs` | 2 | repository facts; agent-start help |
| `ops.rs` | 4 | ls-remote; status; HEAD; kill -0 |
| `pi/doctor.rs` | 5 | wrapper version; two integration status sites; auth check; shell PATH query |
| `pi/install.rs` | 2 | npm install; integration install |
| `pi/sh.rs` | 2 | kill process group; login-shell commands |
| `pr.rs` | 1 | gh pr view |
| `pro/bridge.rs` | 2 | curl health; curl POST |
| `pro/doctor.rs` | 1 | codex login status |
| `pro/herdr_cli.rs` | 2 | shared CLI call; pane read |
| `pro/serve.rs` | 6 | kill -0; relay spawn; TERM; curl models; codex spawn; KILL |
| `pro/turn.rs` | 1 | collector spawn |
| `remote.rs` | 7 | machine list; git remote; get-url; two SSH sites; two SCP sites |
| `round.rs` | 1 | shared git constructor (also dialogue/checkpoint) |
| `routine.rs` | 1 | user-approved shell command |
| `runner.rs` | 2 | TERM and KILL process group |
| `steps.rs` | 1 | publication fetch / ancestry |
| `talk/stale.rs` | 5 | client status; version; ps list; ps pid; server status |
| `thread.rs` | 2 | du; rsync |
| `threads.rs` | 3 | shared git helper; create branch; push branch |
| `ticker.rs` | 1 | ticker spawn |
| **Total** | **65** | |

## Mixed probes: still eligible for recording

- Git `merge-tree` mixes conflicts with repository/tool failures. Ancestry
  is no longer in this category (see the E8 correction above).
- Git repository detection, optional origin URL, detached-HEAD detection, and
  checkpoint discovery: absence/non-repository/detachment and unreadable paths,
  configuration, or repository damage overlap. The shared constructors are not
  exempted. Required Git reads and all Git writes remain failures too.
- `gh auth status`, pi auth check, and Codex login status: a logged-out user is
  not proof that the credential store or provider check worked.
- Herdr reachability/status, session and pane reads (including a disappeared
  tab): missing state and a broken/unreachable CLI or server overlap.
- Shell PATH queries (`command -v`, `type`, `which` through login shells): absent
  executable and shell/startup-script failure overlap. Version/help/npm-root
  calls must themselves succeed.
- `ps -p` and `kill -0`: absent process and permissions/OS/tool failures overlap.
  Process-group cleanup also races child exit; it is not a guaranteed answer.
- SSH-wrapped doctor/readiness/existence checks: remote negative answers and
  SSH/auth/transport/shell failures overlap. SCP/rsync/du are actual operations.
- HTTP health/models checks: negative availability and curl/network/HTTP/tool
  errors overlap. Ordinary HTTP requests, builds, installs, agent/helper starts,
  interactive login, and user-approved routines retain failure semantics.

“Eligible” means observed when run through ADE's RecordingRunner in a project
scope. Standalone pi/Pro and direct helper lifecycle calls do not acquire a new
project ledger in this change. Existing timeout cleanup is not recursively
recorded as a second command failure; the timed-out command is recorded.

## Refusal audit

Both `round_closed` sites now carry `DesignedRefusal`, as do the adjacent
`round_output_pending`, missing-abandon-reason, and abandon-during-merge guards.
The new integration-branch absence result is also a designed refusal.
Changes since E6 (`566bcfb`) added no other unmarked production refusal: the two
publication guards changed their repair instructions and already carry the
marker. Unexpected round-record, filesystem, and subprocess errors stay ordinary
errors. Historical ledger rows are not rewritten or closed by this change.
