# Concrete simplifications, without throwing away reliability

Estimates are **net physical lines removed after replacements**, including tests only where noted. No time or line estimate below is a required target. References are to `d69a2c8`.

## S4 — One shared Rust core, one execution seam

**Should · M · net 650–950 lines · medium risk.**

### Duplicate command execution is the clearest structural waste

`src/pi/sh.rs:1-7` explicitly says its second runner exists because the package has no library target. Compare:

- Main `Cmd` / `Output` / `Runner` / child-process loop: `src/runner.rs:11-152,195-364`.
- Pi versions: `src/pi/sh.rs:22-269`.
- A field-by-field bridge between them: `src/pi/ade.rs:19-48`.
- Repeated fake runner: `src/runner.rs:369-504`; `src/pi/sh.rs:306-392` (S2 separately removes the obsolete canned-verb scaffolding in the main fake).
- Pi is path-included into two binaries with blanket dead-code suppression: `src/main.rs:54-57`; `src/bin/herdr-pi.rs:8-16`; Cargo explicitly avoids a library (`Cargo.toml:19-21`).

**One shape:** add a normal shared library target exposing the CLI entry and narrow pi entry; binary files remain argument/exit adapters. `runner::Cmd` and `Runner` become the only execution types. Pi calls the same runner with `own_group` enabled; preserve its process-group timeout behavior, captured partial output and stdin deadlines. Move pi-specific login-shell helpers, not a second process runner, under pi.

A library also means the same 50 pi/config tests need not execute once in each binary. In the baseline `cargo test` log, `rg '^test (pi::|config::)'` counted 100 lines. This is duplicate execution, not 50 additional source tests to claim as LOC removed ([gates.txt](gates.txt)). Remove only tests of the dead alternative launch path under S2; run the remaining shared tests once.

### Other duplicates to fold into that library

| Current duplicates | One implementation | Budget within S4 |
|---|---|---:|
| `paths::Env` and root resolution (`src/paths.rs:15-105`) vs `pi::Env` / resolver (`src/pi/mod.rs:39-91,176-205`) | Shared `Env`/root; pi adds `.join("pi")`. Preserve root precedence. | 70–100 |
| Core `Recipe` / `Routing` (`src/contracts.rs:8-33`, `src/routing.rs:10-26,44-96`) vs second recipe/routing/adapter reader (`src/pi/doctor.rs:24-142`) | A typed shared config; pi asks it for the set of selected provider/model pairs. | 80–120 |
| Raw git wrappers in `src/branches.rs:16-26`, `src/threads.rs:99-115`, `src/repo.rs:24-45`, `src/git.rs:25-46` | `git::Git` with raw output, text output and typed boolean helpers; caller supplies timeout. Preserve NUL status bytes, do not blindly trim. | 40–70 |
| Brief artifact writer/reader (`src/thread.rs:647-671`), event artifact writer/hash (`src/events.rs:213-250,298-300`), op artifact writer (`src/ops.rs:764` onward) | One content-addressed artifact store used by briefs, reports and ops; retain hash verification and durability semantics. | 40–80 |
| Repeated runner/bridge/fakes listed above | One shared process seam. | Remainder, approximately 400–580 |

**Correction to the starting hypothesis:** `repo::Git::is_ancestor` and `branch_head` already delegate to `git.rs` (`src/repo.rs:49-67`). Do not describe these two wrappers as independently implemented ancestry algorithms. One-time branch-sweep direct ancestry calls disappear under S1. The remaining wrapper duplication is real, but much smaller.

Do not merge locks merely because both use a repository identity. The review operation lock (`src/review.rs:366-412`) spans a durable workflow; the git mutation lock (`src/git.rs:138-153`) guards short filesystem/ref operations. Their lock durations and call nesting differ.

## S5 — One typed box observation boundary, not shell protocols in three modules

**Should · L · net 350–650 lines · high risk.**

Current transport is already batched: one helper SSH trip plus one batched file fetch per project, with multiplexing (`src/steps.rs:1050-1061,1146`; `src/remote.rs:596-659`). Do not replace it with one SSH command per lane. The excess is the boundary encoding and multiple observers:

