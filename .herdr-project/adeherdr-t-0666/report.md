# Simplify herdr-ade before publishing

**Reviewed:** `d69a2c8e8f6c5a40b56890f8831891d06db3eb6e`, on `oci`. Report only; no code changes or private project/config inspection.

**Bottom line:** delete the abandoned question workflow and installation-history repairs first. Consolidate readers and the two Rust execution stacks next. Keep the machinery that distinguishes a seal from a merge, a push, an install and a delivery receipt; those are different facts, not gratuitous states (`src/contracts.rs:248-359`; `src/review.rs:1321-1498`).

The baseline is **68,961 Rust lines, at least 28,210 test-only**. `threads.rs` is 8,347 lines; `ticker.rs` is 7,290, of which 4,024 are its final test module. Size measurements, commands and full surface inventory: [inventory](library/inventory.md). Splitting files is not deleting code.

## Must fix before publishing

| ID | What, why and concrete change | Evidence | Effort / estimated net removal |
|---|---|---|---|
| **M1** | **Delete live `ha ask`.** Coordinators ask in chat, but ADE still creates questions, enforces choices, publishes/retries notifications and walks ask dependencies before nudging work. Remove those commands, writers, publisher, widgets and tests together. Retain read-only old answers/provenance; don't retain an active old workflow. | `skill/COORDINATOR.md:63,69`; `src/ask.rs:357-650`; `src/ticker.rs:2892-2903,2955-3039,3256`; `src/cli.rs:271-295,478-548` | **M · 1,100–1,450 lines** |
| **M2** | **Stop Rundown deleting words and steps.** Its “technical” filter erases paths, filenames and ID-shaped words, then drops empty rows and counts only what survives. Render the actual label, wrapped/truncated; keep the TUI. An existing test demands “Fix so the tab in shows.” | `src/rundown/view.rs:29-45,85-99,129-187,615-644` | **S · 100–150** |
| **M3** | **Rewrite PI skill against current behavior.** Remove the historical model table and contradictory readiness/retry directions; deduplicate coordinator chat-choice instructions. Lane/reviewer skills mostly carry useful operational instructions, not excess. | `skill/PI.md:20-34,46-48,67-71` versus `src/pi/doctor.rs:1-7,848-957`, `src/cli.rs:122-125`; `skill/COORDINATOR.md:63,69` | **S · 40–65** |

Details, safe-deletion boundaries and historical consequences: [deletion ledger](library/deletion-ledger.md).

## Should

