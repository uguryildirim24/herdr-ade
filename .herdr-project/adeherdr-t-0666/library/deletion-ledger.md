# Deletion ledger

All ranges refer to pinned source at `d69a2c8`. Physical-line counts include blank lines, comments and tests. “Net” is an engineering estimate after any replacement, **not** a promised diff or a pass criterion. Do not sum overlapping source regions within a finding. No runtime marker or private deployment state was inspected; whether Rolf's existing installation has completed a repair is unknown.

## M1 — Delete the live ask product, not historical answers

**Must fix before publishing · M · estimated net 1,100–1,450 lines.**

The coordinator's supported workflow is chat, not a structured question product (`skill/COORDINATOR.md:63,69`). Yet `ask` is fully live:

- Public CLI and parameters: `src/cli.rs:271-295,459-473,478-548`.
- Creation, duplicate-question checking, answers, withdrawals and publication: `src/ask.rs:38-100,144-220,252-415,418-650`.
- Extra `HumanMessage` enum with only one variant: `src/contracts.rs:379-385`.
- Ticker calls `ask::tick` at `src/ticker.rs:3256`; `tick` rescans asks and incomplete publications (`src/ask.rs:629-650`). A retrying publisher is not dead code just because skills stopped recommending the command.
- Plan nudge follows ask-to-task-to-step dependencies: `src/ticker.rs:2892-2903,2955-3039,3067-3074`. This is a second waiting/decision workflow layered on the coordinator's conversation.
- Active asks are projected into page, overview and digest: `src/project.rs:845`; `src/overview.rs:105-116`; `src/coordinator.rs:745-746,780-783,967-982`.
- Records/counters: `.state/asks/`, `.state/publications/`, `ask-counter.json`, `not-understood.json` (`src/ask.rs:30-35,64-68,376-378,506-509`).

**Proposal:** remove creation/closure commands, live publication, ask-aware nudging and current waiting widgets together. Use the existing chat request capture, not a new decision schema. Drop `HumanMessage`, `Publication`, publisher locks, question normalization, standing choice zero and their tests. `src/ask.rs` is 958 lines; expect to retain roughly 100–160 lines of historical decoding/answer lookup instead of deleting all evidence. Remaining removal comes from CLI and consumers/tests above.

**Fresh install:** safe after switching the public workflow to chat; no old asks exist. **Historical install:** leave immutable old asks/answers/withdrawals on disk and keep read support, especially `ask:a-N@revision` authority (`src/contracts.rs:462-493`; `src/note.rs:179-188`). Historical answer references must not become invalid. Existing open asks need to be surfaced to the coordinator as historical unresolved decisions during cutover, not silently called answered; their actual existence is unknown here. Do not preserve a writable compatibility command or an active notification loop just to keep old records readable.

## M2 — Rundown deletes meaning to satisfy a presentation preference

**Must fix before publishing · S · estimated net 100–150 lines.**

`technical_word` rejects tokens containing `/`, `_`, backticks, several delimiters, filename extensions, ID shapes and hashes (`src/rundown/view.rs:129-171`). `plain` literally removes those words (`:178-187`). Step conversion drops empty rows (`:29-45,85-89`), and `Card::count` counts the surviving rows (`:97-99`). Thus a label consisting solely of `README.md` or `CI/CD` can disappear from the list and its progress denominator. This consequence follows from the code; no live project was used to demonstrate it.

An existing test deliberately asserts:

```text
plain("Fix t-0508 so the tab in src/rundown shows.")
== "Fix so the tab in shows."
```

Source: `src/rundown/view.rs:615-644`. This is proof of destructive editorial behavior, not merely a subjective styling criticism.

**Proposal:** delete the lexical censor and content-dropping filters; render the stored title/step text with wrapping or truncation. Keep the pleasant panel, progress indicator and one-level subtasks. Keep attribution handling only if it is deliberately part of the supplied display field, not a heuristic that eats arbitrary parenthesized dates. Replace assertions that require missing words; retain width/layout tests (`:647-854`). Fresh/historical text stays readable; no data migration required. Do not delete the Rundown product.

## M3 — Remove contradictory operating history from skills

**Must fix before publishing · S · estimated net 40–65 lines.**

