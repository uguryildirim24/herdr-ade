+++
verdict = "MERGE"
round = "r15"
candidate = "dda3c9ca9f9df4a6280e52dc15cac88f0124b78f"
manifest_hash = "606c1798c766fdc34e9107335ac98f01f1a09c5f50a61111a968794059e8e30c"
policy_hash = "7cf667dea9748fe2844267e971e613ff74b94a3ef0af9df5ee974d759cb7e9da"
gates = []
+++

# Round r15 review

## t-0035 — relay file reads

The pinned lane correctly adds trailing `READ` and `LIST` requests, resumes the same Codex thread, keeps intermediate answers from pi, confines canonical paths to the configured roots, applies the file, round, list and request-round limits, and records roots and use in the relay state and logs.

I fixed five issues in place:

- The live bridge escapes Markdown punctuation. The relay now removes exactly the observed escapes before stripping the bridge note, parsing requests, or returning final text. The test covers escaped headings, section markers, the bridge note, and a file path, so the live `READ` line is recognised.
- `LIST` now respects the bytes left in the 1 MB round budget and stops scanning after enough entries to establish the 500-entry cut.
- Cutting a text file through a multi-byte UTF-8 character no longer mislabels it as binary.
- A sensitive spelling in the requested path is refused before symlink resolution, so a symlink cannot erase `token`, `.env`, or another blocked spelling.
- If a later Codex turn fails after reads were served, the failed request log keeps the real round and file counts instead of writing zeroes.

No gates were listed for this round. I also ran the lane's four checks after the review fixes:

```text
$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(no output; exit 0)

$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets --locked -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.92s

$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked
running 4 tests
...
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.27s

$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo build --release --locked
    Finished `release` profile [optimized] target(s) in 13.82s
```

The test run passed 357 main-binary tests, 50 pi-binary tests, 78 Pro-binary tests, and 4 CLI tests. No live Pro request was sent.
