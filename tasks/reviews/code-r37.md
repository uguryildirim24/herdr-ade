+++
verdict = "MERGE"
round = "r37"
candidate = "340000eae7c3f3df17bf36bc8c71040f15b9ab39"
manifest_hash = "294cf26e2b7fd0b50d7748af7d0973bdbae72d29b56ab78ab4752d6b3a32e711"
policy_hash = "3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174"
gates = []
+++

# Round r37 review

## t-0080

MERGE after one review fix.

- A4 uses the existing 32,000-character brief limit as the warning boundary. `ha context` and `ha doctor` name every directly inlined memory file and point at `memory/archive/`; archive files and symlinks remain excluded.
- E4 now drops only the known-word rule for decision, round and thread records while retaining name, identifier and length checks. The lane accidentally stopped enforcing the existing one-sentence rule on decisions. `review(plain): keep decision lines to one sentence` restores it and makes sentence counting treat only a dot inside a token as internal, so closing punctuation cannot hide another sentence.
- D2 puts the push rule in the standing lane skill. It keeps the required cloud-box exception scoped to publishing that lane's own branch; integration branches and `main` remain coordinator-only.
- D3 makes the scenario scripts use the same detected shell and probe as the doctor, covering bash on the box and zsh on the Mac.

## Gates

The round listed no gates, so the verdict gate list is empty.

I also ran these review checks after the fix:

```text
$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(no output; exit 0)

$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked
running 4 tests
...
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.27s

$ PATH="/bin:$PATH" DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets --locked -- -D warnings
Checking herdr-ade v0.1.0 (/Users/rolfie/projects/herdr-ade/.worktrees/t-0088)
Finished `dev` profile [unoptimized + debuginfo] target(s) in 6.08s
```

The full test command also passed 449 `herdr-ade`, 56 `herdr-pi`, 78 `herdr-pro`, and 4 CLI tests with no failures.