| ID | What, why and concrete change | Evidence | Effort / estimated net removal |
|---|---|---|---|
| **S1** | **Remove one-time repairs from the ticker.** Delete branch sweep v2/old round cleanup, old change reclassification, saved-id startup-counter repair, retry of one obsolete gate-error sentence, and the unused initial binary-swap script/test pair. Keep ordinary branch pruning, leases and current recovery. | `src/branches.rs:78-94,473-916,1607-1829`; `src/review.rs:530-585,1148-1157`; `src/ticker.rs:730-735,2741-2785`; `scripts/migration/swap-binary.sh:22`; exact counts/call sites in [ledger](library/deletion-ledger.md#s1--remove-installation-history-repairs-from-normal-operation) | **M · 1,200–1,330** |
| **S2** | **Delete actual unused pi launch/resume code.** Its callers are tests, while real starts/resumes use `herdr.rs`/`threads.rs`. Also delete the old canned-answer placeholder test, write-only `compact_reason`, unused courier telemetry and production parsing maintained only for old test fixtures. Fix those fixtures instead. | `src/pi/resume.rs:1-127`; `src/pi/launch.rs:163-208,242-251`; actual path `src/herdr.rs:532-589`, `src/threads.rs:3451-3456,3521`; `src/launch.rs:664-679`; `src/steps.rs:812,936`; `src/worktrees.rs:132-155`; `src/scenarios.rs:2868-2958` | **S · 450–520** |
| **S3** | **Delete retired refusals and the disabled shipped recipe.** Remove `[roles]`/old-key diagnostics, Cursor-in-pi archaeology and the disabled Astra sample. No compatibility flags. Do not label every unused enabled recipe dead or remove custom recipe disabling. | `src/launch.rs:82-101`; `src/project.rs:190-227`; `src/doctor.rs:1118-1134`; `src/pi/doctor.rs:514-526,1167-1201`; `assets/default-recipes.toml:36-43`; live disabling `src/routing.rs:88-96` | **S · 190–260** |
| **S4** | **Use a normal shared library.** One `Runner`, environment/root resolver, typed recipe schema, Git execution boundary and artifact store. Delete the pi command-runner clone and field-by-field bridge. This also avoids running shared pi/config tests twice. | `Cargo.toml:19-21`; `src/pi/sh.rs:1-7,22-269`; `src/runner.rs:11-152,195-364`; `src/pi/ade.rs:19-48`; [exact duplicate map](library/architecture.md#s4--one-shared-rust-core-one-execution-seam) | **M · 650–950** |
| **S5** | **Replace shell-scraped box protocols with one Rust observation boundary.** Keep batched SSH/file transfer and hash/receipt checks. Courier, doctor and ticker should consume typed machine facts; derive the taken-event index from import records instead of storing it twice. | `src/steps.rs:842-1030,1080-1088,1193-1199`; `src/doctor.rs:1880-2253`; `src/events.rs:69-99,256-269`; [proposal](library/architecture.md#s5--one-typed-box-observation-boundary-not-shell-protocols-in-three-modules) | **L · 350–650** |
| **S6** | **Build one evidence snapshot/project view.** The current snapshot holds only events, then task/plan/context readers reopen lanes/reviews/tasks. Reuse indexed records, normalize old bindings on read and derive plan state consistently. Stop persisting copied goal/state/canned result sentences as additional authority; remove the seven-kind sentence taxonomy. | `src/task.rs:524-605`; `src/thread.rs:354-454`; `src/events.rs:442-622`; `src/plan.rs:619-640,735-809,893-1098`; `src/contracts.rs:389-405,431-459`; [shape and safeguards](library/architecture.md#s6--one-evidence-snapshot-and-one-derived-project-view) | **M · 400–650** |
| **S7** | **One pending-notification model.** Event journals, review notices, start notices and wake strings implement overlapping delivery paths. Use the existing delivery journal keyed to source facts; preserve submitted-versus-seen receipts and binding identity. | `src/steps.rs:21-223,273-351`; `src/contracts.rs:343-359`; `src/thread.rs:113-115`; `src/review.rs:91-94`; [state map](library/architecture.md#s7--one-pending-notification-model) | **M · 150–250** |
| **S8** | **Split oversized files along existing seams.** Extract placement, recovery, messaging, report preservation, cleanup and views from threads; process/scheduler/observation/launch/nudge from ticker; checks/readiness/machines/timings from doctor; courier/delivery from steps; command families from CLI. Move fixture scaffolding out of scenarios and keep only cross-component cases there. | Function boundaries and proposed module tree in [split map](library/architecture.md#s8--split-at-the-seams-already-present-move-tests-dont-count-moving-as-deletion), anchored to `src/threads.rs:162-5568`, `src/ticker.rs:23-3265`, `src/doctor.rs:23-2253`, `src/steps.rs:17-1565`, `src/scenarios.rs:19-3289` | **M · 0 from moving; optional 100–200 fixture lines**, excluded from total |
| **S9** | **Reduce first-run help, not capabilities.** Show the project-opening path first; demote lane protocol and internal `ticker`/`harness`/workspace-adoption commands. Keep recovery discoverable in long help. No command renaming spree. | Measured help: 27 implemented visible commands + generated help; five other commands already hidden (`src/cli.rs:49-305`). [Full CLI/config/env disposition](library/inventory.md#cli-27-visible-implemented-commands-plus-help) | **S · 0** |

### Historical support: keep evidence, delete old workflows

All S1 repairs are unnecessary for a **fresh** install. Whether their markers have already been written on Rolf's existing install is **unknown**; finish outstanding maintenance before removing the writers. Do not claim the repository proves a migration ran.

Do **not** indiscriminately delete old-format readers. Historical request authority, answered asks, thread-bound plans, unmatched reports and installed tasks still matter. The old round reader at `src/review.rs:201-228` prevents historical reviewers entering a new pile; it is not a surviving round state machine. `classify_old_seals` (`src/review.rs:1674-1792`) is distinct from S1's one-time marker repair. [Reader-by-reader disposition](library/deletion-ledger.md#historical-readers-that-are-not-part-of-the-deletion-totals).

### Tests: retire obsolete assertions, retain defect coverage

Delete sweep/old-error/unused-launch tests with their removed features. Move pure argument and state policy checks out of whole-world scenarios. Do not delete race, crash, partial-copy, sleep/backoff, cross-project isolation or large-history cache tests just because they are long (`src/scenarios.rs:297-346,641-687,1740-2170`; `src/branches.rs:1846-2009`; `src/review/tests.rs:1673-1764`; `src/ticker.rs:6299-6413`). Most test-only code is correctly compiled out; the production old-test-fixture parser is the real exception (S2).

## Nice to have

- **N1 — One note-selection result for briefs and budget warnings.** Keep provenance, task scoping and explicit replacement. Reuse included/omitted facts and size calculation instead of separate selection/budget passes. No summarizer, new cap or per-model policy system. **S · 40–80 lines, low risk.** `src/thread.rs:750-776,820-850,918-935`; `src/note.rs:340-375`.
- **N2 — Encode landing progress as one ordered checkpoint.** Replace five booleans plus phase with `LandingStep`; retain durable effect boundaries and publication/install evidence. **M · 0–80 lines, high risk for a small win.** `src/review.rs:79-87,1321-1498`. Do this last, not as a prerequisite to publishing.

## Validation and scope limits

All four pinned gates passed on unchanged code: `cargo fmt --check`, `cargo test` (**740 passed, 2 ignored**), `cargo clippy --all-targets -- -D warnings`, `git diff --check`. [Full output](library/gates.txt); [measurement details](library/inventory.md#gates-run-on-the-unchanged-code). No extra smoke tests or live probes were run.

**Outside my scope:** publication instructions still advertise generic Herdr 0.9.1+ and private-repository access (`README.md:13,53,74,112`), while this brief requires Rolf's fork. The three-provider pi policy also remains narrowly hard-coded (`src/pi/launch.rs:18,264-324`; `src/bin/herdr-pi.rs:117-124`). The publication/portability lanes should resolve those promises; I have not audited upstream compatibility or credentials. Minor docs drift: operations says old rounds are never read (`docs/operations.md:117`), contradicted by the exclusion reader above.

## Simplification plan — biggest safe wins first

Estimates include associated tests/docs where specified; overlap is excluded. They are planning ranges, not removal quotas.

| Order | Change | Estimated net lines removed | Risk |
|---|---|---:|---|
| **1** | Delete live asks, preserve historical answers (M1). | 1,100–1,450 | Medium: existing unresolved asks need deliberate cutover. |
| **2** | Remove one-time repair writers and retired swap scripts (S1). | 1,200–1,330 | Low for fresh installs; existing repair completion must be known. |
| **3** | Delete unused pi launch/resume, fixture-only fallback, write-only tails and retired policy/catalog baggage (S2–S3). | 640–780 | Low; verify named call sites, preserve actual launch path. |
| **4** | Stop Rundown censorship and correct skills (M2–M3). | 140–215 | Low; leave rendering and lane safety intact. |
| **5** | Introduce shared library/runner/config/Git/artifacts (S4); split touched modules (S8). | 650–950; splitting adds no savings | Medium: preserve timeout, byte and lock semantics. |
| **6** | One evidence/project view; derive plan projections; share memory selection (S6, N1). | 440–730 | Medium: historical plan counts and failed-check holds must remain correct. |
| **7** | Typed box observation/import index and one delivery path (S5, S7). | 500–900 | High: preserve unavailable-vs-gone and crash/delivery boundaries. |
| **8** | Simplify help (S9); optionally ordered landing checkpoint (N2). | 0–80 | Help low; checkpoint high. |

**Expected net scope: roughly 4,700–6,400 lines**, plus substantial file separation—not a rewrite of the 69k-line repository. Publish after the contradictory product behavior is removed; don't hold publication hostage to the optional transport/checkpoint refactors.