1. Courier's embedded shell scrapes TOML/JSON using `sed`, emits TSV, and includes both events and live observations (`src/steps.rs:842-918`). Rust manually parses that TSV (`:930-1006`) and then parses nested Herdr JSON (`:1015-1030`).
2. Doctor builds a different large TSV script with host facts, provider probes and Herdr inventories, then reparses facts (`src/doctor.rs:1880-2253`).
3. The ticker reconciles those observations through machine-phase bookkeeping (`src/ticker.rs:864-997`), the common `thread_pass` (`:1465-2186`) and box-specific `remote_attention` (`src/steps.rs:1255-1472`).
4. Machine lookup can read Herdr profiles or synthesize another profile from ADE config (`src/remote.rs:178-224`); ADE also duplicates target/session/id/label fields (`:59-75`).

**One shape:** a Rust box-side observation operation in the existing ADE executable, returning one typed snapshot through the existing SSH connection. Use its own TOML/JSON decoders; let transport carry bytes and batch files, not parse record semantics in shell. Core type:

```text
MachineObservation {
    identity, observed_at,
    process_snapshot: Known(agents, panes, boot) | Unavailable(error),
    bootstrap_receipts,
    completions: [event id + source hash + report hash],
    progress
}
```

This is a replacement private boundary, not a new public mode or Python service. Doctor can request current facts through the same Rust reader and add readiness checks; the ticker feeds a common observation reducer. Herdr's saved machine identity should be authoritative; ADE adds placement paths and allowed kinds. Check whether config-only profiles have an actual supported consumer before removing that live capability.

**Derived instead of stored:** `RemoteState.taken` duplicates event/hash tuples already persisted in `ImportSource` (`src/events.rs:69-99,164-177,256-269`; `src/steps.rs:1080-1088,1193-1199`). Build the taken set from the durable import records in a cached index; keep last successful observation time and boot/missing-poll evidence separately. Do not substitute a single numerical high-water mark: event IDs from different lanes do not supply a global sequence.

Keep source hash verification, create-only import, attempt/pane matching and receipt verification (`src/steps.rs:1151-1192`; `src/events.rs:120-177`). These are not three equivalent “done” flags: they prove different stages across a crash/SSH boundary. A timeout must remain unavailable/unknown, not gone; sealed events can still import when the box server cannot supply live lists (`src/steps.rs:815-819`; `src/ticker.rs:2734-2738`).

Budget is uncertain until implemented; it counts replacement protocol/schema and overlapping observation setup, not deletion of remote support or all 1,115 lines of `remote.rs`. Don't put this risky rewrite ahead of the low-risk cuts.

## S6 — One evidence snapshot and one derived project view

**Should · M · net 400–650 lines · medium risk.**

The existing direction is right: tasks already have no writable status (`src/task.rs:1-5,59-83`). The current `EvidenceSnapshot` however contains **only events** (`src/task.rs:524-542`). Each task view reopens its lane and reviews (`:576-605`); `terminal_with_evidence` reloads the lane (`:137-149`). Other reads repeat scans:

- Threads: checked/raw/cached/live/snapshot paths (`src/thread.rs:354-454`).
- Events: list/checked/per-thread/unresolved with their own indexes (`src/events.rs:442-622`).
- Reviews: cached list plus separate noncached directory scan (`src/review.rs:150-194`).
- Tasks have another directory scanner (`src/task.rs:272-306`).
- Plan output re-enumerates tasks for each step (`src/plan.rs:619-640`); derivation and failed-check explanations traverse much the same task/step/lane evidence (`:791-809,893-967,999-1098`).
- Context builds another set of projections after loading evidence (`src/coordinator.rs:760-823`); page renderer builds its own (`src/project.rs:819-1056`).

**One shape:** extend the existing snapshot, not a database migration:

```text
ProjectSnapshot { tasks, lanes, reviews, events, read_errors, indexes }
    -> task_views()
    -> plan_view(intent)
    -> project_view()
        -> PROJECT.md / context / overview / Rundown JSON
```

