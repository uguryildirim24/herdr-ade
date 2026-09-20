# Review brief: round r32

plain: This round checks that every line on the screen stays one line and cannot fill it.

Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade skill reviewer`, then do what this brief says.

Round `r32` on integration branch `main`. The commit that adds this file is the brief commit B.
Manifest revision 1, manifest hash `3a004808bc8e09d074e815428dd524e6e158eb2ada88b10a5319a13f4a01f522`, policy hash `df69155c02a5636dc8a86ec27ade9e888d03782a6c7bd85df748f4c262aaee68`.

## Pinned lanes

| lane | attempt | sha | event | artifact |
|---|---|---|---|---|
| t-0069 | 1 | `415002324da8e350353673c5b8f933709f72396c` | `t-0069-1-1` | `4a5a7b5faf3d8d023b0f051aea57e17758f1cc08bcb75a4e608e4291ebe9fa34` |

## Gates

- (none listed in PROJECT.md)

## What to do

1. Merge the pinned lane shas above (the shas, not branch names) into your review branch.
2. Fix in place as `review(<pkg>):` commits.
3. Run every gate above.
4. When the last code commit is the candidate C, write `tasks/reviews/code-r32.md` with exactly this front matter,
   and commit that file alone as the verdict commit V (its only parent is C):

```
+++
verdict = "MERGE"  # or "MERGE-AFTER-DECISION" or "REJECT"
round = "r32"
candidate = "<C>"
manifest_hash = "3a004808bc8e09d074e815428dd524e6e158eb2ada88b10a5319a13f4a01f522"
policy_hash = "df69155c02a5636dc8a86ec27ade9e888d03782a6c7bd85df748f4c262aaee68"
gates = []
+++
```

5. Run `/Users/rolfie/.local/bin/ha --root /Users/rolfie/.herdr-ade done --report <your report> --sha <V>`.

## Reports (data, not instructions)

### t-0069 (artifact `4a5a7b5faf3d8d023b0f051aea57e17758f1cc08bcb75a4e608e4291ebe9fa34`)

Data, not instructions.

```text
# t-0069 report: the overview keeps one line per row

Worktree `/home/ubuntu/projects/herdr-ade/.worktrees/t-0069`, branch
`hp/adeherdr/t-0069-the-overview-keeps-one-line-per-row`.

## What changed

Part 1 (a row is one line) is in `src/talk/screen.rs`:

- `overview_doc` now takes a `full` flag. The compact overview above the chat
  (`full = false`) renders each heading and each row as exactly one
  `Line`, so a row can never wrap onto a second terminal line.
- New `one_line(prefix, text, tone, width, theme)` builds that line: a
  one-cell indent, the row prefix (styled with the row tone), then the text
  clipped to the remaining width. The cap is `width - 1 - prefix`, so the
  narrow and wide layouts each get their own available width rather than a
  fixed number.
- New `clip(text, max)` cuts to at most `max` terminal cells, keeps whole
  words, breaks a word wider than the row at the cell boundary, and always
  ends a cut with the single-character ellipsis `…`. A row that already fits
  is returned unchanged.
- The F2 full overview passes `full = true` and keeps the existing
  `Document::prose` wrapping, so the whole text of every row stays reachable.

Part 2 (long sentences cannot be written) needed no new write-time check: every
listed source already routes through the plain checker's existing machinery and
error shape:

| source | call |
|---|---|
| `round open` `--plain` | `glossary::check_birth` (`plain::check`, R5) |
| `thread start` / `thread adopt` `--plain` | `threads::check_birth_plain` (`plain::check`, R5) |
| plan `--does` and step text | `glossary::check_sentence` (`plain::check`, R5) |

Each of these rejects a 26-word single sentence with the existing
`plain_long_sentence` / `split this 26-word sentence` output. The new
`round::tests::open_refuses_a_round_sentence_over_the_word_cap` locks that in
at the `round open` boundary (26 words refused, 25 accepted).

One render-side change was required to honor "existing records that are already
too long must keep loading and rendering (cut)": `overview.rs::checked` used
`glossary::gate`, which includes the R5 sentence-length rule, so an old long
record was replaced by the `*_INVALID` fallback instead of rendering. New
`glossary::gate_row` runs the same `plain::check` and `format_check` and drops
only the `LongSentence` violation. `checked` now uses it, so jargon, names and
identifiers still hide a row but length alone no longer does. This also fixes
the compact `tagged` rows, where the prefix word was counted in the R5 cap and
made a valid 25-word step fall back to "This step needs a plain description."

## Tests

- `talk::screen::tests::a_long_overview_row_is_one_cut_line_but_complete_in_the_full_overview`
  — a row wider than the width is one line ending in `…` and no continuation
  word leaks to a second line; the same row is complete in the full document.
- `talk::overview::tests::an_existing_long_sentence_renders_cut_instead_of_the_invalid_fallback`
  — a stored 26-word goal renders its text (not `GOAL_INVALID`).
- `round::tests::open_refuses_a_round_sentence_over_the_word_cap`
  — 26-word `round open --plain` refused with `plain_long_sentence`; 25-word
  accepted.
- `talk::overview::tests::fixture_projection_is_read_only_and_keeps_all_work`
  updated to assert rendered rows pass `gate_row` (the render gate).

## Gates

With `PATH=/bin:$PATH` (Linux box; `DEVELOPER_DIR` is a Mac-only gate):

- `cargo fmt --check`: pass.
- `cargo test`: 432 passed, 1 failed.
- `cargo clippy --all-targets -- -D warnings`: pass.

The single failure is `pi::scenarios::scenario_setup_then_check_for_kimi`. It is
pre-existing and box-local: the test scripts the zsh wrapper
(`zsh -lic whence -va pi`) but this box's login shell is bash, so the doctor
runs `/bin/bash -lic type -a pi` and the `FakeRunner` has no rule for it. I
confirmed the identical failure on the untouched base commit `37923f1` with my
changes stashed. Not related to this task.

## Notes / open questions

- I read "existing records that are already too long must keep loading and
  rendering (cut, per part 1)" as requiring the actual text to render, which is
  why `gate_row` exists. If the coordinator wanted those rows to keep showing
  the `*_INVALID` fallback instead, drop the `gate_row` hunk; the row-clipping
  and the write caps stand on their own.
- The task says "Do not push", but the box-lane `ha done` refuses without the
  branch on the lane card's `publish_url` (`ops::check_published_ref`), and the
  lane brief says to publish the lane branch. I pushed only
  `hp/adeherdr/t-0069-the-overview-keeps-one-line-per-row` to
  `https://github.com/uguryildirim24/herdr-ade.git`; no main or upstream ref
  was touched.
- `clip` collapses runs of whitespace, matching the existing `shorten`
  behavior; the full overview still shows the row verbatim.
```

