+++
verdict = "MERGE"
round = "r20"
candidate = "e6b72964db24440e18c9a3db16161c43cbf63ea3"
manifest_hash = "39ac26a4ce6c01bb61b3c6f6b59c025a2f1af339fb8f21a21525516f12a58e40"
policy_hash = "7cf667dea9748fe2844267e971e613ff74b94a3ef0af9df5ee974d759cb7e9da"
gates = []
+++

# Round r20 review

MERGE. The frozen manifest remains revision 1 and contains only t-0046 at the pinned sha. The brief commit and pinned lane sha are ancestors of the candidate.

## t-0046

The wrapper health row now uses `type -a pi` for bash, `whence -va pi` for zsh, and `command -v pi` otherwise. Its parser ignores login-shell chatter and function descriptions while retaining the first executable path. The same row serves `doctor` and `check`, so the cloud box no longer runs a zsh-only probe.

The `pro` doctor row is informational only when the shared `models.json` has no relay provider. A present relay provider still goes through the existing authentication check and fails when its login is unavailable. The provider-specific `check` path is unchanged.

During review, running the doctor tests with `SHELL=/bin/bash` exposed zsh-hardcoded fake commands in three existing tests. Commit `e6b7296` makes the shared fake follow the selected shell and makes the zsh chatter test explicitly select zsh. This changes no production behavior and keeps the same tests green under both target shells.

## Gates and focused proof

The round lists no gates, so `gates = []`.

I ran the doctor module under bash and zsh because shell selection is the defect this round names:

```text
$ SHELL=/bin/bash PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked pi::doctor::tests -- --nocapture
running 16 tests
...
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 40 filtered out
```

```text
$ SHELL=/bin/zsh PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test --locked pi::doctor::tests -- --nocapture
running 16 tests
...
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 40 filtered out
```