Use `record_cache::Records` underneath (`src/record_cache.rs:1-142`), including for short CLI invocations. Expose checked results at decision boundaries; display can carry rows plus explicit read errors. Do not silently replace `checked` with an error-dropping `list` just to save wrappers.

**Derive, don't repeatedly persist:** plan `state`, copied `goal`, and canned `what_you_get` are projections (`src/contracts.rs:431-459`). `plan show` already recomputes state (`src/plan.rs:555-566`), while `sync`/`refresh` also rewrite it and increment revision (`:735-775`); ticker nudging trusts persisted state (`src/ticker.rs:2944-2952`). Keep plan intent (step IDs/text/task links/prerequisites/order) and its edit revision. Produce state and goal from the shared snapshot for all consumers, including nudging. Cached projection is disposable, not additional completion authority. Avoid rereading every old record per tick.

Normalize historical `PlanStep.threads` and task-side `plan_step` into one binding view **on read**, while new writes keep only task links (`src/contracts.rs:435-445`; `src/task.rs:78`; `src/plan.rs:1053-1062`). Preserve failed-critic and missing-binding semantics; do not erase old cards.

Also delete the seven fixed result sentences and the equality check that enforces them (`src/contracts.rs:389-405`; `src/plan.rs:99-124`; `src/cli.rs:345-348`). An ordinary `does` sentence and the goal are enough; public projects do not need a taxonomy of “screen/command/background/document/picture/number/finding.” Historical fields can still decode. This contributes roughly 80–130 of S6's estimate, including the tests of sentence generation (`src/plan.rs:1122-1152`). It is separate from M2's actual loss of rendered words.

**Keep the useful cache:** the existing test at `src/ticker.rs:6299-6413` models 3,000 settled + four live lanes and asserts zero warm idle record opens / four changed lane opens. That exercises a real regression and is not smoke-test bloat. Preserve its behavior when unifying readers. No new invented throughput budget is proposed.

## S7 — One pending-notification model

**Should · M · net 150–250 lines · medium risk.**

There are distinct facts, but multiple delivery implementations:

- Sealed event delivery journal (`src/contracts.rs:343-359`; `src/events.rs:624-666`).
- Thread `start_notices` (`src/thread.rs:113-115`) and review `notices` (`src/review.rs:91-94`) carry a separate `{line, submitted}` representation.
- `deliver_transition_notices` handles review and lane notice loops separately (`src/steps.rs:168-223`); `deliver_events` has another submission loop (`:273-351`).
- Wake cursor stores copied strings and another set of submission/receipt state (`src/steps.rs:21-161`).
- Context cursor stores a previous **view**, which is a separate legitimate concern (`src/coordinator.rs:740-757`).

**Proposal:** generalize existing `deliveries/` to one source-keyed pending/submitted/seen delivery path. A source is a sealed event or a review/start transition; its underlying fact remains in its existing record. Wake cursor should retain outstanding source IDs / binding revision rather than copies of arbitrary prose. Render from the facts. No second inbox projection of completion, and no new journal category on top of the existing ones.

Do not derive away receipt state: a record existing does not prove a prompt was submitted or context was read. Preserve attempt/binding scoping, ordering, the “transport succeeded before crash” at-least-once boundary (`src/events.rs:621-623`), and no retyping already-submitted notices. M1 already removes the ask-specific publication journal; S7's budget does not count it again.

### State/record map: which things actually are one concept?

