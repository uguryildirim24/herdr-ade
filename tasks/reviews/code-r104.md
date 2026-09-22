+++
verdict = "MERGE"
round = "r104"
candidate = "514a31590a3264cd242390afd20753e07326a2e5"
manifest_hash = "4768b124cea1201cbea12c8834c8d1a171d3dcc4e25d02917c0a5d3a66fc0461"
policy_hash = "b0854bd6e2b014fabc7e40bfde1e4ee7fa636e894571a68daae3999272c7ef0e"
gates = []
+++

# Review r104

## t-0271

MERGE. Each acceptance value remains one condition, while its non-empty sentences are checked separately. A refusal identifies both the condition and sentence and names separate `--acceptance` values as the alternative.

The person-facing commands now take the project slug positionally and reject `--project`. `round show <project>` lists rounds, and the two-argument form still shows one round. The exact installed native hook line still parses with `plain hook --kind claude --project <slug> --binding <pane> --phase prompt`. A repository grep found no other generated or documented person-facing command using `--project`.

I found and fixed one parsing defect. `ask withdraw`, `decide overturn`, and `plan step edit` could no longer omit the slug because each has two later positional values. Their two-value forms now resolve the current project, while their three-value forms use the leading project slug. The focused parser checks and command probes cover the three forms.

## Requested checks

The round manifest lists no gates, so the front matter remains `gates = []`. I ran all four checks requested for this review on the candidate.

- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check` — exit 0; no output.
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test` — exit 0. Final suite lines:
  ```text
  test result: ok. 645 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 34.35s
  test result: ok. 58 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.81s
  test result: ok. 86 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.83s
  test result: ok. 9 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.90s
  test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.15s
  test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.08s
  test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
  ```
  The first full run had one transient failure in `harness::tests::installing_the_same_clean_commit_keeps_every_installed_inode` (exit 101); that exact test then passed, and the final full run above passed.
- `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets -- -D warnings` — exit 0. Final line:
  ```text
  Finished `dev` profile [unoptimized + debuginfo] target(s) in 23.94s
  ```
- `git diff --check` — exit 0; no output.
