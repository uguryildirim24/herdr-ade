# Harness failure ledger

In a project's coordinator workspace or lane:

```sh
ha ledger list          # open failures, highest repeat count first
ha ledger list --json
ha ledger show f-0001   # full evidence, including closed failures
ha ledger task f-0001   # task brief on stdout; does not start work
ha ledger done f-0001   # close after the fix has been checked and landed
```

For example, from the coordinator workspace:

```sh
ha ledger task f-0001 | ha thread start demo \
  --title "Fix the failed work check" --repo /path/to/herdr-ade \
  --plain "This work fixes the check that could not start." --task-file -
```

The project resolves from the current workspace or lane launch binding. The
ledger never creates a lane. It closes an observed failure automatically when
the same condition answers successfully; `ledger done` records a checked manual
closure. A designed refusal
(a safety or authority check that intentionally rejects the requested action,
or a completed doctor report that refuses to declare the system healthy)
still exits unsuccessfully, but its structural refusal marker keeps it out of
the failure ledger. Errors are never classified by matching message text.

## One durable record, derived views

`<project>/ledger.jsonl` is an append-only journal guarded by
`<project>/.state/ledger.lock`. Each failure has an id, first and last observation
times, kind, subject, full error evidence, repeat count, and closed state.
Repeated `kind + subject + normalized detail` appends a revision of the **same
id**, not a new failure. Lists and task briefs replay the latest revision per id;
older evidence remains in the journal. Normalization folds whitespace and ANSI
CSI formatting, not error codes, numbers, paths, or case.

Closing is idempotent and records `closed_at`. A new occurrence reopens the
same id and retains its first observation and cumulative count. A successful
re-entry closes the original entry; there is no separate retry entry. Recovery
and context-read cursors are journal facts, not extra writable records.
Malformed or torn journal rows produce an error; they are never silently dropped.

## Observation sites

- Unexpected CLI failures and nonzero, timed-out, or unspawnable child commands
  through the ADE runner, preserving their original outcomes. Designed CLI
  refusals and predicate commands whose nonzero status is a valid negative
  answer are not failures. A caller declares that contract with
  `ExitMeaning::Answer`; ambiguous probes retain `ExitMeaning::Required`.
  Git ancestry uses the narrower `ExitMeaning::Boolean`: 0 is yes, 1 with
  empty stderr is no, and anything else is an error. Its typed helper returns
  `Result<bool>` using the same decoder as the ledger.
  Missing tools, signals, and timeouts are failures under either contract.
  Optional Git branches/files use successful presence queries, not a blanket
  nonzero exemption. See the [subprocess audit](subprocess-audit.md) for counts
  and the mixed probes deliberately left eligible for recording. Child evidence
  includes argv, exit status, stdout and stderr, but not environment or stdin.
- Failed reviewer starts in `round advance`, even when advance returns success.
- Unexpected merge failures, including subprocess errors and conflicts. A
  structurally marked verdict or safety refusal is an outcome, not a failure.
- Unknown thread startup breakage and failed launches with zero attempts.
  Provider failures, lost connections, gone processes, and failed work stay on
  their typed thread/event records rather than becoming harness defects.
- Failed courier passes, attributed to each project carried by that pass, plus
  underlying SSH/transport command errors. A successful pass closes them.

The CLI installs the observing runner. Ticker and courier scopes explicitly bind
commands to the project(s) being processed; unrelated projects do not inherit the
coordinator's scope. No project is invented for an unscoped global command.
Recording failures warn on stderr if the ledger cannot be written, without
replacing the original operation's result. This records the harness's commands,
not arbitrary tool commands run inside an agent's own session.

`ha context` and the project screen show at most five open entries: repeated
failures or failures newer than the last non-peek context read, worst first.
Digest lines are capped at 220 characters. The screen uses plain descriptions
and stable failure references, not raw command errors; existing row clipping
is unchanged. `context --peek`, list, show, task and screen reads do not advance
the context cursor. Full evidence is available through `ledger show` and `task`.
