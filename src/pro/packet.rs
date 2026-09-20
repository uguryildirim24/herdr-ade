//! The packet Pro reads (SPEC-pro-bridge v2, "Turn").
//!
//! The brief, then every named file under a `=== <abs path> ===` header, then
//! the one-message instruction. Caps: 60k estimated tokens and 200 KB
//! (spec §4). A path that looks like a secret is refused before it is read.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

use super::{PACKET_MAX_BYTES, PACKET_MAX_TOKENS};

#[derive(Debug, Clone, PartialEq)]
pub struct Packet {
    pub text: String,
    pub bytes: usize,
    pub tokens: u64,
    /// The absolute path of every file that went in, brief first.
    pub files: Vec<String>,
}

/// A rough token estimate: four bytes per token, rounded up.
fn estimate_tokens(bytes: usize) -> u64 {
    (bytes as u64).div_ceil(4)
}

/// The reason a path must not enter a packet, if any (spec §3, the
/// secret-name filter).
fn secret_reason(path: &Path) -> Option<&'static str> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if name.starts_with(".env") {
        return Some(".env file");
    }
    if name == "auth.json" {
        return Some("auth.json");
    }
    if name.starts_with("credentials") {
        return Some("credentials file");
    }
    if name.ends_with(".key")
        || name.ends_with(".pem")
        || name.starts_with("id_rsa")
        || name.starts_with("id_ed25519")
    {
        return Some("key file");
    }
    if path.components().any(|c| c.as_os_str() == ".ssh") {
        return Some(".ssh directory");
    }
    None
}

/// The backtick run to wrap `content` in: one longer than any run inside, so a
/// file that itself holds a fence cannot close the wrapper early.
fn fence_for(content: &str) -> String {
    let mut longest = 0usize;
    let mut run = 0usize;
    for c in content.chars() {
        if c == '`' {
            run += 1;
            longest = longest.max(run);
        } else {
            run = 0;
        }
    }
    "`".repeat(longest.max(2) + 1)
}

fn section(path: &Path, text: &str) -> Result<String> {
    refuse_secret(path)?;
    let absolute =
        std::path::absolute(path).with_context(|| format!("bad path {}", path.display()))?;
    let canonical = std::fs::canonicalize(&absolute)
        .with_context(|| format!("could not resolve {}", absolute.display()))?;
    refuse_secret(&canonical)?;
    let fence = fence_for(text);
    Ok(format!(
        "=== {} ===\n{fence}\n{}\n{fence}\n\n",
        absolute.display(),
        text.trim_end_matches('\n'),
    ))
}

/// Build the packet for one turn. The brief and every attachment must exist
/// and pass the secret filter.
pub fn build(brief: &Path, attachments: &[PathBuf], out: &Path) -> Result<Packet> {
    refuse_secret(out)?;
    if let (Some(parent), Some(name)) = (out.parent(), out.file_name()) {
        let parent = std::fs::canonicalize(parent)
            .with_context(|| format!("could not resolve output folder {}", parent.display()))?;
        refuse_secret(&parent.join(name))?;
    }
    let brief_text = std::fs::read_to_string(brief)
        .with_context(|| format!("could not read the brief {}", brief.display()))?;
    let mut text = section(brief, &brief_text)?;
    let mut files = vec![absolute_string(brief)?];
    for attachment in attachments {
        let content = std::fs::read_to_string(attachment)
            .with_context(|| format!("could not read the attachment {}", attachment.display()))?;
        text.push_str(&section(attachment, &content)?);
        files.push(absolute_string(attachment)?);
    }
    let out = std::path::absolute(out).with_context(|| format!("bad path {}", out.display()))?;
    text.push_str(&format!(
        "Answer in one message. Your whole reply is written verbatim to `{}`.",
        out.display()
    ));

    let bytes = text.len();
    let tokens = estimate_tokens(bytes);
    if bytes > PACKET_MAX_BYTES {
        bail!(
            "packet is {bytes} bytes; the cap is {PACKET_MAX_BYTES} (drop or split an attachment)"
        );
    }
    if tokens > PACKET_MAX_TOKENS {
        bail!(
            "packet is about {tokens} tokens; the cap is {PACKET_MAX_TOKENS} (drop or split an attachment)"
        );
    }
    Ok(Packet {
        text,
        bytes,
        tokens,
        files,
    })
}

fn refuse_secret(path: &Path) -> Result<()> {
    if let Some(reason) = secret_reason(path) {
        bail!(
            "refusing `{}`: it looks like a {reason} and never enters a packet",
            path.display()
        );
    }
    Ok(())
}

fn absolute_string(path: &Path) -> Result<String> {
    Ok(std::path::absolute(path)
        .with_context(|| format!("bad path {}", path.display()))?
        .display()
        .to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, text).unwrap();
        path
    }

    #[test]
    fn the_packet_holds_brief_files_and_the_out_instruction() {
        let dir = tempfile::tempdir().unwrap();
        let brief = write(dir.path(), "brief.md", "Do the thing.\n");
        let spec = write(dir.path(), "SPEC.md", "Spec body.\n");
        let out = dir.path().join("answer.md");
        let packet = build(&brief, std::slice::from_ref(&spec), &out).unwrap();
        assert!(
            packet
                .text
                .starts_with(&format!("=== {} ===", brief.display()))
        );
        assert!(packet.text.contains("Do the thing."));
        assert!(packet.text.contains(&format!("=== {} ===", spec.display())));
        assert!(packet.text.contains("Spec body."));
        assert!(packet.text.contains("written verbatim to"));
        assert_eq!(
            packet.files,
            vec![brief.display().to_string(), spec.display().to_string()]
        );
    }

    #[test]
    fn a_fence_inside_a_file_does_not_close_its_wrapper() {
        let dir = tempfile::tempdir().unwrap();
        let brief = write(dir.path(), "brief.md", "before\n```\ncode\n```\nafter\n");
        let packet = build(&brief, &[], &dir.path().join("o.md")).unwrap();
        // The wrapper fence must be longer than the three-backtick run inside.
        assert!(packet.text.contains("````\n"), "{}", packet.text);
    }

    #[test]
    fn a_secret_path_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let brief = write(dir.path(), "brief.md", "x");
        for name in [
            ".env",
            ".env.local",
            "auth.json",
            "credentials.json",
            "id_rsa",
            "server.pem",
            "k.key",
        ] {
            let secret = write(dir.path(), name, "SECRET");
            let error = build(&brief, &[secret], &dir.path().join("o.md")).unwrap_err();
            assert!(error.to_string().contains("refusing"), "{name}: {error}");
        }
        let ssh = dir.path().join(".ssh");
        std::fs::create_dir_all(&ssh).unwrap();
        let key = write(&ssh, "config", "host *");
        assert!(build(&brief, &[key], &dir.path().join("o.md")).is_err());
    }

    #[test]
    fn the_byte_cap_is_enforced() {
        let dir = tempfile::tempdir().unwrap();
        let brief = write(dir.path(), "big.md", &"x".repeat(PACKET_MAX_BYTES + 1));
        let error = build(&brief, &[], &dir.path().join("o.md")).unwrap_err();
        assert!(error.to_string().contains("cap"));
    }

    #[test]
    fn token_estimate_rounds_up() {
        assert_eq!(estimate_tokens(0), 0);
        assert_eq!(estimate_tokens(1), 1);
        assert_eq!(estimate_tokens(4), 1);
        assert_eq!(estimate_tokens(5), 2);
        assert_eq!(estimate_tokens(60_000 * 4), 60_000);
    }
}
