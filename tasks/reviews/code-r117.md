+++
verdict = "MERGE"
round = "r117"
candidate = "5fc03667c291b8e9b46985bf873b59b1440cb5eb"
manifest_hash = "d91d5480ad02f98c02d16b2120caf978a7e38ac1d30c3edbc3ba38a53eb48b03"
policy_hash = "7d013b92516df0434273f0f2ae57e9ec661dd6ba01b7b9a588a327b3cc724425"
gates = [{ command = "cargo fmt --check", exit = 0 }, { command = "cargo test", exit = 0 }, { command = "cargo clippy --all-targets -- -D warnings", exit = 0 }, { command = "git diff --check", exit = 0 }]
+++

# Review

## t-0314

The storage move is complete and consistent across local records, box records, courier paths, lane cards, artifacts, reports, documentation, and tests. The conversion preserves existing bytes, keeps live project-owned lane folders in place, refuses old/new conflicts before moving data, and is idempotent.

I fixed two review findings. Installation now converts local records immediately after the new plugin image is installed and converts box records immediately after the new box plugin is installed, so a later build, settings step, or machine lookup failure cannot leave new readers pointed at unmoved records. Conversion also fails closed when the old thread-record path cannot be read instead of silently marking malformed data as converted.

All pinned gates pass on candidate `5fc03667c291b8e9b46985bf873b59b1440cb5eb`.
