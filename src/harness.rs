//! The harness repositories and `ha harness install` (t-0054).
//!
//! The list lives once, in `[harness]` in `config.toml`: each row is a
//! `path`/`box_path` pair, the same shape a project's `repos` rows have. Every
//! project may start lanes and open rounds on a harness repository without
//! listing it in `PROJECT.md`.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::paths::Ctx;
use crate::project::{Repo, Settings};
use crate::remote;
use crate::runner::Cmd;

/// The saved machine whose box gets the same build and install.
pub const BOX_MACHINE: &str = "oci";
/// The plugin build's tool path, exactly as the coordinator uses it by hand.
pub const DEVELOPER_DIR: &str = "/Library/Developer/CommandLineTools";

const BUILD_TIMEOUT: Duration = Duration::from_secs(1800);
const BOX_BUILD_TIMEOUT: Duration = Duration::from_secs(3600);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);

/// The `[harness]` table of `config.toml`.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    harness: HarnessConfig,
}

#[derive(Debug, Default, Deserialize)]
struct HarnessConfig {
    #[serde(default)]
    repos: Vec<Repo>,
}

/// The harness repositories from `config.toml`. An absent table is an empty list.
pub fn repos(config_dir: &Path) -> Result<Vec<Repo>> {
    let file = config_dir.join("config.toml");
    let text = std::fs::read_to_string(&file).unwrap_or_default();
    if text.trim().is_empty() {
        return Ok(Vec::new());
    }
    let raw: RawConfig =
        toml::from_str(&text).with_context(|| format!("{} does not parse", file.display()))?;
    Ok(raw.harness.repos)
}

/// A path's canonical form when it exists, else the path as given.
fn canonical_or(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// True when `path` is one of the harness repositories.
pub fn is_harness_repo(config_dir: &Path, path: &str) -> bool {
    let target = canonical_or(path);
    repos(config_dir)
        .unwrap_or_default()
        .iter()
        .any(|repo| canonical_or(&repo.path) == target)
}

/// True when a project may start a lane or open a round on `path`: the path is
/// one of its own listed repositories, or a harness repository.
pub fn allowed_repo(settings: &Settings, config_dir: &Path, path: &str) -> bool {
    let target = canonical_or(path);
    settings
        .repos
        .iter()
        .any(|repo| canonical_or(&repo.path) == target)
        || is_harness_repo(config_dir, path)
}

/// The kind of build a harness repository needs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The plugin: `cargo build --release` with `DEVELOPER_DIR`, installs
    /// `herdr-ade` and `herdr-pi`.
    Plugin,
    /// The fork: the same build plus `ZIG`, installs `herdr`.
    Fork,
}

impl Kind {
    fn binaries(self) -> &'static [&'static str] {
        match self {
            Kind::Plugin => &["herdr-ade", "herdr-pi"],
            Kind::Fork => &["herdr"],
        }
    }
}

/// Read the repository's package name from its `Cargo.toml`.
fn kind(repo: &str) -> Result<Kind> {
    let file = Path::new(repo).join("Cargo.toml");
    let text = std::fs::read_to_string(&file)
        .with_context(|| format!("harness_repo_invalid: {} has no Cargo.toml", repo))?;
    let value: toml::Value = toml::from_str(&text)
        .with_context(|| format!("harness_repo_invalid: {} does not parse", file.display()))?;
    let name = value
        .get("package")
        .and_then(|package| package.get("name"))
        .and_then(toml::Value::as_str)
        .unwrap_or_default();
    match name {
        "herdr-ade" => Ok(Kind::Plugin),
        "herdr" => Ok(Kind::Fork),
        other => bail!(
            "harness_repo_unknown: {repo} is package `{other}`; only `herdr-ade` and `herdr` are harness repositories"
        ),
    }
}

fn zig_path(repo: &str) -> String {
    format!("{repo}/.target/rebase/zig-0.16.0/zig")
}

fn mac_path_env(ctx: &Ctx) -> String {
    format!("/bin:{}", ctx.env.var("PATH").unwrap_or_default())
}

fn local_build(ctx: &Ctx, repo: &str, kind: Kind) -> Result<()> {
    let mut cmd = Cmd::new("cargo", BUILD_TIMEOUT)
        .args(["build", "--release", "--locked"])
        .cwd(repo)
        .env("PATH", mac_path_env(ctx))
        .env("DEVELOPER_DIR", DEVELOPER_DIR);
    if kind == Kind::Fork {
        cmd = cmd.env("ZIG", zig_path(repo));
    }
    let out = ctx.runner.run(&cmd)?;
    if !out.success() {
        bail!(
            "harness_build_failed: cargo build in {repo}: {}",
            out.error_text()
        );
    }
    Ok(())
}

