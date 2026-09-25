//! Repository-relative gate globs: `/` separates segments, `*` and `?`
//! match within a segment, and a whole `**` segment matches zero or more
//! directories. No absolute paths, escapes, character classes or negation.
use anyhow::{Result, bail};

pub(crate) fn validate(pattern: &str) -> Result<()> {
    if pattern.is_empty()
        || pattern.starts_with('/')
        || pattern.split('/').any(|part| {
            part.is_empty() || part == "." || part == ".." || (part.contains("**") && part != "**")
        })
        || pattern.contains(['[', ']', '{', '}', '\\', '!'])
    {
        bail!(
            "gate_paths_invalid: `{pattern}` is not a repository-relative glob (*, ?, ** segments)"
        );
    }
    Ok(())
}

fn segment(pattern: &str, name: &str) -> bool {
    let p = pattern.as_bytes();
    let n = name.as_bytes();
    let mut matches = vec![false; n.len() + 1];
    matches[0] = true;
    for &ch in p {
        let mut next = vec![false; n.len() + 1];
        if ch == b'*' {
            next[0] = matches[0];
        }
        for i in 1..=n.len() {
            next[i] = match ch {
                b'*' => next[i - 1] || matches[i],
                b'?' => matches[i - 1],
                _ => matches[i - 1] && ch == n[i - 1],
            };
        }
        matches = next;
    }
    matches[n.len()]
}

pub(crate) fn matches(pattern: &str, path: &str) -> bool {
    let parts: Vec<_> = pattern.split('/').collect();
    let names: Vec<_> = path.split('/').collect();
    let mut dp = vec![false; names.len() + 1];
    dp[0] = true;
    for part in parts {
        let mut next = vec![false; names.len() + 1];
        if part == "**" {
            next[0] = dp[0];
            for i in 1..=names.len() {
                next[i] = dp[i] || next[i - 1];
            }
        } else {
            for i in 1..=names.len() {
                next[i] = dp[i - 1] && segment(part, names[i - 1]);
            }
        }
        dp = next;
    }
    dp[names.len()]
}

#[cfg(test)]
mod tests {
    #[test]
    fn directory_glob_does_not_match_a_sibling() {
        assert!(super::matches("src/**", "src/lib.rs"));
        assert!(super::matches("src/**", "src/a/lib.rs"));
        assert!(!super::matches("src/**", "src2/lib.rs"));
        assert!(!super::matches("src/**", "paper/a.md"));
    }
}