`skill/PI.md:46-48` says readiness makes no network refresh; current doctor performs live, cached tool-free model calls (`src/pi/doctor.rs:1-7,19-22,848-957`). `skill/PI.md:67-71` says explicit retry spends the bounded allowance and refuses after it; current CLI explicitly says only automatic retries are bounded (`src/cli.rs:122-125`), and manual retry uses `resolve_coordinator_retry` with `automatic = false` (`src/threads.rs:1762-1765`; `src/launch.rs:379-415`). Delete the “Rows this round” model catalog and disabled-row directions (`skill/PI.md:20-34`), then state current login, failure and restart behavior once. Remove repeated chat-choice prose from coordinator skill (`skill/COORDINATOR.md:63,69`). The lane/reviewer skills are already reasonably focused; see [inventory](inventory.md#skill-size-and-history).

Fresh-install safe. Historical data is unaffected. This is a rewrite of instructions, not a new runtime refusal or test requirement.

## S1 — Remove installation-history repairs from normal operation

**Should · M · estimated net 1,200–1,330 lines including retired tests/scripts.**

| Candidate | Where / measured removable region | Fresh install? | Existing-history caveat |
|---|---|---|---|
| Branch sweep v2 and old round cleanup | `src/branches.rs:78-94` (17 lines), `:473-916` (444 lines), sweep-only tests `:1607-1829` (223 lines); ticker call `src/ticker.rs:733-735` | Yes. Approximately **687 lines** plus tiny docs/import cleanup. `active_refs`, `Candidate`, `candidates`, `round_ref`, `round_worktrees`, `sweep_rounds` have no other production callers (command below). | Marker existence/completion is unknown. If an existing install still owes this cleanup, finish that maintenance before deleting the writer. Keep **ordinary** `resolved_thread`, branch leases and checked-out-branch protection (`src/branches.rs:118-182,301-471`). |
| Old tree-change reclassification | `src/review.rs:530-585` (56), two tests `src/review/tests.rs:563-664` (102), ticker call `src/ticker.rs:730-732` (3) | Yes, **161 lines**. New seals record change evidence; this function only corrects old cached true classifications and writes `.change-reclass-v1.json`. | Do not delete the resulting corrected historical evidence. Actual marker state is unavailable. |
| Saved-id/label startup-counter repair | `src/ticker.rs:2741-2785` (45), `src/thread.rs:101-104` (4), test `src/ticker.rs:4184-4302` (119) | Yes, approximately **168 lines**. Comment names the prior lookup bug; special state resets launch attempts once. | Complete or deliberately abandon affected old starts first. Keep normal `ProcessGone`/provider/start recovery; it solves a current problem. |
| Retry one obsolete gate-refusal sentence | `src/review.rs:1148-1157`, specifically the attention-string alternative; test `src/review/tests.rs:1839-1877` (39) | Yes, about **43 lines**. A past error string should not be a permanent transition condition. | Preserve the general extra-passing-gates policy and its current verdict tests (`src/review/tests.rs:1881-1942`). Retire only the one old-refusal repair. |
| Old-manifest `event review-advance` shim | `src/cli.rs:191-192,1192-1195,1927-1928,2366-2370` | Yes, about **15–20 lines** including the parser test. Current `herdr-plugin.toml:1-96` has no event hook. | Replace the loaded old manifest during normal installation; don't carry a no-op command forever. No historical record needs it. |
| Initial herdr 0.9.0 binary swap script | `scripts/migration/swap-binary.sh:1-113`, `swap-binary.test.sh:1-86`, `docs/operations.md:231` | Yes, **200 lines**. Backup filename hard-codes `herdr-0.9.0-...` (`swap-binary.sh:22`). | No code consumer found. Current installer calls `local_install` (`src/harness.rs:412-478,1282-1283`), not this script. A private external caller is unknown; no such consumer is named in the repo. Do not delete current live-handoff/install verification. |

Call-site commands and result summaries:

```text
$ rg -n 'candidates\(|active_refs\(|round_ref\(|round_worktrees\(|sweep_rounds\(' src/branches.rs
78: active_refs definition
481: candidates definition
567,586: candidates uses active_refs/round_ref
637,649: round_ref/round_worktrees definitions
674: round_worktrees uses round_ref
682: sweep_rounds definition
702,704: sweep_rounds uses round_worktrees/round_ref
767,778,824: sweep_once uses active_refs/round_ref
897,909: sweep_once calls candidates/sweep_rounds

$ git grep -n swap-binary
# Matches only docs/operations.md:231 and the script/test pair itself.
# No src/, plugin manifest or CI invocation.
```

These are **one-time writers**, not “dead forever because a marker was verified.” That verification was not possible from the repository.

## S2 — Delete real unused code and test-driven compatibility

**Should · S · estimated net 450–520 lines, non-overlapping with S1/S3/S4.**

### Unused pi launch/resume implementation

`src/main.rs:54-55` and `src/bin/herdr-pi.rs:11-16` blanket-allow dead code on path-included modules. A production-looking alternative start/restart path survives under that suppression:

- `src/pi/resume.rs:1-127`: no production caller outside the unused `agent_start_args`. It even claims `launch.resume_session`, absent from the actual `Launch` schema (`src/contracts.rs:95-140`).
- `src/pi/launch.rs:163-208`: `start_args`, `agent_start_args` and their docs, 46 lines.
- `src/pi/launch.rs:242-251`: `validate_env`, 10 lines, called only by a test. The live validator already rejects nonempty recipe env at `:280-282`.
- Exact-string start test `src/pi/launch.rs:334-372` (39), unused-env test `:425-435` (11), start/restart scenarios `src/pi/scenarios.rs:33-81` (49), module declaration `src/pi/mod.rs:22`.

These selected ranges total about **283 lines**, before trivial import cleanup. Search evidence:

```text
$ rg -n 'agent_start_args|append_resume_session|session_from_pane_get|validate_env\(' src tests
# Definitions in src/pi/launch.rs and src/pi/resume.rs;
# agent_start_args calls append_resume_session;
# every other match is a test in pi/launch.rs, pi/resume.rs, pi/scenarios.rs.
```

Real process creation is `src/herdr.rs:532-589`; the real parked-pi resume path appends `--session` at `src/threads.rs:3451-3456` and calls `agent_start_opts` at `:3521`. Keep both. Keep wrapper/trust/provider validation and actual setup/auth tests. Fresh and historical installs lose no runtime behavior by removing the unused path; old session files are still handled by the live path.

### Write-only / tests-only tails

- `Launch.compact_reason` is written at `src/launch.rs:556`, generated by an 80-character truncator at `:664-679`, and serialized/tested at `src/contracts.rs:128-131,724`. `rg -n '\bcompact_reason\b' src` finds no read. Comment claims an `ade_last` consumer, but no such consumer appears in this repo. Delete field/generator/constant/write, roughly **20–25 lines**; serde already tolerates extra old record fields. An external consumer is unknown, not established.
- Courier free-disk telemetry: shell emits `free` (`src/steps.rs:851-852`), parser stores it (`:936`) in `CourierManifest.free_bytes` (`:812`); test reads it (`:2109`); `CourierOutcome` omits it (`:827-835`). Delete emission/field/parser/test assertion, about **5 lines**. Real disk checks remain in `src/doctor.rs:505-600,2195-2228`.
- `src/worktrees.rs:132-155` accepts line-delimited porcelain solely because “Scripted tests written before status used `-z` still use lines.” Current producers use `-z` locally (`src/git.rs:198-217`) and remotely (`src/worktrees.rs:347-349`). Rewrite those existing fixtures with NUL records; remove approximately **10–20 lines** of production fallback. Preserve rename/copy handling. This is not historical persisted data.

### Delete the old scaffolding test that only tests its own canned answers

`src/scenarios.rs:2868-2958` defines `parse_json_stdout` and `ade_new_verb_scenarios_have_canned_herdr_replies`. It creates a fake runner, supplies constant replies and then asserts those replies; it never executes ADE start/done/waiting behavior. Its scenario names still include `ask` and the absent `say` command (`src/runner.rs:419-463`). `rg -n 'ADE_NEW_VERB|on_ade_new_verbs' src tests` finds only that fake helper/constants and this one test. Delete the test and its dedicated scaffolding, approximately **135–155 lines**. Keep actual command, seal and transport tests. This is retired test scaffolding, not production code to move into a unit module.

## S3 — Delete retired refusal policy and the disabled catalog entry

**Should · S · estimated net 190–260 lines with associated tests/docs; PI skill edits belong to M3.**

- Named `[roles]` / `requires_claude` refusal code: `src/launch.rs:82-101`; project roles/global gates special-case `src/project.rs:190-200`; old-key detector `:208-227`; its doctor call `src/doctor.rs:1118-1134`. Remove them and removed-feature tests such as `src/project.rs:1615-1627`, plus the old setup prose at `docs/operations.md:51`. Keep ordinary current-schema parsing; no replacement compatibility flag. Config is not historical task evidence.
- Cursor-in-pi prohibition and npm-tree archaeology: `src/pi/launch.rs:223-232`; `src/pi/doctor.rs:514-526,1167-1201`; related test cases `src/pi/launch.rs:382-400`. The code explicitly calls the feature “outside pi; native cursor lanes only, then retired.” Delete the special scan and one-provider refusal. This does **not** mean adding a Cursor provider, re-enabling it or deleting the native adapter. Generic runtime/provider support remains responsible for what can actually run.
- Exactly one of seven shipped recipe rows is disabled: `assets/default-recipes.toml:36-43`, `pi_codex_astra_xhigh`. Delete its **8 lines** from shipped data. The current config overlays defaults (`src/launch.rs:103-108`), so a disabled sample otherwise follows every install forever. This is not a recommendation to delete `Recipe.enabled` or all named recipes: explicit custom recipe disabling is a live routing feature (`src/routing.rs:88-96`), and enabled un-routed rows remain one-off lane choices (`src/launch.rs:181-190`).

Fresh install safe. Historical launches retain their frozen recipe data (`src/contracts.rs:95-140`) independently of current recipe catalog. A current configured route referencing a removed catalog ID must carry its own complete recipe row; that is a setup edit, not a compatibility reader.

## Historical readers that are NOT part of the deletion totals

The rule is preserve evidence, not preserve every old workflow. No automatic migration layer, format switch or fallback writer is proposed.

| Keep / isolate | Why it is not safe to delete from an upgrade | Fresh-only theoretical saving, **not budgeted** |
|---|---|---|
| `src/review.rs:201-228` old round reviewer exclusion | Otherwise old reviewer lanes can enter a new pile as ordinary members. Docs claiming round records are never read are inaccurate (`docs/operations.md:117`). | About 28 lines, but keep this read-only exclusion. |
| `src/review.rs:1674-1792` `classify_old_seals`, historical merge/install fields in `src/thread.rs:206-219` | Converts available old seal/ancestry/install evidence into honest displayed completion. Deleting it would risk the plan-count regression explicitly warned about in the brief. Not the same function as S1's marker repair. | Roughly 120 production lines plus callers, but keep history loading/derivation. |
| `src/contracts.rs:427-445`; `src/task.rs:78`; `src/plan.rs:1047-1098` historical `threads` and task-side `plan_step` | New CLI links tasks only (`src/cli.rs:390-436`), but old plans can depend on these links. Normalize them at read time into one binding view (S6), not silently discard them. | No net count until normalized reader is designed. |
| `src/prompt.rs:38-69,709-718` historical talk/request authority | Existing task and note provenance can refer to these IDs. | Keep; no new talk writer required. |
| `src/thread.rs:319-326` unmatched historical reports | Makes old reports readable without pretending they sealed completion. | 8 lines; keep. |
| `src/inbox.rs:38-56` removed-kind read filter | Old thread/round message projections must not reappear as actionable work. The write-time refusal at `:90-92` can disappear with typed current writers; don't remove the read filter and resurrect duplicates. | About 19 lines; keep. |
| Serde defaults / small aliases, e.g. `src/thread.rs:128`, `src/contracts.rs:112,315` | Missing old failure evidence truthfully becomes unknown; renamed stored counters still decode. These are cheap historical-data support, not an active old API. | Tiny; not worth a history break. |

Fresh public installs can never manufacture many of these old records, but publishing the same product is not permission to lose Rolf's historical evidence. Separate that small decoding concern from obsolete runtime repair loops.