fn local_install(ctx: &Ctx, repo: &str, bin: &str) -> Result<()> {
    let dir = ctx.env.home.join(".local/bin");
    std::fs::create_dir_all(&dir)?;
    let from = Path::new(repo).join("target/release").join(bin);
    let to = dir.join(bin);
    let out = ctx.runner.run(
        &Cmd::new("cp", INSTALL_TIMEOUT)
            .arg(from.to_string_lossy().into_owned())
            .arg(to.to_string_lossy().into_owned()),
    )?;
    if !out.success() {
        bail!(
            "harness_install_failed: install {bin} into {}: {}",
            to.display(),
            out.error_text()
        );
    }
    Ok(())
}

fn print_version(ctx: &Ctx, bin: &str) -> Result<()> {
    let path = ctx.env.home.join(".local/bin").join(bin);
    let out = ctx
        .runner
        .run(&Cmd::new(path.to_string_lossy().into_owned(), VERSION_TIMEOUT).arg("--version"))?;
    if out.success() {
        println!("{}", out.stdout.trim());
    }
    Ok(())
}

fn box_build(ctx: &Ctx, target: &str, box_path: &str, kind: Kind) -> Result<()> {
    let mut env = format!(
        "PATH=/bin:$HOME/.cargo/bin:$HOME/.local/bin:/usr/local/bin:/usr/bin:/bin DEVELOPER_DIR={DEVELOPER_DIR}"
    );
    if kind == Kind::Fork {
        env.push_str(&format!(" ZIG={}/.target/rebase/zig-0.16.0/zig", box_path));
    }
    let mut installs = String::new();
    for bin in kind.binaries() {
        installs.push_str(&format!("\ncp target/release/{bin} $HOME/.local/bin/{bin}"));
    }
    let script = format!(
        "set -e\n\
         cd {path}\n\
         git fetch --quiet\n\
         git merge --ff-only @{{u}}\n\
         {env} cargo build --release --locked\n\
         mkdir -p $HOME/.local/bin{installs}",
        path = remote::quote(box_path),
    );
    let out = remote::ssh(ctx.runner, target, &script, None, BOX_BUILD_TIMEOUT)?;
    if !out.success() {
        bail!(
            "harness_box_failed: build and install on {target} in {box_path}: {}",
            out.error_text()
        );
    }
    Ok(())
}

/// The machine-wide install lock: two projects never install at once.
pub struct InstallLock {
    _file: std::fs::File,
}

pub fn lock(config_dir: &Path) -> Result<InstallLock> {
    std::fs::create_dir_all(config_dir)?;
    let path = config_dir.join(".harness-install.lock");
    let file = std::fs::File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    match file.try_lock() {
        Ok(()) => Ok(InstallLock { _file: file }),
        Err(std::fs::TryLockError::WouldBlock) => {
            bail!("harness_install_busy: another harness install is running")
        }
        Err(std::fs::TryLockError::Error(error)) => {
            Err(error).with_context(|| format!("could not lock {}", path.display()))
        }
    }
}

/// `ha harness install`: build every harness repository after a merge and
/// install it into `~/.local/bin`, then the same on the saved box.
pub fn install(ctx: &Ctx) -> Result<()> {
    let repos = repos(&ctx.config_dir)?;
    if repos.is_empty() {
        bail!(
            "harness_repos_missing: add `[harness] repos` to {}",
            ctx.config_dir.join("config.toml").display()
        );
    }
    let _lock = lock(&ctx.config_dir)?;
    let box_target = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        BOX_MACHINE,
    )
    .ok()
    .map(|profile| profile.target);
    let mut fork = false;
    for repo in &repos {
        let kind = kind(&repo.path)?;
        fork |= kind == Kind::Fork;
        local_build(ctx, &repo.path, kind)?;
        for bin in kind.binaries() {
            local_install(ctx, &repo.path, bin)?;
        }
        for bin in kind.binaries() {
            print_version(ctx, bin)?;
        }
        match (&box_target, &repo.box_path) {
            (Some(target), Some(box_path)) => box_build(ctx, target, box_path, kind)?,
            (Some(_), None) => {
                eprintln!("note: {} has no box_path; skipped the box step", repo.path)
            }
            (None, _) => {}
        }
    }
    if fork {
        println!("the running server keeps its image; a live handoff is Rolf's call");
    }
    Ok(())
}
