+++
verdict = "MERGE"
round = "r114"
candidate = "d4a62f47515cb156b1e5b60a89aeea83fb2ae6ee"
manifest_hash = "b098182c578c64fd8b9834c9b74f33094e64d413df97d986fde5b68171383469"
policy_hash = "7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425"
gates = [{ command = "cargo fmt --check", exit = 0 }, { command = "cargo test", exit = 0 }, { command = "cargo clippy --all-targets -- -D warnings", exit = 0 }, { command = "git diff --check", exit = 0 }]
+++

# Review verdict

MERGE.

- `t-0308`: The merge command carries the installer’s typed binary, process, and task proof into its result, renders the same process evidence as a standalone install, forwards warnings, and resumes publication and installation without repeating completed effects. Closed rounds no longer regain stale verdict attention.
- `t-0309`: The current project page keeps each open task to one status-and-next-action line while retaining applicable task notes, instructions, and facts in their dedicated sections. Pasted-message previews extract Rolf’s words from id-bearing wrappers.
- `t-0310`: Automatic round allocation now advances beyond durable records, review refs, briefs, and verdicts. Review repair adds support for a surviving revision branch such as `review/r7-3`, and the command documentation now describes the monotonic allocation rule.

All pinned gates passed on candidate `d4a62f47515cb156b1e5b60a89aeea83fb2ae6ee`.
