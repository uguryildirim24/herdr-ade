# W28 report

## Result

- Acceptance values now remain one condition while each sentence is checked independently.
- A refusal identifies the condition and sentence that failed and points to separate `--acceptance` values as another option.
- Project-scoped commands now take the project slug positionally; the `--project` flag is removed, including from the native hook command.
- `round show <project>` lists rounds, while `round show <project> <round>` shows one round. No `round list` alias was added.
- Coordinator skill and operations documentation use the positional command forms.

## Tests

- `cargo fmt --check`
- `cargo test` (644 library, 58 pi, 86 pro, and all integration tests passed)
- `cargo clippy --all-targets -- -D warnings`
- `git diff --check`
