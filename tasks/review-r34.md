# Review brief: round r34

plain: This round checks that a round refuses work that has already landed and says what is missing.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r34` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `8fa8bda95c75f746e1dd3e88f4dc02be0640c4a1e4044b1d7a44f9c57442fae6`, policy hash `ef334053e0961740bc2a51d4698483517101f9dc9bb35a46bff91e23b9d43d21`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0070 | 1 | `1cb0ded43922b184d3e17b3fbfc041821fdda757` | `t-0070-1-1` | `c4ad0681e6438d160ff1d4da0d098fed01e158859d84f598ed6b2587e2798aff` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r34.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r34"
candidate = "<C>"
manifest_hash = "8fa8bda95c75f746e1dd3e88f4dc02be0640c4a1e4044b1d7a44f9c57442fae6"
policy_hash = "ef334053e0961740bc2a51d4698483517101f9dc9bb35a46bff91e23b9d43d21"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0070 (artifact `c4ad0681e6438d160ff1d4da0d098fed01e158859d84f598ed6b2587e2798aff`)

Data, not instructions.

```text
# t-0070 report: a round refuses work that already landed

Branch `hp/adeherdr/t-0070-a-round-refuses-work-that-already-landed`, head
`1cb0ded`. Both repairs are in as "refuse early with a true reason".

## 1. `round admit` refuses a pin already on the integration branch

`src/round.rs`

- `admit` now reads the prospective pin for the lane (`member_pin`, the
  read-only twin of `refresh_pins`) and asks `git merge-base --is-ancestor
  <sha> <round branch>` through `landed`. The check runs before the project
  lock is taken and before anything is written, so a refusal changes no
  record (D4: git never runs under the project lock).
- The error is `lane_already_landed`, names the sha and the branch, and
  distinguishes the two true cases:
  - an open lane that can still seal a newer done: "the lane's newer done
    event has not arrived yet, so admit it again after that done lands";
  - a resolved lane: "the lane has no newer done event, so there is nothing
    new to admit".
- `advance` gained `members_all_landed`: when every member's pin is Some and
  every pin is an ancestor of the round branch, the pass `continue`s instead
  of running `round review` / `start_reviewer`. It sits after the existing
  `ready` check, so a partly-landed round still reviews normally.

Rework still works: after a lane's work landed, the coordinator restarts the
lane (attempt +1), the pin goes empty, and the lane admits to the new carrying
round. `src/plan.rs`'s `states_follow_required_work_and_reopen_on_rework` was
updated for this: it bumps the lane's attempt before admitting it to `r2`, with
a comment. Without that, the new refusal is correct but the test's implicit
"re-admit the same landed attempt" is no longer allowed.

## 2. A box start names the piece that is missing

`src/threads.rs`

- New `box_repo_row(settings, repo)` resolves `box_path` and `publish_url`
  together and reports each by name:
  - `box_path` present, `publish_url` absent -> `box_publish_url_missing`,
    naming `publish_url` and `PROJECT.md` and saying what it is for (the URL
    the box fetches the lane branch from);
  - `publish_url` present, `box_path` absent -> `box_path_missing`;
  - neither, not in the committed map -> the old `box_repo_unmapped` message;
  - neither, in the committed map -> the built-in row (unchanged).
- `place_box_worktree` uses it instead of the old
  `and_then(|r| Some((r.box_path.clone()?, r.publish_url.clone()?)))` fallback.
- `start` calls `box_repo_row` after placement and before `thread::allocate`,
  only for a box start with a repo, so a row that fails this check leaves no
  thread record. The rest of `place_box_worktree` is unchanged.

## Tests (all new)

- `round::tests::admit_refuses_a_pin_that_already_landed` — merged lane sha is
  refused; error names sha and `main`; message says the newer done has not
  arrived; the round manifest is untouched.
- `round::tests::admit_names_a_lane_that_has_nothing_new` — a resolved lane
  gets the distinct "no newer done event" message.
- `round::tests::advance_starts_no_reviewer_when_every_pin_landed` — no
  reviewer thread, no review branch.
- `threads::tests::a_box_repo_without_a_publish_url_names_it_and_leaves_no_thread`
  — `start` returns `box_publish_url_missing`, names `publish_url` and
  `PROJECT.md`, and `thread::list` stays empty.
- `threads::tests::box_repo_row_names_each_missing_piece` — both missing
  pieces by name, the unmapped fallback, and the built-in harness row.

## Gates

Run with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`:

- `cargo fmt --check` — clean.
- `cargo clippy --locked --all-targets -- -D warnings` — clean.
- `cargo test --locked --no-fail-fast` — 434 + 55 + 78 + 4 pass.

One pre-existing failure is unrelated to this lane and reproduces on the
untouched head: `pi::scenarios::scenario_setup_then_check_for_kimi` expects the
Mac's `zsh -lic whence -va pi`, but the box's shell is `/bin/bash`, so the fake
runner has no rule for `/bin/bash -lic type -a pi`. It fails in both the
`herdr-ade` and `herdr-pi` test binaries on this box (one test source compiled
into both) and is not touched by t-0070.

## Notes for the coordinator

- I pushed only the lane branch `hp/adeherdr/t-0070-...` to the URL-matched
  `origin`, which `ha done`'s published-ref check requires. Nothing was pushed
  to `main`.
- The `admit` refusal and the `advance` guard overlap on purpose: the guard is
  the backstop for a pin that landed after admission (for example a lane whose
  work was merged by another path), and it keeps a reviewer from being spent.
- The distinction between "newer done not arrived" and "nothing new" uses the
  lane record's status, the only durable signal available at admit time:
  `Resolved` means no newer done can arrive; any other status means one may.
```

