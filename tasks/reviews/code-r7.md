+++
verdict = "MERGE"
round = "r7"
candidate = "6269d091acce4b06c079bf78bf34599c05819800"
manifest_hash = "0eaf668eb1063d5c789a78053d30a2df5c3b3d3ae00ce9fef5a5bfc9d3299abf"
policy_hash = "e1529634be8dfdf8688cad0bfcfd99dfa67f7f19fe2d1c3c7ab9f12ee7c0eec3"
gates = []
+++

# Round r7 review

## Verdict

MERGE. Lane t-0013 puts picture generation on Codex's real backend with `gpt-6-astra`, high reasoning, and `image_generation`; it does not route pictures through the bridge or the Pro model. The profile and short instructions live in the plugin-owned Codex home, and the lane skill says each call spends one turn.

I added one `review(pro):` commit. It fixes repeat calls after the first stopped lane, keeps picture profiles out of Pro bridge turns, skips rollout waiting on profile resumes, stops a non-kept picture lane on failures, and makes the final PNG copy no-clobber. Generated PNGs are ignored in lane worktrees. A profile lane is only reused when its persisted profile is exactly `gpt-image-gen`.

`Lane.profile` has a serde default, so existing lane records load. The 20-minute image wait checks the herdr agent every poll and exits immediately when it is blocked. `image.lock` uses `flock`; the kernel releases the lock when the process exits, including after a crash. The lock file remains as the stable inode and does not remain locked.

The canonical `herdr-pro` link fix is idempotent. The separate `gpt-image-gen.config.toml` contains no bridge URL. Normal Pro starts and resumes retain their doctor, bridge, cooldown, and rollout gates. I sent no picture turn and no Pro message, and did not run `herdr-pro init` against the real state directory or touch `~/.codex`.

## Gates

The round manifest lists no gates, so the front matter is `gates = []`. I also ran every gate required by the reviewer task, with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`.

```text
$ cargo fmt --check
(no output)
exit 0
```

```text
$ cargo test --locked

test result: ok. 52 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.83s

     Running tests/cli.rs (target/debug/deps/cli-a145eb26b3815bdb)

running 4 tests
test ticker_start_without_projects_creates_nothing ... ok
test path_like_names_and_slugs_are_refused ... ok
test context_prints_a_usable_prefix_in_a_scrubbed_environment ... ok
test peek_records_nothing_and_context_records_seen_items ... ok

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.22s
exit 0
```

The omitted earlier test binaries also passed: 324, 41, and 52 tests, followed by the 4 CLI tests shown above.

```text
$ cargo clippy --all-targets --locked -- -D warnings
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 0.08s
exit 0
```

```text
$ cargo build --release --locked
   Compiling serde_spanned v1.1.1
   Compiling toml_datetime v0.7.5+spec-1.1.0
   Compiling toml v0.9.12+spec-1.1.0
   Compiling serde_derive v1.0.229
   Compiling clap_derive v4.6.7
   Compiling clap v4.6.7
    Finished `release` profile [optimized] target(s) in 10.84s
exit 0
```
