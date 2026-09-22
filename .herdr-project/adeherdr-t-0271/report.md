# W28 report

## Result

- Acceptance values now remain one condition while each sentence is checked independently.
- A refusal identifies the condition and sentence that failed and points to separate `--acceptance` values as another option.
- Person-facing project commands now take the project slug positionally and reject `--project`. The installed native hook keeps its existing `--project` machine interface so live coordinator hooks continue to work.
- `round show <project>` lists rounds, while `round show <project> <round>` shows one round. No `round list` alias was added.
- Coordinator skill and operations documentation use the positional command forms.

## Tests

- `cargo fmt --check`
- `cargo test` (including exact parsing of the installed hook command)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
