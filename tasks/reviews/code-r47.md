+++
verdict = "MERGE"
round = "r47"
candidate = "65700545ec23aced17e13e5b65368c987025eaee"
manifest_hash = "f0515d8bc807e6fd33f8f8d8aef703d1d0f1063928a47f4c5147f19205a9d64d"
policy_hash = "518f1148d931b343359ba456509849e5bef9394f3aa3ca9eb2253c85e9652f4d"
gates = []
+++

# Round r47 review

## t-0098 — finished-job history for the picker

MERGE as a historical corpus, **not as calibrated routing ground truth**. The
pinned lane adds only the 90-case JSON file and its detailed audit report. No
blocking defect found; no review repair was needed.

The supported case schema keeps outcome labels outside classifier state. All
90 original task texts match their recorded hashes and occur in the committed
birth briefs; all 45 embedded review briefs match the birth trees exactly. The
40/0/50 tier split, two completed fixed-product exclusions, and distinctions
between first MERGE, replacement verdicts, and successful review rejection are
consistent with the audit and the cited Git history. The r12 rejection really
names four omissions; the t-0071 cheap success is not a stronger-model rescue.

The report correctly treats all 50 strong labels as uncertain recommendations
or observed-success bounds, preserves stronger-review repairs on cheap cases,
and refuses to invent missing scores or failure causes. Its weight correction
and confidence/borderline arithmetic agree with `src/routing.rs` and the
checked-in policy. Do not use this corpus alone to claim routing accuracy,
minimum required capability, independent samples, or measured cost savings.
No saved responses exist: evaluating this file would make live scoring calls.

## Gates and supplemental checks

The brief lists no formal gates. No Cargo build, product test, live inference,
installation, settings write, or integration-branch move was performed.
Cloud completion requires publishing this lane's own branch before `ha done`;
that is not an integration push or merge.
The complete supplied diff was read and its additions byte-matched to the two
merged files; the lane report also matches the brief's artifact SHA-256.

Read-only validation ran on `oci`; its script and full output are in this
review lane's library, `.herdr-project/adeherdr-t-0103/library/`:

```text
$ python3 .herdr-project/adeherdr-t-0103/library/validate_history.py
PASS: 90 cases, tier counts 40/0/50; 92 audit entries; sealed lane report matches brief hash
PASS: 43 rounds, 42 first MERGE / 1 REJECT; ranked first MERGE 86/90, first-V landing 78/90
done_commit_references: 96
final_sha_ancestry: 92
full_birth_review_briefs: 45
full_briefs_and_hashes: 90
local_artifact_hashes: 15
local_done_events: 15
ranked_first_merge: 86
ranked_landed_first: 78
unavailable_artifact_references: 81
unavailable_done_events: 81
verdict_reference_checks: 182
LIMIT: launch models/attempt counters and authoritative merge records live on the Mac; not independently verified here.
PASS: no live inference, no settings writes, no product execution
(exit 0)

$ git diff --check ca5d598 HEAD
(no output; exit 0, before the verdict commit)

$ git merge-base --is-ancestor 30397a49ac993dade0eaf1942c91eb6093d3ffe7 HEAD && git merge-base --is-ancestor e73d648bb704480c0cf14b589e79865f5f30e4ac HEAD
(no output; exit 0, candidate contains the pin and brief B)
```

The 182 verdict-reference checks read first and accepted verdict files for the
91 audited threads with carrying rounds; repeated references are not independent
rounds. The 96 DONE references cover 95 distinct report hashes; only 15 reports
were available locally for independent hash checks. I do not repeat the lane's
claim of verifying every sealed report as a check performed by this reviewer.

## Manifest availability

```text
$ /home/ubuntu/.local/bin/ha --root /home/ubuntu/.herdr-ade round show adeherdr r47
herdr-ade: round_manifest_unavailable: /home/ubuntu/.herdr-ade/adeherdr/.state/rounds/r47.toml cannot be read (No such file or directory (os error 2)); membership is not rebuilt from events
(exit 1)
```

OCI has no canonical r47 record. The only available Git brief revision is B
`e73d648bb704480c0cf14b589e79865f5f30e4ac`; no stale revision was observed, but
live manifest freshness could not be independently checked here. This verdict
uses the supplied hashes; the coordinator's merge command must validate them
against its authoritative record. No record was fabricated or repaired.
