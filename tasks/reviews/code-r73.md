+++
verdict = "MERGE"
round = "r73"
candidate = "736cf6aa5e47062a61dd2befb9067fb5482674f2"
manifest_hash = "ca4b5c692bf1f06f8e751a1f3b31d41568bf589d1cbd41fa6b8d2d160ddb1df9"
policy_hash = "bff8071031d843087f976592f50ab33dc6b68d153f49f9b3e9a328610080ef29"
gates = ["cargo fmt --check", "cargo test", "cargo clippy --all-targets -- -D warnings", "git diff --check"]
+++

## t-0174

The lane now keeps non-disposable ignored files during local and box thread resolution and closed-round review cleanup. It also reports retained data separately in `doctor` and documents the editable disposable list.

The review repaired a box-only deletion hole where an ignored filename matching the inspection marker could hide itself, switched status and nested-checkout transport to collision-safe NUL framing, and made malformed framing fail closed. It also stopped a leading slash in a disposable entry from broadening a root path into a component-wide match. Dirty worktrees take precedence over retained-data warnings in `doctor`.

All requested gates pass.
