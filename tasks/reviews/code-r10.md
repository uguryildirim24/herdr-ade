+++
verdict = "MERGE"
round = "r10"
candidate = "b41e9d8ba62b3ed7980f8d332524ef992f3d2dfb"
manifest_hash = "d60559a43bbc86e9bbd39ea5dfa22b65780fe74fced615b510ab45291c8ae7c3"
policy_hash = "e1529634be8dfdf8688cad0bfcfd99dfa67f7f19fe2d1c3c7ab9f12ee7c0eec3"
gates = []
+++

# Round r10 review

## Verdict

MERGE. Both pinned lanes are ancestors of the candidate, and the full build, test, format, and lint gates pass.

### t-0018 — the harness starts the review itself

The event hook uses Herdr's `pane.agent_status_changed` dot name and runs the release binary from the plugin root. The hook entry point reads the event envelope plus Herdr's workspace and pane variables, maps coordinator and lane panes to projects, and falls back to all projects only when none match. The ticker runs the same advance pass after refreshing pins.

A ready round creates the standard brief, starts a thread with the reviewer role, and binds it. The advance lock prevents hook/ticker races. A bound reviewer cannot be replaced; a gone reviewer produces one durable inbox announcement and is never restarted. Verdict announcements record their token before the inbox and talk side effects. A MERGE verdict produces one inbox item and one talk line but never calls merge.

I added `review(round): keep one reviewer and read hook envelope`. It closes two gaps: a manual bind could replace the reviewer, and the hook ignored `HERDR_PLUGIN_EVENT_JSON` instead of using its pane and workspace as a fallback. The coordinator skill now says a gone reviewer is reported rather than replaced.

### t-0019 — picture references and lane nesting

`herdr-pro image --with` validates at most four readable regular files before starting a lane, places each on the profile lane's Codex start line as `--image <file>`, and names the attachments in the request. Bridge-backed Pro lanes receive no image arguments. Pro and picture starts and resumes choose the caller's pane, then the project coordinator pane, then no parent.

I added `review(pro): start a fresh lane for picture references`. Codex accepts image attachments only at process start, so a referenced call can no longer silently reuse a lane left ready by `--keep`. I also removed the stale documented `--parent` argument. No Pro message or picture turn was sent, and no real plugin state or `~/.codex` was touched.

## Proof and gates

The round manifest lists no gates, so the front matter remains `gates = []`. I ran the lane's reviewer proof and every gate required by the reviewer task with `PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools`.

```text
$ cargo test --locked round::tests::advance_starts_one_reviewer_and_never_a_second -- --exact
running 1 test
test round::tests::advance_starts_one_reviewer_and_never_a_second ... ok

test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 327 filtered out; finished in 1.14s
```

```text
$ cargo fmt --check
(no output)
exit 0
```

```text
$ cargo test --locked

test result: ok. 59 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 2.89s

     Running tests/cli.rs (target/debug/deps/cli-a145eb26b3815bdb)

running 4 tests

test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.28s
```

The earlier test binaries also passed: 328 and 41 tests, followed by the 59 Pro tests and 4 CLI tests shown above.

```text
$ cargo clippy --all-targets --locked -- -D warnings
    Checking serde v1.0.229
    Checking clap v4.6.7
    Checking toml v0.9.12+spec-1.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 4.31s
```

```text
$ cargo build --release --locked
   Compiling serde_derive v1.0.229
   Compiling clap_derive v4.6.7
   Compiling clap v4.6.7
    Finished `release` profile [optimized] target(s) in 10.27s
```
