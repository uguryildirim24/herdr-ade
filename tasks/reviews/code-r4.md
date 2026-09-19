+++
verdict = "MERGE"
round = "r4"
candidate = "6bfc12311b445ff9eb7a460e79d8776fff7cb06c"
manifest_hash = "1372caaa4fc1d59f32ebda0f58661ea46291ab12751fe862b2417588eaead15c"
policy_hash = "172e51b8408490ed414d7c6414e94353ab6f2e321ec23e1d68c59ce47abc193d"
gates = []
+++

# Round r4 review

## t-0010

MERGE. The lane removes the Jev model picker, its module, records, tests, skill, calls, and live doctor probes. Launches now use a role's default or an explicit row from its allowed list. The effort check accepts Claude xhigh and refuses only unknown values. The coordinator defaults to Claude Opus at xhigh, while built-in research and planner roles use agy Gemini 3.8 Flash High and Claude Fable at xhigh.

I made one review commit. It refuses the removed `jev` PROJECT.md key instead of silently accepting it, and corrects the coordinator skill so `--recipe` describes the implemented allowed-list rule without claiming restart can change a recipe.

Historical thread records still deserialize: the release binary read the live `adeherdr` context containing old launch fields and exited 0. The release doctor's only failure against Rolf's real config was exactly `picker_removed: [recipes.review_codex_sol_high] cost is gone; the lane picker was removed`; all later pi checks were green. Generated research and planner bindings both accepted `skill <role>` and printed the lane skill. agy 1.2.7 lists only `gemini-3.8-flash-high`, `-medium`, and `-low` for Gemini 3.8.

The requested source grep leaves only removal errors and tests, generic UI pickers, plain-language and round gates, prose about operational cost, and test fixture vocabulary. No Jev model-picker implementation remains. `src/pro/` is unchanged.

## Commands

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(no output; exit 0)

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked
running 4 tests
....
test result: ok. 4 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.28s

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo clippy --all-targets --locked -- -D warnings
    Checking toml v0.9.12+spec-1.1.0
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 5.25s

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo build --release --locked
   Compiling clap v4.6.7
    Finished `release` profile [optimized] target(s) in 12.04s

$ target/release/herdr-ade --root /Users/rolfie/.herdr-ade context adeherdr --peek
## Routines (0)
(exit 0)

$ target/release/herdr-ade --root /Users/rolfie/.herdr-ade doctor
[FAIL] recipes: picker_removed: [recipes.review_codex_sol_high] cost is gone; the lane picker was removed
...
[ok  ] provider kimi-coding: login ready
[ok  ] ~/.codex: no openai_base_url override
[ok  ] ~/.pi: present; informational, the harness never writes it
herdr-ade: some checks failed
(exit 1, exactly one FAIL row)

$ agy --version; agy models | rg 'gemini-3\\.8'
1.2.7
gemini-3.8-flash-high  Gemini 3.8 Flash (High)
gemini-3.8-flash-medium Gemini 3.8 Flash (Medium)
gemini-3.8-flash-low   Gemini 3.8 Flash (Low)
```
