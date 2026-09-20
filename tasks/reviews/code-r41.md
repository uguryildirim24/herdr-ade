+++
verdict = "MERGE"
round = "r41"
candidate = "3be77aa8bfd2cd4b516d333d1858f95b773ff2eb"
manifest_hash = "788efc90e48ab9a000436ec6f4c522e6ee3c68717948813f542c28db43b5ab8b"
policy_hash = "3401228a1791aa7e7a698c29f1126da65d46d4a840c3c529b226b4d737856174"
gates = []
+++

# Round r41 review

## t-0089

MERGE. The lane adds short commands to overturn a recorded decision and withdraw any open ask. Both actions retain their history, actor, time, and reason. Overturned choices remain visible on the project screen and in coordinator context. Withdrawn asks leave the open set and board, remain visible in conversation history when published, and cannot later be answered, re-asked, or published.

Duplicate detection compares normalized question words before any counter or record write and names the existing open ask. New ask output is one physical line beginning with its id. The report accurately states the normalization's limits.

I fixed one publication race in `review(ask): serialize publication with closure`: ask publication now shares the ask-set lock with answer and withdrawal. A publisher that checked an ask just before withdrawal can no longer restore that closed card to the board afterward.

## Gates

The round brief listed no gates, so I ran none.
