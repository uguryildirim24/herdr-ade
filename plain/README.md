# Plain-language word lists

These files are inputs to the pure checker in `src/plain.rs` (SPEC-ADE D17).
The checker does not write them.

## `words.txt`

Everyday English, 5000 words, lowercase, unique, sorted.

- Source: SCOWL (Spell Checker Oriented Word Lists),
  <https://github.com/en-wl/wordlist> and <http://wordlist.aspell.net/>.
- Version: SCOWL 2020.12.07 (`rel-2020.12.07`, commit `5ef55f9c4273`).
- Cut: `final/english-words.10` (most common band: Moby 1000, Internet 1000,
  and Brian Kelk frequency class 16) plus enough of `final/english-words.20`
  (Kelk frequency classes 7–15) to reach 5000 ASCII alphabetic words.
- Licence: the SCOWL collective-work grant (Copyright 2000-2018 Kevin
  Atkinson) permits use, copy, modify, distribute and sell of these word
  lists, provided the copyright notice and permission notice appear in
  supporting documentation. Size 10 and 20 are built from public-domain
  sources (Moby Words II, Brian Kelk's UK English Wordlist with Frequency
  Classification). The Atkinson permission notice:

      Permission to use, copy, modify, distribute and sell these word
      lists, the associated scripts, the output created from the scripts,
      and its documentation for any purpose is hereby granted without fee,
      provided that the above copyright notice appears in all copies and
      that both that copyright notice and this permission notice appear in
      supporting documentation. Kevin Atkinson makes no representations
      about the suitability of this array for any purpose. It is provided
      "as is" without express or implied warranty.

  Full notices: <https://raw.githubusercontent.com/en-wl/wordlist/rel-2020.12.07/scowl/Copyright>.

The Google 10,000-word frequency dump was not used: its GitHub licence
metadata is `NOASSERTION`, so it is not a passing redistribution source.

A pass on this list is form compliance, never a proof that the text is
understandable (SPEC-ADE D17, item 29).

## `vocabulary.txt`

Plugin nouns named by SPEC-ADE D17 R4: lane, round, reviewer, build, branch,
merge, report, checkpoint, and the rest of the plugin's own nouns. Lowercase,
unique, sorted. Not a second everyday list.