| Stored family | Authority / recommendation | Evidence |
|---|---|---|
| tasks + requests + notes/retirements | Intent and provenance. Keep; historical readers are legitimate. | `src/task.rs:59-83`; `src/prompt.rs:693-718`; `src/note.rs:35-55,316-336` |
| thread + frozen Launch + LaneCard + bootstrap | Execution identity, placement and receipt across machines. Group related fields structurally; don't delete the frozen identity snapshot because paths repeat. | `src/thread.rs:75-230`; `src/contracts.rs:59-140`; `src/lane.rs:466-530` |
| ops -> events -> artifacts | In-progress seal intent, immutable completed fact, content. Keep all three roles. | `src/contracts.rs:248-338`; `src/ops.rs:1-28`; `src/events.rs:304-351` |
| receipts + imports + remote taken | Keep box proof/import provenance; derive taken from import ledger (S5). | `src/events.rs:69-177,256-269,354-414` |
| deliveries + start/review notices + wake lines | One delivery state machine referencing source facts (S7). | `src/steps.rs:21-223,273-351` |
| plan states + page body + context maps | Views/cache, not completion authority (S6). Context's last-seen view remains a necessary diff cursor. | `src/plan.rs:735-809`; `src/project.rs:1076-1091`; `src/coordinator.rs:740-757` |
| review phase + landing checkpoint | Durable external effects, keep. Can encode more simply (N2). | `src/review.rs:19-31,79-87,1321-1498` |
| asks + publications + ask/not-understood counters | Obsolete live workflow; remove writers (M1), keep old evidence decoding. | `src/ask.rs:30-100,376-378,506-509,629-650` |
| `.branch-sweep-v2`, `.change-reclass-v1` and old-start repair bit | One-time installation history, remove writers (S1). | `src/branches.rs:762-916`; `src/review.rs:530-585`; `src/thread.rs:101-104` |

The lifecycle is several **axes**, not one overlarge enum to flatten: project availability (`src/project.rs:242-260`), execution status (`src/thread.rs:25-33`), seal kind (`src/contracts.rs:207-215`), task evidence state (`src/task.rs:108-128`), review progress (`src/review.rs:19-31`), delivery receipt (`src/contracts.rs:343-351`) and display group (`src/thread.rs:962-971`). “Finished” must not become “published and installed,” nor should an unavailable observation become a failed process. The excess is duplicated projection/notification and repair fields, not those distinctions.

## S8 — Split at the seams already present; move tests, don't count moving as deletion

**Should · M · net 0 lines for file moves; optionally 100–200 net fixture/test lines after factoring.**

Suggested structure (ordinary modules, not a plugin framework or trait for every helper):

```text
src/lib.rs
src/bin/{herdr-ade,herdr-pi,herdr-rundown}.rs   # thin entry points
src/threads/{mod,start,placement,recovery,messages,artifacts,cleanup,views}.rs
src/ticker/{mod,process,machines,observe,launch,nudge}.rs
src/doctor/{mod,checks,readiness,machines,timings}.rs
src/delivery.rs
src/courier.rs
src/cli/{mod,project,tasks,lanes,reviews,plan,internal}.rs
src/test_support/{world,git,records}.rs        # cfg(test)
```

Concrete extraction seams:

| Existing file | Extract these responsibilities | Keep facade after |
|---|---|---|
| `threads.rs` (8,347 total; 5,569 before tail tests) | Start/placement `:162-1530` (local `:1185-1408`, box `:735-1132`); recovery `:1531-2280,3152-3361`; messages/attestation `:2281-2817`; artifact links/copy `:3732-4723`; cleanup `:2818-3150,3362-3730,4725-5253`; views `:5255-5568`. | Re-export command entry points and small orchestration. Keep artifact-preservation and cleanup safety; their length is not evidence they can be deleted. |
| `ticker.rs` (7,290; 3,266) | Process/lock/supervision `:23-664`; pass scheduler `:671-805`; machines `:807-1015,2723-2836`; observation/progress `:1185-2186`; launch `:2188-2548`; local snapshot `:2550-2721`; nudge `:2879-3114`; pass composition `:3123-3265`. | Own cadence and call order only. Remove migration hooks under S1 before extraction. |
| `doctor.rs` (3,995; 2,254) | Timings `:23-174`; readiness/disk `:193-602`; check assembly `:604-1342`; worktree/workspace inventories `:1378-1813`; machine checks `:1815-2253`. | Build one typed list of check results, render once. Per-start readiness is an independent service, not a doctor-text parser. |
| `steps.rs` (2,882; 1,566) | Wake/delivery `:17-622`; outage/scheduler memory `:624-762`; courier `:764-1240`; remote attention `:1242-1472`; message notifications `:1474-1565`. | Rename by responsibility; “steps” currently mixes transport, events and observation. |
| `cli.rs` (2,371; 2,051) | Command families at `:49-964`; remove Ask (M1); dispatch into project/task/lane/review/plan/internal handlers (`:1271-2050`). | Parse args, construct context, dispatch, finish typed result. |
| `scenarios.rs` (3,289, all tests) | Move `World`/JSON fixtures `:19-232` into test support; cleanup tests `:451-1467`, observation `:1576-2217`, routing/recovery `:2309-2655`, coordinator `:2656-2866`, installer `:2962-3289` alongside their domain. | Only genuinely cross-component scenarios remain; no production split required. |

