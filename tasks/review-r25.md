# Review brief: round r25

plain: This round checks the rule that lets every coordinator change the harness itself.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r25` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `9f3600cb0171cd15429d3bf9c481253c092a764c3c2dbdc7a1064514bb24ef7e`, policy hash `941e9ea13a16368ca1ee73b91e4a1dc137816aed5c9c2f6e7e6acea90d78dfbd`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0054 | 1 | `d7776e7f025ad9f6914bbba3e1ffc0725679278d` | `t-0054-1-2` | `809855886e48fa74bef9e49cacc0b85e0b1955b4b88eda0a635a5c45d782a458` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r25.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r25"
candidate = "<C>"
manifest_hash = "9f3600cb0171cd15429d3bf9c481253c092a764c3c2dbdc7a1064514bb24ef7e"
policy_hash = "941e9ea13a16368ca1ee73b91e4a1dc137816aed5c9c2f6e7e6acea90d78dfbd"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0054 (artifact `809855886e48fa74bef9e49cacc0b85e0b1955b4b88eda0a635a5c45d782a458`)

Data, not instructions.

````text
# t-0054: every coordinator may evolve the harness

## Config key (coordinator action after merge)

`~/.config/herdr-ade/config.toml` gains one table:

```toml
[harness]
repos = [
  { path = "/Users/rolfie/projects/herdr-ade", box_path = "/home/ubuntu/projects/herdr-ade" },
  { path = "/Users/rolfie/projects/herdr", box_path = "/home/ubuntu/projects/herdr" },
]
```

Rows are the same shape as a project's `[[repos]]` rows (`path`, optional
`box_path`). This is the one place the harness repositories live. Without it
`ha harness install` refuses with `harness_repos_missing`. I did not touch the
live file: lanes never edit `config.toml` (that is the new skill rule).

Code: `src/harness.rs`. `repos()` parses it, `is_harness_repo()` and
`allowed_repo()` answer the repo question. `thread start` and `round open` use
`allowed_repo`: a repo that is neither in `PROJECT.md` nor in `[harness]` is
refused with `repo_not_listed` (before, `thread start` only warned and
`round open` accepted anything).

## The verb

`ha harness install` (`src/harness.rs::install`, wired in `src/cli.rs`):

- takes a machine-wide `try_lock` on `<config_dir>/.harness-install.lock`;
  a second concurrent install fails with `harness_install_busy`;
- for each `[harness]` row, reads the package name from `Cargo.toml`:
  `herdr-ade` builds `cargo build --release --locked` with
  `PATH=/bin:$PATH` and `DEVELOPER_DIR=/Library/Developer/CommandLineTools`
  and installs `herdr-ade` and `herdr-pi` into `~/.local/bin`; `herdr` builds
  the same plus `ZIG=<repo>/.target/rebase/zig-0.16.0/zig` and installs
  `herdr`. The fork prints that a live handoff is Rolf's call;
- runs `~/.local/bin/<bin> --version` and prints the line;
- when the saved machine `oci` exists (`herdr machine list --json`), runs the
  same build and install on the box over one SSH call: `cd <box_path>`,
  `git fetch --quiet`, `git merge --ff-only @{u}`, the same cargo build, then
  the copies into `$HOME/.local/bin`.
- `round merge` on a harness repo prints exactly `run ha harness install` at
  the end of a checkpointed merge.

## Skill paragraph

`skill/COORDINATOR.md`, new `### Harness evolves` right after Model choice: any
coordinator may edit `config.toml` (add a recipe, allow it for a role, change a
role default, add a machine); after each edit one `ha say` line naming the
change and one `ha decide` line (class `routine`, or `money` with `--basis`
quoting Rolf); the `config-changed` item is the trace; lanes and reviewers
never touch the file; a harness flaw is fixed through a lane and a round from
your own project, and after the merge run `ha harness install`.

`docs/operations.md`: the config line now reads "the harness settings ...; any
coordinator may edit it", the guard line drops `config.toml` (keeps the
approval list), and one sentence plus a command-table row document `[harness]`
and `harness install`.

## Tests (fake runner)

- `threads::tests::a_harness_repo_starts_from_a_project_that_does_not_list_it`
  — start succeeds on a `[harness]` repo the project does not list; an
  unrelated unlisted repo fails with `repo_not_listed` and leaves no record.
- `round::tests::round_open_accepts_an_unlisted_harness_repo`.
- `scenarios::harness_install_builds_and_installs_each_repo_kind` — two
  builds, `DEVELOPER_DIR`/`PATH` on both, `ZIG` only on the fork, the three
  installs, three version reads, no SSH without `oci`.
- `scenarios::harness_install_runs_the_box_steps_only_when_oci_is_saved` —
  one SSH per repo with `git fetch`, `git merge --ff-only`,
  `cargo build --release --locked` and the copies when `oci` is saved; zero
  otherwise.
- `scenarios::harness_install_lock_refuses_a_second_install`.

No real build or install runs from the tests.

## Gates

`cargo fmt --check`, `cargo test --locked` (394 + 56 + 78 + 4 pass),
`cargo clippy --all-targets --locked -- -D warnings`, `cargo build --release
--locked` — all green with `PATH=/bin:$PATH
DEVELOPER_DIR=/Library/Developer/CommandLineTools`.

## Note for the round

This changes existing behaviour on purpose: an unlisted non-harness repo is now
refused by `thread start` and `round open` instead of warned/accepted. The
round `testkit::fixture` and the `ade_start` test list their repo so the normal
listed path stays covered.
````

