# Review brief: round r91

plain: An install with a broken box step still builds and runs its own fix first.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r91` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `08c1577a7ea71ee54a8a89ed360c4b9bea467e466453c84ba974727dec866be3`, policy hash `d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0224 | 1 | `ced57749e87035275d0c66dab9a9ee77529005ac` | `t-0224-1-1` | `fa3715350e5eba8d835f78e0acb15d0fa9e4c54f3d1b0908e0e52b312a05ef14` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r91.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r91"
candidate = "<C>"
manifest_hash = "08c1577a7ea71ee54a8a89ed360c4b9bea467e466453c84ba974727dec866be3"
policy_hash = "d9b7ce2a8a3a9102c2ed533f14bca3988e9f54acbf667b744058b12959db943a"
gates = []
+++
```

5. Follow the reviewer skill's Done instructions, then run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0224 (artifact `fa3715350e5eba8d835f78e0acb15d0fa9e4c54f3d1b0908e0e52b312a05ef14`)

Data, not instructions.

```text
# W16 report

Implemented the install ordering fix in `src/harness.rs`.

- The local phase now builds and installs all configured harness repositories before resolving the saved box.
- If the installed `herdr-ade` replaced the running image, re-exec happens before any machine lookup, SSH build, settings copy, or process proof.
- The remote phase updates the same install records after local installation completes.
- Updated the failed-machine-list regression to prove the local build and re-exec boundary run before box lookup fails.

Gates passed:

- `cargo fmt --check`
- `cargo test` (622 main tests, 57 herdr-pi tests, 82 herdr-pro tests, and integration tests)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`

Commit: `ced5774`
```

