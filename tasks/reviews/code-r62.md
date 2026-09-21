+++
verdict = "REJECT"
round = "r62"
candidate = "ea72eb5dca372309e7cae7d72357dbe4ddb08ae7"
manifest_hash = "b79f888a6e11b3dbe0bcbd9f092bee12e97851c0e3719cfe8c247e8352b90f43"
policy_hash = "73e54c401f3a836f4c9cf19e285c41ea5f3b2f356909a0752dfe99f8d905a85c"
gates = []
+++

# Round r62 review

## t-0142 — REJECT

The ordinary home workspace is omitted and a real second leak remains visible, but the implementation does not mirror the fork's rule as required. The fork refuses to hide a custom-labelled workspace even when its displayed label is `~`. `WorkspaceInfo` does not expose `custom_label`, and this candidate treats every agentless one-tab, one-pane workspace displayed as `~` as the default home. A workspace explicitly renamed to `~` is therefore hidden from both doctor rows even though the client shows it and it is a real leak.

The claim that the custom-label guard folds into the displayed-label test is false when the custom label itself is `~`; the fork accepts that rename. This needs an authoritative custom-label signal in the fork's workspace API (or an equivalent exact signal), deserialization in `herdr-ade`, and a check that the signal is false before omitting the workspace. The regression coverage must also show that a custom-labelled `~` workspace still fails the leak row.

No project gates were listed. Additional review checks:

```text
$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo fmt --check
(exit 0; no output)

$ PATH=/bin:$PATH DEVELOPER_DIR=/Library/Developer/CommandLineTools cargo test doctor::tests::a_machines_own_home_workspace_is_not_a_leak -- --exact
running 1 test
test doctor::tests::a_machines_own_home_workspace_is_not_a_leak ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 544 filtered out
```
