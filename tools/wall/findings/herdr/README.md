# Historical Herdr contract findings

This historical report describes the builds below, not current test results. External evidence archives are not included. See [host requirements](../../README.md#host-requirements) before using the reproducers.

**Three confirmed findings, one separate suspicion.** Ranked by interruption cost
for Rolf. No production behavior changed; the sole Rust change is an ignored
regression test naming H02. This is reliability testing of our own harness in
its disposable sandbox. Known D21–D46 are not re-reported; H01 reproduces on a
ready lane and is independent of D26's working-turn delivery.

The complete source-call inventory and argument shapes are in [CALLS.md](CALLS.md).

## Build and evidence

All instance-4 findings used the lane's exact base:

- ADE commit `6051f43e94d790d1ec219410d0127a7aae4c86b2`;
  `herdr-ade 0.1.0+6051f43.1791098730`.
- Installed ADE SHA256:
  `02b40faf0a8d7ade113be95380b173ecca5dce5312b1823bb65e5a37b559240a`.
- Real `herdr 0.9.1`, SHA256:
  `0226645a3891b8a047e40dc22d68216c2674d9ea0e6769aa637e4aec3234b2cb`.
- The fake identifies its reference fork as `85a7743a`; the real executable's
  identity above is measured, not inferred from that source comment.

Runtime evidence is intentionally untracked, under the lane's recorded library:
`/path/to/evidence/` (abbreviated **L** below). Each
`evidence-*` directory contains `local.tar`/`box.tar` captured by `wall evidence`
before reset. The neighboring `.log` records commands and outputs without
requiring archive extraction. Guest logs are `runs/herdr-contract.jsonl` inside
`local.tar`; project records are under `.herdr-ade/wall/`.

Every repro begins with the selected instance's reset, prints EXPECTED/ACTUAL,
captures evidence, stops/deletes `scratch-t-0819`, and resets again. They require
an already installed build. Use only an assigned, unoccupied instance:

```sh
tools/wall/findings/herdr/repro-01 --instance 4
tools/wall/findings/herdr/repro-02 --instance 4
tools/wall/findings/herdr/repro-03 --instance 4
# Optional evidence location (must not already exist):
tools/wall/findings/herdr/repro-01 --instance 4 --evidence /absolute/new/directory
```

A nonzero repro is the expected result while its finding exists, **not** a
normal gate failure. These are discovery repros under `findings/`, not new
always-failing entries in the permanent fixed-regression gate. A fixing lane
can promote the relevant repro when it fixes the behavior.

## H03 — Courier observes the inherited socket instead of its requested session

**Priority:** high. The box observation can report an existing lane as absent,
which undermines unattended state/recovery decisions for named sessions.

**Build:** the measured base/build/hashes above.

**Exact steps:** `repro-03 --instance 4` resets (default session has no panes),
starts `scratch-t-0819`, creates a pane there, and queries its actual pane list.
It then drives the **real installed ADE courier endpoint** with
`HERDR_ADE_BOX_INPUT=1 ha doctor`, using its ordinary typed stdin request:

```json
{"build":"0.1.0+6051f43.1791098730","request":{"Courier":{"session":"scratch-t-0819","taken":[]}}}
```

That helper inherits `HERDR_SOCKET_PATH=/home/wall-4/.config/herdr/herdr.sock`.
This is a legal ambient default-session socket, not a damaged record. The
helper's requested named session is the scratch session. No SSH or provider
simulation is involved in the observation itself.

**Expected:** the courier's `Ready.result.panes` contains the requested session's
real pane `w1:p1`, with its cwd/tab/workspace identity.

**Actual:** it reports `Ready` with `panes=[]` and `agents=[]`, observing the
empty default session instead. `HERDR_SOCKET_PATH` wins over the
`HERDR_SESSION` environment value set by ADE. A successful answer from the
wrong server is indistinguishable from genuine lane absence at this boundary.
The fake's scripted list responses do not enforce this environment precedence.

**Record/log evidence:** `L/evidence-H03-verified.log:4-5` proves the scratch pane;
`:6` records the actual helper input, inherited socket and incorrect `Ready`
reply. `L/evidence-H03-verified/local.tar` retains
`runs/herdr-contract.jsonl:4-6`. Repro exit **1**. This is live-only observation
misrouting, not merely a hand-constructed Herdr CLI example.

**Separation:** both servers and the helper work; selecting the requested
session fails; the helper does not signal unknown/unreachable. A resulting
production retry/cleanup decision is **not** established by this repro.

**Likely code path:** `src/doctor.rs:434-450` (`observe_list`) sets
`HERDR_SESSION` but does not clear inherited `HERDR_SOCKET_PATH`;
`src/steps.rs:1090-1091` uses it for courier lists. The same precedence issue is
present in the courier screen command at `src/steps.rs:1163`, and doctor machine
probes also call `observe_list`. The main `Herdr::cmd` wrapper handles competing
environment correctly; these direct Runner paths bypass it.

**Smallest suggested fix:** select the requested session explicitly with
`--session`, or remove the inherited socket whenever setting `HERDR_SESSION`,
in both direct command constructors. Model this competing-environment case in
those callers' tests, rather than assuming the requested env value wins.

**Not tested:** the full saved-machine SSH journey into an actual erroneous
recovery decision, screen/cwd confusion where both sessions reuse the same pane
ID, or all doctor output consequences. Scope proven is the real courier helper's
wrong successful observation.

## H01 — Large file-based notes become permanently uncertain before delivery

**Priority:** high. A legitimate note read from a file can hold progress while
asking for manual confirmation of a delivery that provably never started.

**Build:** the measured base/build/hashes above.

**Exact steps:** `repro-01 --instance 4` resets; starts the real server in
`scratch-t-0819`; starts a real ADE coordinator and lane with the wall's scripted
agent; holds the lane at its working checkpoint; reports readiness through the
real lifecycle hook; waits until the original brief is acknowledged; writes a
131072-byte ASCII note; runs:

```text
ha thread prompt wall t-0001 --text-file /home/wall-4/large-note.txt
```

The ready state appears as `done` after prior work, which ADE intentionally
accepts alongside `idle`. This repro does **not** depend on mid-turn delivery.

**Expected:** file-based text reaches Herdr using a transport that can represent
it. At minimum, a definite exec failure before submission must not become an
uncertain delivery or an unreachable-server diagnosis.

**Actual:** the kernel refuses to start the Herdr client: `Argument list too
long (os error 7)`. ADE reports `(unreachable)` although `status server` succeeds.
The saved follow-up is `state = "uncertain"`, `delivered_at = ""`. The fake
accepts the same `agent prompt` argv because it models neither exec's per-string
limit nor this definite pre-submission failure. The subsequent delivery path
returns immediately for an uncertain note rather than retrying it.

**Record/log evidence:**

- `L/evidence-H01-final.log:6` records the real ready/done lane before the note.
- `:7` has the actual ADE command, exit 1 and E2BIG/unreachable message.
- `:8` summarizes `.state/threads/t-0001.toml` → `follow_ups[0]`, 131072 bytes,
  `state="uncertain"`, no delivery timestamp.
- `:9` proves that the same server is still reachable.
- `L/evidence-H01-final/local.tar` retains the exact record and
  `.state/inbox/*prompt-uncertain-t-0001*.md`.
- `L/matrix.json` contains the real/fake comparison: 65536-byte text succeeds;
  131072-byte text is accepted by the actual fake validator but cannot be exec'd.
- `L/repro-01-final.log`: `ACTUAL: E2BIG=True, uncertain=True,
  server_reachable=True`; repro exit **1**.

**Separation:** fixture/start/readiness succeeded; no client was launched for
the large note; the harness did not recover its uncertainty; delivery and
semantic completion of the note are **not** established.

**Likely code path:** `src/herdr.rs:698,717` puts all text in one argv entry;
`src/runner.rs:367` propagates spawn failure; `src/herdr.rs:403` maps every Runner
error to `unreachable`; `src/threads.rs:2357` persists uncertainty before the
call, `:2201,2389` does not classify this failure as definite refusal, and
`:2343` prevents uncertain-note retry.

**Smallest suggested fix:** use a stdin/file/socket text transport across the
Herdr boundary rather than argv for arbitrary note text; preserve definite
pre-spawn failures separately from ambiguous submission/connection failures.
Do not silently truncate the note or add an arbitrary text-size policy.

**Not tested:** Mac exec limits, remote SSH size thresholds, aggregate env/argv
limits, provider consumption of long text, or a repaired delivery path. This
is a Linux transport/state defect, not a claim about model behavior.

## H02 — Fake accepts notification titles that the real server rejects

**Priority:** low immediate impact; a concrete contract hole that permits a
future invalid call to pass unit tests.

**Build:** the measured base/build/hashes above.

**Exact steps:** `repro-02 --instance 4` resets and starts a scratch server;
submits `notification show TITLE --body 'wall contract'` for TITLE equal to
empty string, one space, newline and U+2003; runs the newly ignored test
`wall_h02_empty_notification_title` against the actual fake validator.

**Expected:** the fake rejects the same blank/whitespace-only titles as the
real server.

**Actual:** real Herdr exits 1 with
`{"error":{"code":"invalid_params","message":"notification title is empty"}}`;
fake validation succeeds. The ignored test fails on the first title.

**Record/log evidence:** `L/evidence-H02.log:4-7`,
`L/evidence-H02/local.tar` → `runs/herdr-contract.jsonl:4-7`, and
`L/evidence-H02-fake.log` (assertion: `EXPECTED fake rejection ... ACTUAL accepted
""`). Repro exit **101** from the deliberately failing ignored Rust test.

**Separation:** all four real calls reached server validation; no notification
was created. This is a confirmed real/fake mismatch, **not** a confirmed
production notification outage: current callsites use nonempty fixed prefixes.
There is no delivery recovery or model-semantic result to claim.

**Likely code path:** `src/runner/fake_herdr.rs:439` specifies only positional
arity/options and has no notification-title content validation;
`src/herdr.rs:756` forwards its title unchanged. Current production callers are
`src/actions.rs:160` and `src/steps.rs:1508` and do not produce blank titles.

**Smallest suggested fix:** add the real trimmed-empty-title validation to the
fake's notification branch; promote the ignored test after fixing it. No new
production guard is justified by this finding.

**Not tested:** graphical toast presentation, platform notification integration,
or any current ADE path that naturally generates an empty title (none found).

## Separate suspicion S01 — Hook-derived work state after server restart

`L/evidence-states.log:28-32` records `working`, a real server restart, then
`unknown`; the pi process PID remains **3465898** before and after restart.
An earlier exploratory run (`L/restart.log`) later reported `idle` while the
scripted process was still held at its working checkpoint. Thus retained
process identity does not imply retained hook state. A fake scripted stable
`agent list` response will not exercise this transition.

**Not a confirmed provider-backed defect:** the stand-in's screen is not real
pi's screen, so normal terminal-based state recovery could differ. Need a real
pi turn, server handoff, and its lifecycle/terminal evidence before concluding
ADE sends a notice or makes a recovery decision at the wrong time. No failure
repro or acceptance claim is assigned to this suspicion.

## Coverage and limits

**Complete source enumeration:** [CALLS.md](CALLS.md), including direct callers
outside the wrapper. No production Rust edits, wall/guest.py edits, provider
calls or permanent configuration changes.

**Live coverage (partial behavioral scope, not exhaustive):**

- `L/matrix.json`: **111** real calls compared with actual fake validation;
  long/Unicode/empty/dash-leading values, name boundaries, odd IDs, read sources
  and formats, wait bounds, metadata, keys and large prompts. Lookup errors
  are not misrepresented as static-validator mismatches.
- Real `ha new` → `ha open` with a 40-character slug (hashed agent name), and
  `Unicode 界🦊` display name; real lane start and file-based follow-up.
  `L/evidence-boundaries-final.log:4-8` and its archive.
- Start timeout 3000/300001/u64::MAX rejected; 3001 accepted but can exhaust its
  readiness budget, 300000 successfully starts the scripted agent. This
  distinction is important: accepted syntax is not successful readiness.
- Wait timeout 0 through u64::MAX succeeds for an already-ready agent. No
  enormous-duration waiter was left running.
- Launch failure: real `agent start --env PATH=<empty directory>` returns a
  startup timeout; `pane read --source visible` and `detection` expose
  `env: ‘pi’: No such file or directory`; process-info shows bash, not pi.
  `L/evidence-boundaries-final.log:25-28`. The screen contains the signature
  inspected by `src/coordinator.rs:795`; this checks the real screen contract,
  not a full coordinator failure/recovery journey.
- Hook-driven idle/done, working, blocked transitions; blocked prompt refusal;
  delayed ready→working during `agent prompt --wait` succeeds; a zero timeout
  can return timeout before the blocked-state refusal. No assumption of an
  atomic read-then-prompt operation. `L/evidence-states.log`.
- Closed tab/pane produces explicit `pane_not_found`, `agent_not_found`,
  `tab_not_found`; no successful empty-screen substitute. Restart preserves
  the foreground process but state recovery remains S01.

Additional recording scenarios (reset/capture/reset, not model acceptance):

```sh
tools/wall/findings/herdr/run-scenario transitions --instance 4
tools/wall/findings/herdr/run-scenario boundaries --instance 4
```

**Not exercised / unavailable scope:** full provider-backed pi lifecycle and
prompt consumption, sub-poll-width race exhaustiveness, every remote routing
combination, every metadata TTL/capacity boundary, action popup panes absent
from the wall manifest. Mac GUI, Claude trust screen and physical reboot are
unavailable on this Linux sandbox. No throwaway provider pane was started; if
a later check starts one, use `--model claude-haiku-4-5-20251001` as required by
the standing brief. D26 mid-work delivery is explicitly outside this result.

**Preserved behavior:** ordinary scripted coordinator/lane startup, valid
Unicode display text and dash-leading prompts, documented name/timeout
validation, real pane-read launch diagnostics, and closed-resource errors.
The normal Rust and wall gates remain the acceptance checks; the three discovery
repros intentionally fail while the findings remain. Final gate results and
clean-reset verification are recorded in the lane report.
