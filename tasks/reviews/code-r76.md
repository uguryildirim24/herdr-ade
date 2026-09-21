+++
verdict = "MERGE"
round = "r76"
candidate = "fff5829dc043ab6a2eb61f45da107a96edb867ed"
manifest_hash = "187050bdac3226601b71e850fc9d213620e30012f365483c8445a6f6ebe55244"
policy_hash = "8e43e0ebbd105ebf9f903667c622cb0dad8aa881e4ceb5a344db4659ff0338da"
gates = []
+++

## t-0179

MERGE. Doctor limits finished-worktree inspection to resolved thread records and reports unreadable thread state as unknown. Disposable patterns are resolved from the global list plus only the matching project or harness repository row; component matching keeps `*` from crossing `/`, and nested Git checkouts are restored as durable data after disposable filtering.

The review fixed the post-r74 `thread cancel` call site so cancellation uses the same project-aware resolved list as `thread resolve` and closed-round cleanup. Regression tests now exercise all three paths, repository isolation, bounded wildcard matching, and nested-checkout retention.