Tests that can get smaller after extraction:

- File-free argument construction tests in `src/pi/scenarios.rs:33-81` currently set up pinned install/trust/wrapper files to assert a string. Delete with the unused implementation (S2), not move to another scenario suite.
- The old `ade_new_verb_scenarios_have_canned_herdr_replies` test only exercises answers it installs in its own fake (`src/scenarios.rs:2868-2958`; `src/runner.rs:419-463`). Delete that placeholder and helper under S2; it is not ADE behavior coverage.
- Pure prerequisite graph/result-state policy is currently mixed with real-repo fixture work (`src/plan.rs:1053-1098,1450-1494,1910-1940`). After S6, run existing cases against in-memory snapshot values; retain integration cases for missing/corrupt evidence and writing changed records.
- `src/scenarios.rs:2309-2341` builds a project/world to validate recipe choice. `launch::validate_explicit_recipe` can own the small input/output table; do not remove real start/route integration coverage (`:2365-2655`).
- Keep two-project isolation, sleeping-machine/backoff, partial-copy retention, lease races and landing crash-boundary scenarios (`src/scenarios.rs:297-346,641-687,1740-2170`; `src/branches.rs:1846-2009`; `src/review/tests.rs:1673-1764`). They name current defects, not abandoned policy.

The mere presence of `#[cfg(test)]` in production source is not runtime bloat. Test-only read counters (`src/thread.rs:456-468`; `src/events.rs:426-439`) and fake runner modules disappear from a release build. The NUL-status fallback (S2) is the actual exception where old fixtures dictated extra **production** code.

## N1 — Share note selection; don't build a new memory manager

**Nice to have · S · net 40–80 lines · low risk.**

Notes already have one append-only write path, provenance, task scoping and replacement semantics (`src/note.rs:35-64,316-375`). Historical task notes are readers, not another supported writer (`src/note.rs:121-157`). Keep those distinctions.

Memory warning selection enumerates/sorts scopes (`src/thread.rs:750-776`); brief selection independently chooses task-scoped notes (`:918-935`); `compose_brief` separately budgets rendered facts (`:820-850`). Make one pure selection result `{instructions, included_facts, omitted_ids, measured_size}` and use it for both rendering and warning. Keep the existing cap unless there is evidence it is wrong; don't add token estimators, per-model budgets or a summarizer service. Instructions are intentionally separate from the capped memory facts (`:802-818`); a universal cap would silently omit rules.

## N2 — Replace landing booleans with an ordered checkpoint, not with wishful derivation

**Nice to have · M · net 0–80 lines · high risk for little size gain.**

Review has `phase` plus `fast_forward`, `push`, `install`, `close`, `prune` (`src/review.rs:19-31,60-95`). `land_with_install` executes them in that order, persisting after effects (`:1321-1498`). Encode the ordered part as one `LandingStep` carried by `Phase::Landing` instead of five independent booleans. The record still needs candidate/verdict/seal pins and recovery attention.

Do not replace persisted install/publication evidence with “git says merged.” Current landing verifies the actual push URLs (`src/review.rs:1260-1304,1399-1434`), does not call install before publication, and allows thread cleanup debt to outlive landing (`:1450-1483`). These prevent wrong pushes, premature completion and stuck reviews. Keep the safeguards and their tests. This is last in the plan precisely because splitting the module is safer and yields most readability benefit without altering recovery semantics.
