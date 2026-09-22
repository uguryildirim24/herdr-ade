//! Build identity shared by process freshness checks.

/// The package version and commit carried by a binary, without the
/// machine-specific build stamp.
///
/// `--version` output may include the binary name. A build produced by this
/// crate has the shape `0.1.0+<commit>.<stamp>`; comparisons deliberately stop
/// before `<stamp>` because two machines build the same commit at different
/// times.
pub(crate) fn commit_version(version: &str) -> Option<&str> {
    let version = version.split_whitespace().find(|part| part.contains('+'))?;
    let plus = version.find('+')?;
    let metadata = &version[plus + 1..];
    let commit_len = metadata.find('.').unwrap_or(metadata.len());
    (commit_len > 0).then(|| &version[..plus + 1 + commit_len])
}

/// True when both versions were built from the same package version and
/// commit, regardless of their per-machine build stamps.
pub(crate) fn same_commit(left: &str, right: &str) -> bool {
    commit_version(left)
        .zip(commit_version(right))
        .is_some_and(|(left, right)| left == right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn machine_specific_stamps_do_not_make_the_same_commit_stale() {
        assert!(same_commit(
            "herdr-ade 0.1.0+2d22e69.1790044097",
            "0.1.0+2d22e69.1790044082"
        ));
    }

    #[test]
    fn a_different_commit_is_stale() {
        assert!(!same_commit(
            "0.1.0+2d22e69.1790044097",
            "0.1.0+c50263a.1790044082"
        ));
    }
}
