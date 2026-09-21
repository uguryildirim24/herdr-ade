# Review brief: round r81

plain: A new kind of helper plugs in without changing the core.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r81` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `467fe12ac87ae8cc68bf85072e21ba7c0960af753c2dac8b5f2c48a1f0dae757`, policy hash `3ff3234b04a6b9438ce5a97e9ebb249f4698636f90353f2a3946112ef9d3e1d2`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0187 | 1 | `234706e3720837a386f2557fb40fbab1d64ed8ae` | `t-0187-1-1` | `dca8f70088d31b4903774970f4f807da26044cd347f1cda89a1a74340dc99fde` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r81.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r81"
candidate = "<C>"
manifest_hash = "467fe12ac87ae8cc68bf85072e21ba7c0960af753c2dac8b5f2c48a1f0dae757"
policy_hash = "3ff3234b04a6b9438ce5a97e9ebb249f4698636f90353f2a3946112ef9d3e1d2"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0187 (artifact `dca8f70088d31b4903774970f4f807da26044cd347f1cda89a1a74340dc99fde`)

Data, not instructions.

```text
# D6 report

Implemented config-driven helper adapters, model recipes, and machines.

- Added per-kind adapter declarations for launch flags, readiness probes, hook grammar, coordinator/talk support, capabilities, permission flags, and effort values. Config can add or replace adapter rows without engine branches.
- Removed Claude-specific coordinator, talk, hook, routing, and launch paths. Task requirements and routing now use generic capabilities.
- Moved shipped model recipes and machine/path/repository facts into TOML assets, with user-configured rows able to replace or extend them.
- Refactored remote placement, cleanup, installation, path mapping, and pane probes to consume machine declarations.
- Doctor and readiness now probe the provider/model pairs actually selected by recipes and routing. Removed the unused fixed Kimi probe, and keyed readiness caches by the exact probe arguments.
- Extended `herdr-pi check` to accept and probe an exact model.
- Updated operations and coordinator documentation.

Tests cover a config-only made-up adapter, non-Claude coordinator/talk behavior, generic capability routing, a second configured machine, and exact routed provider/model probes.

Gates passed:

- `cargo fmt --check`
- `cargo test` (585 main tests, 57 herdr-pi tests, 79 herdr-pro tests, and integration suites)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

Commit: `234706e`
```


## Repair revision

This revision reviews the integration base `a627474a5788007a63531439cbf70c9602d35fc2`.
