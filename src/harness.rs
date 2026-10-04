//! The harness repositories and `ha harness install` (t-0054).
//!
//! The list lives once, in `[harness]` in `config.toml`: each row is a
//! `path`/`box_path` pair, the same shape a project's `repos` rows have. Every
//! project may start lanes and review piles on a harness repository without
//! listing it in `PROJECT.md`.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths::Ctx;
use crate::project::{Repo, Settings};
use crate::remote;
use crate::runner::Cmd;

pub(crate) mod check;

/// The plugin build's tool path, exactly as the coordinator uses it by hand.
pub(crate) const DEVELOPER_DIR: &str = "/Library/Developer/CommandLineTools";

const BUILD_TIMEOUT: Duration = Duration::from_secs(1800);
const BOX_BUILD_TIMEOUT: Duration = Duration::from_secs(3600);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);
const PROCESS_WAIT: Duration = Duration::from_secs(5);
pub(crate) const BOX_WORKER_MARKER: &str = ".lane-worker";
const TICKER_AGENT: &str = "com.rolfie.herdr-ade-ticker";

pub(crate) fn ticker_agent_label() -> &'static str {
    TICKER_AGENT
}

fn ticker_agent_path(home: &Path) -> PathBuf {
    home.join("Library/LaunchAgents")
        .join(format!("{TICKER_AGENT}.plist"))
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn ticker_agent_definition(ctx: &Ctx) -> String {
    let bin = ctx.env.home.join(".local/bin/herdr-ade");
    let path = format!(
        "{}:/bin:{}",
        ctx.env.home.join(".local/bin").display(),
        ctx.env.var("PATH").unwrap_or_default()
    );
    let mut env = format!(
        "<key>PATH</key><string>{}</string><key>HERDR_ADE_TICKER_SUPERVISOR</key><string>launchd</string>",
        xml(&path)
    );
    for key in ["XDG_CONFIG_HOME", "HERDR_BIN_PATH"] {
        if let Some(value) = ctx.env.var(key) {
            env.push_str(&format!("<key>{key}</key><string>{}</string>", xml(value)));
        }
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\"><dict>\n<key>Label</key><string>{TICKER_AGENT}</string>\n<key>ProgramArguments</key><array><string>{}</string><string>--root</string><string>{}</string><string>ticker</string><string>ensure</string></array>\n<key>EnvironmentVariables</key><dict>{env}</dict>\n<key>RunAtLoad</key><true/>\n<key>StartInterval</key><integer>120</integer>\n</dict></plist>\n",
        xml(&bin.to_string_lossy()),
        xml(&ctx.root.to_string_lossy())
    )
}

unsafe extern "C" {
    fn getuid() -> u32;
}

fn ticker_agent_loaded(domain: &str) -> bool {
    std::process::Command::new("/bin/launchctl")
        .args(["print", &format!("{domain}/{TICKER_AGENT}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

pub(crate) fn ticker_supervisor_loaded() -> bool {
    ticker_agent_loaded(&format!("gui/{}", unsafe { getuid() }))
}

fn agent_bootout(loaded: bool, changed: bool) -> bool {
    loaded && changed
}

fn agent_bootstrap(loaded: bool, changed: bool) -> bool {
    !loaded || changed
}

fn install_ticker_agent(ctx: &Ctx) -> Result<()> {
    let path = ticker_agent_path(&ctx.env.home);
    let definition = ticker_agent_definition(ctx);
    let domain = format!("gui/{}", unsafe { getuid() });
    let loaded = ticker_agent_loaded(&domain);
    let changed = std::fs::read_to_string(&path).map_or(true, |old| old != definition);
    if changed {
        std::fs::create_dir_all(path.parent().context("agent path has no parent")?)?;
        crate::project::write_atomic(&path, definition.as_bytes())?;
    }
    if agent_bootout(loaded, changed) {
        let status = std::process::Command::new("/bin/launchctl")
            .args(["bootout", &format!("{domain}/{TICKER_AGENT}")])
            .status()?;
        if !status.success() {
            bail!("ticker supervisor bootout failed: {status}");
        }
    }
    if agent_bootstrap(loaded, changed) {
        let status = std::process::Command::new("/bin/launchctl")
            .args(["bootstrap", &domain, &path.to_string_lossy()])
            .status()?;
        if !status.success() {
            bail!("ticker supervisor bootstrap failed: {status}");
        }
    }
    Ok(())
}

#[derive(Debug, Default, Deserialize)]
struct HarnessConfig {
    #[serde(default)]
    repos: Vec<Repo>,
}

/// The harness repositories from `config.toml`. An absent table is an empty list.
pub(crate) fn repos(config_dir: &Path) -> Result<Vec<Repo>> {
    let document = crate::config::Document::read(config_dir)?;
    repos_from(&document)
}

pub(crate) fn repos_from(document: &crate::config::Document) -> Result<Vec<Repo>> {
    Ok(document.section::<HarnessConfig>("harness")?.repos)
}

/// A path's canonical form when it exists, else the path as given.
fn canonical_or(path: &str) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| PathBuf::from(path))
}

/// True when a project may start a lane or review a pile on `path`: the path is
/// one of its own listed repositories, or a harness repository.
pub(crate) fn allowed_repo(settings: &Settings, config_dir: &Path, path: &str) -> Result<bool> {
    let target = canonical_or(path);
    if settings
        .repos
        .iter()
        .any(|repo| canonical_or(&repo.path) == target)
    {
        return Ok(true);
    }
    Ok(repos(config_dir)?
        .iter()
        .any(|repo| canonical_or(&repo.path) == target))
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
            Kind::Plugin => &["herdr-ade", "herdr-pi", "herdr-rundown"],
            Kind::Fork => &["herdr"],
        }
    }

    fn name(self) -> &'static str {
        match self {
            Kind::Plugin => "plugin",
            Kind::Fork => "fork",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct InstalledBinary {
    pub(crate) name: String,
    pub(crate) path: String,
    pub(crate) version: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct InstalledRepo {
    pub(crate) path: String,
    pub(crate) kind: String,
    pub(crate) binaries: Vec<InstalledBinary>,
    pub(crate) commit: String,
    pub(crate) boxes: Vec<BoxRepoInstall>,
}

/// Evidence from the installed image, using the ticker's lock observation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum TickerProof {
    Running {
        pid: std::num::NonZeroU32,
        build: String,
    },
    Stale {
        pid: std::num::NonZeroU32,
        build: String,
    },
    NotRequired,
    Unknown {
        reason: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct Observation {
    pub(crate) binary: String,
    pub(crate) ticker: TickerProof,
}

impl Observation {
    fn verified(&self, expected: &str) -> bool {
        crate::build::same_commit(&self.binary, expected)
            && match &self.ticker {
                TickerProof::Running { build, .. } => crate::build::same_commit(build, expected),
                TickerProof::NotRequired => true,
                _ => false,
            }
    }
}

pub(crate) fn observation(root: &Path) -> Observation {
    let ticker = match crate::ticker::lock_state(root) {
        crate::ticker::LockState::Held(info) => match std::num::NonZeroU32::new(info.pid) {
            Some(pid) if !info.version.is_empty() => {
                if crate::build::same_commit(&info.version, crate::VERSION) {
                    TickerProof::Running {
                        pid,
                        build: info.version,
                    }
                } else {
                    TickerProof::Stale {
                        pid,
                        build: info.version,
                    }
                }
            }
            _ => TickerProof::Unknown {
                reason: "ticker lock has no complete build record".into(),
            },
        },
        crate::ticker::LockState::Free => {
            let (projects, errors) = crate::project::list_slugs_with_errors(root);
            if !errors.is_empty() {
                TickerProof::Unknown {
                    reason: errors
                        .iter()
                        .map(|error| format!("{error:#}"))
                        .collect::<Vec<_>>()
                        .join("; "),
                }
            } else if projects.is_empty() {
                TickerProof::NotRequired
            } else {
                TickerProof::Unknown {
                    reason: "ticker lock is free".into(),
                }
            }
        }
        crate::ticker::LockState::Unknown(reason) => TickerProof::Unknown { reason },
    };
    Observation {
        binary: format!("herdr-ade {}", crate::VERSION),
        ticker,
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub(crate) enum ProcessProof {
    Observed {
        machine: String,
        expected: String,
        observation: Observation,
    },
    Unknown {
        machine: String,
        reason: String,
    },
}

impl ProcessProof {
    fn verified(&self) -> bool {
        matches!(self, Self::Observed { expected, observation, .. } if observation.verified(expected))
    }

    fn running(&self) -> bool {
        self.verified()
            && matches!(
                self,
                Self::Observed {
                    observation: Observation {
                        ticker: TickerProof::Running { .. },
                        ..
                    },
                    ..
                }
            )
    }
}

impl std::fmt::Display for ProcessProof {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown { machine, reason } => write!(out, "{machine}: unknown ({reason})"),
            Self::Observed {
                machine,
                expected,
                observation,
            } => {
                let binary = if crate::build::same_commit(&observation.binary, expected) {
                    "installed"
                } else {
                    "stale"
                };
                writeln!(
                    out,
                    "{machine} binary: {} ({binary}; expected {expected})",
                    observation.binary
                )?;
                match &observation.ticker {
                    TickerProof::Running { pid, build } | TickerProof::Stale { pid, build } => {
                        let state = if matches!(&observation.ticker, TickerProof::Running { .. })
                            && crate::build::same_commit(build, expected)
                        {
                            "running"
                        } else {
                            "stale"
                        };
                        write!(out, "{machine} ticker pid {pid}: {build} ({state})")
                    }
                    TickerProof::NotRequired => {
                        write!(out, "{machine} ticker: not running (no projects)")
                    }
                    TickerProof::Unknown { reason } => {
                        write!(out, "{machine} ticker: unknown ({reason})")
                    }
                }
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(tag = "step", rename_all = "snake_case")]
pub(crate) enum InstallFailure {
    Profile { reason: String },
    Build { repo: String, reason: String },
    Settings { reason: String },
    Guard { reason: String },
}

impl std::fmt::Display for InstallFailure {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Build { repo, reason } => write!(out, "{repo}: {reason}"),
            Self::Profile { reason } | Self::Settings { reason } | Self::Guard { reason } => {
                out.write_str(reason)
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct BoxRepoInstall {
    pub(crate) machine: String,
    pub(crate) path: String,
    pub(crate) commit: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct BoxInstall {
    pub(crate) machine: String,
    pub(crate) target: String,
    pub(crate) settings_installed: bool,
    pub(crate) errors: Vec<InstallFailure>,
    pub(crate) process: ProcessProof,
}

impl BoxInstall {
    fn pending(&self) -> bool {
        // Unlike the local no-project exemption, a saved worker needs its
        // courier ticker even before its first project arrives.
        !self.settings_installed || !self.errors.is_empty() || !self.process.running()
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct InstallOutcome {
    pub(crate) repositories: Vec<InstalledRepo>,
    pub(crate) boxes: Vec<BoxInstall>,
    pub(crate) live_handoff_required: bool,
    pub(crate) processes: Vec<ProcessProof>,
    pub(crate) coordinator_hooks: Vec<String>,
    pub(crate) checks: check::InstallCheck,
}

impl InstallOutcome {
    pub(crate) fn box_failed(&self) -> bool {
        self.boxes.iter().any(BoxInstall::pending)
    }

    pub(crate) fn message(&self) -> String {
        let mut message = self
            .repositories
            .iter()
            .flat_map(|repo| repo.binaries.iter())
            .map(|binary| format!("{}\n", binary.version))
            .collect::<String>();
        for box_result in &self.boxes {
            message.push_str(&format!(
                "box {} ({}): {}\n",
                box_result.machine,
                box_result.target,
                if !box_result.pending() {
                    "installed"
                } else {
                    "pending"
                }
            ));
        }
        for hook in &self.coordinator_hooks {
            message.push_str(&format!("coordinator hook rebound: {hook}\n"));
        }
        for process in self
            .processes
            .iter()
            .chain(self.boxes.iter().map(|result| &result.process))
        {
            message.push_str(&format!("{process}\n"));
        }
        if self.live_handoff_required {
            message.push_str("the running server keeps its image; live handoff pending\n");
        }
        message.push_str(&format!("{}\n", self.checks.result));
        message
    }

    pub(crate) fn summary(&self) -> &str {
        &self.checks.result
    }

    pub(crate) fn warnings(&self) -> String {
        let mut warnings = String::new();
        for result in &self.boxes {
            for error in &result.errors {
                warnings.push_str(&format!("box pending: {}: {error}\n", result.machine));
            }
            if !result.process.running() {
                warnings.push_str(&format!("box pending: {}\n", result.process));
            }
        }
        warnings
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

fn local_build(ctx: &Ctx, repo: &str, kind: Kind, source_clean: bool) -> Result<()> {
    // Cargo does not notice a dirty-to-clean transition caused only by Git
    // excludes. Invalidate the cached stamp without narrowing build.rs's inputs.
    if source_clean
        && kind == Kind::Plugin
        && binary_version(ctx, &Path::new(repo).join("target/release/herdr-ade"))
            .is_some_and(|version| version.contains("-dirty"))
    {
        let out = ctx
            .runner
            .run(&Cmd::new("touch", INSTALL_TIMEOUT).arg("build.rs").cwd(repo))?;
        if !out.success() {
            bail!(
                "harness_build_failed: refresh build stamp in {repo}: {}",
                out.error_text()
            );
        }
    }
    let mut cmd = Cmd::new("cargo", BUILD_TIMEOUT)
        .args(["build", "--release", "--locked"])
        .cwd(repo)
        .own_group()
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

fn binary_version(ctx: &Ctx, path: &Path) -> Option<String> {
    let out = ctx
        .runner
        .run(&Cmd::new(path.to_string_lossy().into_owned(), VERSION_TIMEOUT).arg("--version"))
        .ok()?;
    out.success()
        .then(|| out.stdout.trim().to_string())
        .filter(|version| !version.is_empty())
}

fn install_record(dir: &Path, bin: &str) -> PathBuf {
    dir.join(format!(".{bin}.installed-commit"))
}

fn record_installed_commit(dir: &Path, bin: &str, commit: &str) -> Result<()> {
    let record = install_record(dir, bin);
    let staged = dir.join(format!(".{bin}.installed-commit-{}", std::process::id()));
    std::fs::write(&staged, format!("{commit}\n"))?;
    std::fs::rename(&staged, &record)?;
    Ok(())
}

fn local_install(ctx: &Ctx, repo: &str, bin: &str, commit: &str, source_clean: bool) -> Result<()> {
    let dir = ctx.env.home.join(".local/bin");
    std::fs::create_dir_all(&dir)?;
    let from = Path::new(repo).join("target/release").join(bin);
    let to = dir.join(bin);
    let versions = binary_version(ctx, &from).zip(binary_version(ctx, &to));
    let versions_name_commits = versions.as_ref().is_some_and(|(source, installed)| {
        crate::build::commit_version(source).is_some()
            && crate::build::commit_version(installed).is_some()
    });
    let reported_same = versions
        .as_ref()
        .is_some_and(|(source, installed)| crate::build::same_commit(source, installed));
    let recorded_same = versions.is_some()
        && std::fs::read_to_string(install_record(&dir, bin))
            .is_ok_and(|installed| installed.trim() == commit);
    if source_clean && (reported_same || (!versions_name_commits && recorded_same)) {
        record_installed_commit(&dir, bin, commit)?;
        return Ok(());
    }
    let staged = dir.join(format!(".{bin}.install-{}", std::process::id()));
    let copy = ctx.runner.run(
        &Cmd::new("cp", INSTALL_TIMEOUT)
            .arg(from.to_string_lossy().into_owned())
            .arg(staged.to_string_lossy().into_owned()),
    )?;
    if !copy.success() {
        bail!(
            "harness_install_failed: stage {bin} for {}: {}",
            to.display(),
            copy.error_text()
        );
    }
    let moved = match ctx.runner.run(
        &Cmd::new("mv", INSTALL_TIMEOUT)
            .args(["-f"])
            .arg(staged.to_string_lossy().into_owned())
            .arg(to.to_string_lossy().into_owned()),
    ) {
        Ok(output) => output,
        Err(error) => {
            let _ = std::fs::remove_file(&staged);
            return Err(error).context(format!(
                "harness_install_failed: could not replace {}",
                to.display()
            ));
        }
    };
    if !moved.success() {
        let _ = std::fs::remove_file(&staged);
        bail!(
            "harness_install_failed: install {bin} into {}: {}",
            to.display(),
            moved.error_text()
        );
    }
    if source_clean {
        record_installed_commit(&dir, bin, commit)?;
    } else {
        let _ = std::fs::remove_file(install_record(&dir, bin));
    }
    Ok(())
}

/// Hard links retain the exact previous images and install stamps while the
/// installer atomically replaces their public paths.
struct PreviousBinaries {
    dir: PathBuf,
    saved: Vec<(PathBuf, Option<PathBuf>)>,
}

impl PreviousBinaries {
    fn new(home: &Path) -> Result<Self> {
        let dir = home
            .join(".local/bin")
            .join(format!(".harness-previous-{}", std::process::id()));
        std::fs::create_dir_all(dir.parent().context("binary folder missing")?)?;
        std::fs::create_dir(&dir)?;
        Ok(Self {
            dir,
            saved: Vec::new(),
        })
    }

    fn remember(&mut self, home: &Path, bin: &str) -> Result<()> {
        let dir = home.join(".local/bin");
        for path in [dir.join(bin), install_record(&dir, bin)] {
            let backup = self
                .dir
                .join(path.file_name().context("binary name missing")?);
            let saved = if path.exists() {
                std::fs::hard_link(&path, &backup)?;
                Some(backup)
            } else {
                None
            };
            self.saved.push((path, saved));
        }
        Ok(())
    }

    fn changed(&self, home: &Path, bin: &str) -> Result<bool> {
        let target = home.join(".local/bin").join(bin);
        match self.saved.iter().find(|(path, _)| path == &target) {
            Some((_, Some(backup))) => Ok(std::fs::read(backup)? != std::fs::read(target)?),
            Some((_, None)) => Ok(target.exists()),
            None => Ok(false),
        }
    }

    fn restore(&self) -> Result<()> {
        for (path, saved) in &self.saved {
            if let Some(saved) = saved {
                std::fs::rename(saved, path)?;
            } else if path.exists() {
                std::fs::remove_file(path)?;
            }
        }
        Ok(())
    }

    fn discard(self) -> Result<()> {
        std::fs::remove_dir_all(self.dir)?;
        Ok(())
    }
}

fn installed_snapshot(ctx: &Ctx) -> Result<Vec<check::ProjectCheck>> {
    let bin = ctx.env.home.join(".local/bin/herdr-ade");
    let output = ctx
        .runner
        .run(&Cmd::new(bin.to_string_lossy(), INSTALL_TIMEOUT).args([
            "--root",
            &ctx.root.to_string_lossy(),
            "install-check",
        ]))?;
    if !output.success() {
        bail!("installed binary could not load records");
    }
    serde_json::from_str(&output.stdout).context("installed binary did not return install counts")
}

fn rollback<T>(
    ctx: &Ctx,
    previous: PreviousBinaries,
    checks: &mut check::InstallCheck,
    reason: &str,
) -> Result<T> {
    checks.result = format!("{reason}; rollback on mac pending, boxes untouched");
    checks.record(&ctx.config_dir)?;
    previous.restore()?;
    let bin = ctx.env.home.join(".local/bin/herdr-ade");
    if ctx.detached_ticker && bin.exists() {
        let output = ctx.runner.run(
            &Cmd::new(bin.to_string_lossy(), INSTALL_TIMEOUT)
                .env("HERDR_ADE_INSTALL_TICKER", "1")
                .args(["--root", &ctx.root.to_string_lossy(), "ticker", "start"]),
        )?;
        if !output.success() {
            checks.result = format!(
                "{reason}; binaries restored on mac, ticker restart failed, boxes untouched"
            );
            checks.record(&ctx.config_dir)?;
            bail!("{}", checks.result);
        }
    }
    previous.discard()?;
    checks.result = format!("{reason}; rolled back on mac, boxes untouched");
    checks.record(&ctx.config_dir)?;
    bail!("{}", checks.result)
}

/// Stage the whole mod, then publish it in one filesystem operation. Mac's
/// RENAME_SWAP can replace a nonempty directory without a missing-path window.
#[cfg(target_os = "macos")]
fn install_coordinator_handoff(home: &Path, repo: &Path) -> Result<()> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;

    unsafe extern "C" {
        fn renamex_np(
            from: *const std::ffi::c_char,
            to: *const std::ffi::c_char,
            flags: u32,
        ) -> i32;
    }

    fn copy_folder(from: &Path, to: &Path) -> Result<()> {
        std::fs::create_dir(to)?;
        for entry in std::fs::read_dir(from)? {
            let entry = entry?;
            let target = to.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_folder(&entry.path(), &target)?;
            } else {
                std::fs::copy(entry.path(), target)?;
            }
        }
        Ok(())
    }

    let mods = home.join(".local/share/herdr-ade/mods");
    std::fs::create_dir_all(&mods)?;
    let target = mods.join("coordinator-handoff");
    let staged = mods.join(format!(
        ".coordinator-handoff-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let result = (|| -> Result<()> {
        copy_folder(&repo.join("mods/coordinator-handoff"), &staged)?;
        if target.exists() {
            let from = CString::new(staged.as_os_str().as_bytes())?;
            let to = CString::new(target.as_os_str().as_bytes())?;
            // Both strings remain alive and NUL-terminated for this call.
            if unsafe { renamex_np(from.as_ptr(), to.as_ptr(), 0x00000002) } != 0 {
                return Err(std::io::Error::last_os_error().into());
            }
        } else {
            std::fs::rename(&staged, &target)?;
        }
        Ok(())
    })();
    // After a swap, staged holds the previous installation, not the new one.
    if staged.exists() {
        std::fs::remove_dir_all(&staged)?;
    }
    result.context("harness_install_failed: coordinator handoff mod")
}

fn installed_version(ctx: &Ctx, bin: &str) -> Result<InstalledBinary> {
    let path = ctx.env.home.join(".local/bin").join(bin);
    let out = ctx
        .runner
        .run(&Cmd::new(path.to_string_lossy().into_owned(), VERSION_TIMEOUT).arg("--version"))?;
    if !out.success() {
        bail!(
            "harness_version_failed: {} --version: {}",
            path.display(),
            out.error_text()
        );
    }
    Ok(InstalledBinary {
        name: bin.to_string(),
        path: path.to_string_lossy().into_owned(),
        version: out.stdout.trim().to_string(),
    })
}

/// Resolve the box's zig on the box for a fork build, as a shell snippet.
///
/// The repository-local `<box_path>/.target/rebase/zig-0.16.0/zig` wins when it
/// exists; otherwise a `zig` on PATH. With neither, or with a version other
/// than 0.16.0, the snippet prints `harness_box_zig_*` on stderr and exits
/// nonzero, so `box_build` reports it on the `harness_box_failed` line. The Mac
/// side never runs this: `zig_path` stays the Mac-shaped path there.
fn box_zig_script(box_path: &str) -> String {
    format!(
        "herdr_repo_zig={repo}/.target/rebase/zig-0.16.0/zig\n\
         if [ -x \"$herdr_repo_zig\" ]; then\n\
         ZIG=\"$herdr_repo_zig\"\n\
         elif command -v zig >/dev/null 2>&1; then\n\
         ZIG=\"$(command -v zig)\"\n\
         else\n\
         echo \"harness_box_zig_missing: no zig found: neither $herdr_repo_zig nor a zig on PATH\" >&2\n\
         exit 1\n\
         fi\n\
         herdr_zig_version=\"$(\"$ZIG\" version)\"\n\
         if [ \"$herdr_zig_version\" != \"0.16.0\" ]; then\n\
         echo \"harness_box_zig_version: $ZIG reports zig $herdr_zig_version, not 0.16.0\" >&2\n\
         exit 1\n\
         fi\n\
         export ZIG",
        repo = remote::quote(box_path),
    )
}

fn box_binary(machine: &crate::remote::MachineDeclaration, bin: &str) -> Result<String> {
    match bin {
        "herdr-ade" => Ok(machine.ade_bin.clone()),
        "herdr-pi" => Ok(machine.pi_bin.clone()),
        "herdr-rundown" => Path::new(&machine.ade_bin)
            .parent()
            .map(|dir| dir.join(bin).to_string_lossy().into_owned())
            .context("machine ade_bin has no parent folder"),
        "herdr" => Path::new(&machine.ade_bin)
            .parent()
            .map(|dir| dir.join(bin).to_string_lossy().into_owned())
            .context("machine ade_bin has no parent folder"),
        _ => bail!("harness binary `{bin}` has no machine destination"),
    }
}

fn box_build(
    ctx: &Ctx,
    target: &str,
    machine: &crate::remote::MachineDeclaration,
    box_path: &str,
    kind: Kind,
    expected_commit: &str,
) -> Result<String> {
    let zig = if kind == Kind::Fork {
        format!("\n{}", box_zig_script(box_path))
    } else {
        String::new()
    };
    let refresh_stamp = if kind == Kind::Plugin {
        "\nif [ -z \"$source_dirty\" ] && [ -x target/release/herdr-ade ]; then\n\
         case \"$(target/release/herdr-ade --version)\" in\n\
         *-dirty*) touch build.rs ;;\n\
         esac\n\
         fi"
    } else {
        ""
    };
    let mut installs = String::new();
    for bin in kind.binaries() {
        // A previous clean install may already have recorded this commit while
        // copying a stale dirty binary. Its record alone cannot justify a skip.
        let installed_stamp = if kind == Kind::Plugin {
            "\ncase \"$(\"$install_to\" --version 2>/dev/null || :)\" in\n\
             *-dirty*) installed_dirty=dirty ;;\n\
             *) installed_dirty= ;;\n\
             esac"
        } else {
            "\ninstalled_dirty="
        };
        installs.push_str(&format!(
            "\ninstall_to={to}\n\
             install_record=\"$(dirname \"$install_to\")/.{bin}.installed-commit\"\n\
             install_tmp=\"${{install_to}}.install.$$\"\n\
             record_tmp=\"${{install_record}}.$$\"\n\
             mkdir -p \"$(dirname \"$install_to\")\"{installed_stamp}\n\
             if [ -z \"$source_dirty\" ] && [ -z \"$installed_dirty\" ] && [ -x \"$install_to\" ] && [ \"$(cat \"$install_record\" 2>/dev/null || :)\" = \"$source_head\" ]; then\n\
               :\n\
             else\n\
               cp target/release/{bin} \"$install_tmp\"\n\
               chmod 755 \"$install_tmp\"\n\
               mv -f \"$install_tmp\" \"$install_to\"\n\
               if [ -z \"$source_dirty\" ]; then\n\
                 printf '%s\\n' \"$source_head\" > \"$record_tmp\"\n\
                 mv -f \"$record_tmp\" \"$install_record\"\n\
               else\n\
                 rm -f \"$install_record\"\n\
               fi\n\
             fi",
            to = remote::quote(&box_binary(machine, bin)?),
        ));
    }
    let script = format!(
        "set -e\n\
         {{\n\
         cd {path}\n\
         git fetch --quiet\n\
         expected_commit={expected_commit}\n\
         if ! git merge-base --is-ancestor \"$expected_commit\" @{{u}}; then\n\
           printf 'integration not published: upstream in %s does not contain %s\\n' {path} \"$expected_commit\" >&2\n\
           exit 1\n\
         fi\n\
         git merge --ff-only @{{u}}\n\
         # The box clone's files arrive by sync, but its index does not.\n\
         git read-tree HEAD && git update-index -q --refresh\n\
         source_head=\"$(git rev-parse HEAD)\"\n\
         source_dirty=\"$(git status --porcelain --untracked-files=normal)\"\n\
         export PATH={build_path}\n\
         export DEVELOPER_DIR={DEVELOPER_DIR}{zig}{refresh_stamp}\n\
         cargo build --release --locked{installs}\n\
         }} >&2\n\
         printf '%s\\n' \"$source_head\"",
        path = remote::quote(box_path),
        expected_commit = remote::quote(expected_commit),
        build_path = remote::quote(&machine.path),
    );
    let out = remote::ssh(ctx.runner, target, &script, None, BOX_BUILD_TIMEOUT)?;
    if !out.success() {
        bail!(
            "harness_box_failed: build and install on {target} in {box_path}: {}",
            out.error_text()
        );
    }
    let head = out.stdout.trim();
    if head.is_empty() || !head.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("box did not report a valid installed commit: {head}");
    }
    Ok(head.to_string())
}

/// Install only the policy a lane machine consumes. Dispatch recipes and the
/// routing rubric stay on the coordinator; copying them would give the box a
/// second, stale source of model-selection policy.
fn box_settings(
    ctx: &Ctx,
    target: &str,
    machine: &crate::remote::MachineDeclaration,
) -> Result<()> {
    let rules_path = ctx.config_dir.join("RULES.md");
    let rules = std::fs::read_to_string(&rules_path)
        .with_context(|| format!("harness_rules_missing: {}", rules_path.display()))?;
    let script = format!(
        "set -e\n\
         dir={config_dir}\n\
         mkdir -p \"$dir\"\n\
         tmp=\"$dir/.RULES.md.install.$$\"\n\
         cat > \"$tmp\"\n\
         chmod 600 \"$tmp\"\n\
         mv -f \"$tmp\" \"$dir/RULES.md\"\n\
         printf '%s\\n' 'lane worker; dispatch stays on the coordinator' > \"$dir/{BOX_WORKER_MARKER}\"\n\
         chmod 600 \"$dir/{BOX_WORKER_MARKER}\"",
        config_dir = remote::quote(&format!("{}/.config/herdr-ade", machine.home)),
    );
    let out = remote::ssh(ctx.runner, target, &script, Some(&rules), INSTALL_TIMEOUT)?;
    if !out.success() {
        bail!(
            "harness_box_settings_failed: install lane policy on {target}: {}",
            out.error_text()
        );
    }
    Ok(())
}

/// The machine-wide install lock: two projects never install at once.
pub(crate) struct InstallLock {
    _file: std::fs::File,
}

/// Ordinary commands must not race the installer's ticker replacement.
pub(crate) fn install_in_progress(config_dir: &Path) -> bool {
    let Ok(file) = std::fs::File::options()
        .write(true)
        .open(config_dir.join(".harness-install.lock"))
    else {
        return false;
    };
    file.try_lock().is_err()
}

pub(crate) fn lock(config_dir: &Path) -> Result<InstallLock> {
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

fn repo_head(ctx: &Ctx, repo: &str) -> Result<String> {
    let head = crate::repo::Git::new(ctx.runner, repo)
        .with_timeout(VERSION_TIMEOUT)
        .run(&["rev-parse", "HEAD"])
        .with_context(|| format!("harness_build_head: could not read HEAD in {repo}"))?;
    if head.is_empty() {
        bail!("harness_build_head: empty HEAD in {repo}");
    }
    Ok(head)
}

fn repo_clean(ctx: &Ctx, repo: &str) -> Result<bool> {
    let status = crate::repo::Git::new(ctx.runner, repo)
        .with_timeout(VERSION_TIMEOUT)
        .stdout(&["status", "--porcelain", "--untracked-files=normal"])
        .with_context(|| format!("harness_build_status: could not inspect {repo}"))?;
    Ok(status.is_empty())
}

fn local_process_proofs(ctx: &Ctx, plugin_version: Option<&str>) -> Result<Vec<ProcessProof>> {
    if !crate::project::list_slugs(&ctx.root).is_empty() && ctx.detached_ticker {
        let bin = ctx.env.home.join(".local/bin/herdr-ade");
        // A ticker can itself be landing the review. It cannot wait for
        // its own lock to release. Let the installed image request the
        // stop asynchronously; its next pass will finish this install.
        if plugin_version.is_some_and(|version| {
            matches!(crate::ticker::lock_state(&ctx.root), crate::ticker::LockState::Held(info)
                if info.pid == std::process::id() && !crate::build::same_commit(&info.version, version))
        }) {
            std::process::Command::new(&bin)
                .args(["--root", &ctx.root.to_string_lossy(), "ticker", "start"])
                .env("HERDR_ADE_INSTALL_TICKER", "1")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .context("harness_ticker_pending: could not launch installed ticker")?;
            bail!("harness_ticker_pending: installed ticker is replacing this ticker; landing will resume on its next pass");
        }
        let output = ctx.runner.run(
            &Cmd::new(bin.to_string_lossy(), INSTALL_TIMEOUT)
                .env("HERDR_ADE_INSTALL_TICKER", "1")
                .args(["--root", &ctx.root.to_string_lossy(), "ticker", "start"]),
        )?;
        if !output.success() {
            bail!(
                "harness_ticker_failed: new ticker start: {}",
                output.error_text()
            );
        }
    }
    let bin = ctx.env.home.join(".local/bin/herdr-ade");
    let deadline = Instant::now() + PROCESS_WAIT;
    loop {
        let proof = process_proof(
            "local",
            plugin_version.unwrap_or(crate::VERSION),
            ctx.runner
                .run(&Cmd::new(bin.to_string_lossy(), VERSION_TIMEOUT).args([
                    "--root",
                    &ctx.root.to_string_lossy(),
                    "--json",
                    "ticker",
                    "status",
                ])),
        );
        if proof.verified() {
            return Ok(vec![proof]);
        }
        if Instant::now() >= deadline {
            bail!("harness_ticker_failed: {proof}");
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn box_process_script(machine: &crate::remote::MachineDeclaration) -> String {
    remote::with_path(
        &machine.path,
        &format!(
            "HERDR_ADE_INSTALL_TICKER=1 {bin} --root {root} ticker start >&2 || :\n{bin} --root {root} --json ticker status",
            bin = remote::quote(&machine.ade_bin),
            root = remote::quote(&machine.root),
        ),
    )
}

fn process_proof(
    machine: &str,
    expected: &str,
    output: Result<crate::runner::Output>,
) -> ProcessProof {
    #[derive(Deserialize)]
    struct Receipt {
        data: Data,
    }
    #[derive(Deserialize)]
    struct Data {
        observation: Observation,
    }
    let result = output.and_then(|out| {
        if !out.success() {
            bail!("{}", out.error_text());
        }
        let receipt: Receipt = serde_json::from_str(&out.stdout)?;
        Ok(receipt.data.observation)
    });
    match result {
        Ok(observation) => ProcessProof::Observed {
            machine: machine.into(),
            expected: expected.into(),
            observation,
        },
        Err(error) => ProcessProof::Unknown {
            machine: machine.into(),
            reason: format!("{error:#}"),
        },
    }
}

fn box_process_proofs(
    ctx: &Ctx,
    machine: &crate::remote::MachineDeclaration,
    expected: &str,
) -> ProcessProof {
    process_proof(
        &machine.id,
        expected,
        remote::ssh(
            ctx.runner,
            &machine.target,
            &box_process_script(machine),
            None,
            Duration::from_secs(140),
        ),
    )
}

fn refresh_local_guard(ctx: &Ctx) -> Result<()> {
    let binary = ctx.env.home.join(".local/bin/herdr-pi");
    let output = ctx.runner.run(
        &Cmd::new(binary.display().to_string(), Duration::from_secs(30))
            .arg("refresh-guard")
            .env("HERDR_ADE_ROOT", ctx.root.display().to_string()),
    )?;
    if !output.success() {
        bail!(
            "harness_local_guard_failed: could not refresh the installed pi guard: {}",
            output.error_text()
        );
    }
    Ok(())
}

fn refresh_box_guard(ctx: &Ctx, target: &str, machine: &remote::MachineDeclaration) -> Result<()> {
    let command = format!(
        "HERDR_ADE_ROOT={} {} refresh-guard",
        remote::quote(&machine.root),
        remote::quote(&machine.pi_bin)
    );
    let script = remote::with_path(&machine.path, &command);
    let output = remote::ssh(ctx.runner, target, &script, None, Duration::from_secs(30))?;
    if !output.success() {
        bail!(
            "harness_box_guard_failed: could not refresh the pi guard on {target}: {}",
            output.error_text()
        );
    }
    Ok(())
}

fn install_box(
    ctx: &Ctx,
    label: &str,
) -> Result<(crate::contracts::MachineProfile, remote::MachineDeclaration)> {
    let declaration = remote::machine_declaration(&ctx.config_dir, label)?;
    let profile = remote::machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        &declaration.label,
    )?;
    Ok((profile, declaration))
}

fn box_repo_path(machine: &remote::MachineDeclaration, repo: &Repo) -> Option<String> {
    machine
        .repos
        .iter()
        .find(|row| row.path == repo.path)
        .and_then(|row| row.box_path.clone())
        .or_else(|| repo.box_path.clone())
}

/// `ha harness install`: build every harness repository after a merge and
/// install it into `~/.local/bin`, then the same on the saved box.
pub(crate) fn install(ctx: &Ctx) -> Result<InstallOutcome> {
    install_for(ctx, None)
}

pub(crate) fn install_for_review(ctx: &Ctx, slug: &str, id: &str) -> Result<InstallOutcome> {
    install_for(ctx, Some((slug, id)))
}

fn review_precedes(left: (&str, &str), right: (&str, &str)) -> bool {
    let number = |id: &str| id.rsplit_once('-')?.1.parse::<u64>().ok();
    left.0
        .cmp(right.0)
        .then_with(|| match (number(left.1), number(right.1)) {
            (Some(left), Some(right)) => left.cmp(&right),
            _ => left.1.cmp(right.1),
        })
        .is_lt()
}

fn defer_to_earlier_reviews(ctx: &Ctx, current: Option<(&str, &str)>) -> Result<()> {
    let mut landing = Vec::new();
    for slug in crate::project::list_slugs(&ctx.root) {
        let project = crate::project::Project::load(&ctx.root, &slug)?;
        for review in crate::review::list(&project)? {
            // Review installs run in project/id order. Never block the ticker
            // waiting for another review that it must itself advance.
            if review.phase == crate::review::Phase::Landing
                && current.is_none_or(|id| review_precedes((&slug, &review.id), id))
            {
                landing.push(format!("{slug}/{}", review.id));
            }
        }
    }
    if !landing.is_empty() {
        bail!(
            "harness_review_landing: earlier reviews still landing: {}",
            landing.join(", ")
        );
    }
    Ok(())
}

fn install_for(ctx: &Ctx, current: Option<(&str, &str)>) -> Result<InstallOutcome> {
    let repos = repos(&ctx.config_dir)?;
    if repos.is_empty() {
        bail!(
            "harness_repos_missing: add `[harness] repos` to {}",
            ctx.config_dir.join("config.toml").display()
        );
    }
    defer_to_earlier_reviews(ctx, current)?;
    let _lock = lock(&ctx.config_dir)?;
    let mut fork = false;
    let mut kinds = Vec::new();
    let mut installed = Vec::new();
    let mut checks = check::InstallCheck {
        before: check::snapshot(&ctx.root),
        after: Vec::new(),
        result: "install pending".into(),
    };
    checks.record(&ctx.config_dir)?;
    let mut previous = PreviousBinaries::new(&ctx.env.home)?;
    let local_result = (|| -> Result<()> {
        for repo in &repos {
            let kind = kind(&repo.path)?;
            fork |= kind == Kind::Fork;
            let commit = repo_head(ctx, &repo.path)?;
            let clean_before = repo_clean(ctx, &repo.path)?;
            local_build(ctx, &repo.path, kind, clean_before)?;
            let after_build = repo_head(ctx, &repo.path)?;
            let source_clean = clean_before && repo_clean(ctx, &repo.path)?;
            if after_build != commit {
                bail!(
                    "harness_build_changed: {} moved from {commit} to {after_build} while it was building",
                    repo.path
                );
            }
            for bin in kind.binaries() {
                previous.remember(&ctx.env.home, bin)?;
                local_install(ctx, &repo.path, bin, &commit, source_clean)?;
            }
            let mut binaries = Vec::new();
            for bin in kind.binaries() {
                binaries.push(installed_version(ctx, bin)?);
            }
            kinds.push(kind);
            installed.push(InstalledRepo {
                path: repo.path.clone(),
                kind: kind.name().into(),
                binaries,
                commit,
                boxes: Vec::new(),
            });
        }
        Ok(())
    })();
    if let Err(error) = local_result {
        return rollback(
            ctx,
            previous,
            &mut checks,
            &format!("install failed on mac: {error:#}"),
        );
    }
    match installed_snapshot(ctx) {
        Ok(after) => checks.after = after,
        Err(_) => {
            return rollback(
                ctx,
                previous,
                &mut checks,
                "REGRESSION: installed binary could not check records",
            );
        }
    }
    if let Some(reason) = check::regression(&checks.before, &checks.after) {
        return rollback(ctx, previous, &mut checks, &reason);
    }
    let rundown_changed = previous.changed(&ctx.env.home, "herdr-rundown")?;
    previous.discard()?;
    checks.result = format!("installed on mac; {}", checks.summary());
    checks.record(&ctx.config_dir)?;
    #[cfg(target_os = "macos")]
    for (repo, kind) in repos.iter().zip(&kinds) {
        if *kind == Kind::Plugin {
            install_coordinator_handoff(&ctx.env.home, Path::new(&repo.path))?;
        }
    }
    if rundown_changed {
        crate::rundown::reopen_existing(ctx)?;
    }
    if kinds.contains(&Kind::Plugin) {
        refresh_local_guard(ctx)?;
        if cfg!(target_os = "macos") && ctx.detached_ticker {
            install_ticker_agent(ctx)?;
        }
    }
    let plugin_version = installed
        .iter()
        .flat_map(|repo| &repo.binaries)
        .find(|binary| binary.name == "herdr-ade")
        .map(|binary| binary.version.clone());
    let processes = local_process_proofs(ctx, plugin_version.as_deref())?;

    // Finish with this invocation's image even if its on-disk binary changed.
    let mut boxes = Vec::new();
    for label in remote::declared_machine_labels(&ctx.config_dir)? {
        let mut result = BoxInstall {
            machine: label.clone(),
            target: String::new(),
            settings_installed: false,
            errors: Vec::new(),
            process: ProcessProof::Unknown {
                machine: label.clone(),
                reason: "machine not reached".into(),
            },
        };
        match install_box(ctx, &label) {
            Ok((profile, mut machine)) => {
                let label = profile.label.clone();
                result.machine = label.clone();
                result.target = profile.target.clone();
                machine.target = profile.target.clone();
                machine.id = profile.id.clone();
                let mut box_plugin_installed = false;
                for ((repo, kind), installed_repo) in repos.iter().zip(&kinds).zip(&mut installed) {
                    if let Some(box_path) = box_repo_path(&machine, repo) {
                        match box_build(
                            ctx,
                            &profile.target,
                            &machine,
                            &box_path,
                            *kind,
                            &installed_repo.commit,
                        ) {
                            Ok(head) => {
                                installed_repo.boxes.push(BoxRepoInstall {
                                    machine: label.clone(),
                                    path: box_path,
                                    commit: head,
                                });
                                if *kind == Kind::Plugin {
                                    box_plugin_installed = true;
                                }
                            }
                            Err(error) => {
                                result.errors.push(InstallFailure::Build {
                                    repo: repo.path.clone(),
                                    reason: format!("{error:#}"),
                                });
                            }
                        }
                    } else {
                        result.errors.push(InstallFailure::Build {
                            repo: repo.path.clone(),
                            reason: format!("no box_path on {label}"),
                        });
                    }
                }
                match box_settings(ctx, &profile.target, &machine) {
                    Ok(()) => result.settings_installed = true,
                    Err(error) => result.errors.push(InstallFailure::Settings {
                        reason: format!("{error:#}"),
                    }),
                }
                if box_plugin_installed
                    && let Err(error) = refresh_box_guard(ctx, &profile.target, &machine)
                {
                    result.errors.push(InstallFailure::Guard {
                        reason: format!("{error:#}"),
                    });
                }
                result.process = box_process_proofs(
                    ctx,
                    &machine,
                    plugin_version.as_deref().unwrap_or(crate::VERSION),
                );
            }
            Err(error) => result.errors.push(InstallFailure::Profile {
                reason: format!("{error:#}"),
            }),
        }
        boxes.push(result);
    }
    let coordinator_hooks = crate::hook::reinstall_open(ctx)?;
    let mut machines = vec!["mac".to_string()];
    machines.extend(
        boxes
            .iter()
            .filter(|result| !result.pending())
            .map(|result| result.machine.clone()),
    );
    checks.result = format!("installed on {}; {}", machines.join(", "), checks.summary());
    if boxes.iter().any(BoxInstall::pending) {
        checks.result.push_str("; boxes pending");
    }
    checks
        .result
        .push_str("; ticker first full pass pending; journey pending");
    if let Err(error) = crate::journey::after_install(ctx, current) {
        checks
            .result
            .push_str(&format!("; post-install checks FAIL: {error:#}"));
        if let Err(delivery) = crate::journey::launch_failed(ctx, current, &error) {
            checks
                .result
                .push_str(&format!("; journey notice pending: {delivery:#}"));
        }
    }
    checks.record(&ctx.config_dir)?;
    Ok(InstallOutcome {
        repositories: installed,
        boxes,
        live_handoff_required: fork,
        processes,
        coordinator_hooks,
        checks,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::{FakeRunner, fail, ok};
    use crate::runner::{RealRunner, Runner};
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    #[cfg(target_os = "macos")]
    #[test]
    fn coordinator_mod_install_replaces_the_whole_folder_atomically() {
        let home = tempfile::tempdir().unwrap();
        let repo = Path::new(env!("CARGO_MANIFEST_DIR"));
        let target = home
            .path()
            .join(".local/share/herdr-ade/mods/coordinator-handoff");
        install_coordinator_handoff(home.path(), repo).unwrap();
        let manifest = target.join(".claude-plugin/plugin.json");
        let original = std::fs::read(&manifest).unwrap();
        assert_eq!(
            original,
            std::fs::read(repo.join("mods/coordinator-handoff/.claude-plugin/plugin.json"))
                .unwrap()
        );
        std::fs::write(&manifest, "old manifest").unwrap();
        std::fs::write(target.join("removed-hook.ts"), "obsolete").unwrap();
        install_coordinator_handoff(home.path(), repo).unwrap();
        assert_eq!(std::fs::read(&manifest).unwrap(), original);
        assert!(!target.join("removed-hook.ts").exists());
        assert!(target.join("hooks/register.ts").exists());
        assert_eq!(
            std::fs::read_dir(target.parent().unwrap()).unwrap().count(),
            1
        );
        assert!(install_coordinator_handoff(home.path(), &repo.join("missing-repo")).is_err());
        assert_eq!(std::fs::read(&manifest).unwrap(), original);
        assert_eq!(
            std::fs::read_dir(target.parent().unwrap()).unwrap().count(),
            1
        );
    }

    #[test]
    fn review_install_order_uses_project_then_numeric_suffix() {
        for (earlier, later) in [("review-9", "review-10"), ("review-99", "review-100")] {
            assert!(review_precedes(("demo", earlier), ("demo", later)));
            assert!(!review_precedes(("demo", later), ("demo", earlier)));
            assert!(!review_precedes(("demo", earlier), ("demo", earlier)));
        }
        assert!(review_precedes(
            ("alpha", "review-100"),
            ("beta", "review-9")
        ));
        assert!(!review_precedes(
            ("beta", "review-9"),
            ("alpha", "review-100")
        ));
        assert!(review_precedes(
            ("demo", "pile-review-9"),
            ("demo", "pile-review-10")
        ));
    }

    #[test]
    fn install_proof_names_the_lock_holder_not_an_installers_child() {
        let home = tempfile::tempdir().unwrap();
        let root = home.path().join("root");
        crate::project::create(&root, "demo", "", vec![]).unwrap();
        let config_dir = home.path().join("cfg");
        let _install = lock(&config_dir).unwrap();
        let path = crate::ticker::lock_path(&root);
        let mut holder = std::fs::File::options()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)
            .unwrap();
        holder.lock().unwrap();
        use std::io::Write;
        holder
            .write_all(format!(r#"{{"pid":4321,"version":"{}"}}"#, crate::VERSION).as_bytes())
            .unwrap();
        let env = crate::paths::Env::for_test(home.path(), &[]);
        let runner = FakeRunner::new();
        runner.on("ticker start", ok(""));
        runner.on("ticker status", ok(&status_receipt(&observation(&root))));
        let ctx = Ctx {
            env: &env,
            root,
            config_dir,
            runner: &runner,
            detached_ticker: true,
        };
        let proof = local_process_proofs(&ctx, Some(crate::VERSION)).unwrap();
        assert!(proof[0].verified());
        assert!(
            matches!(&proof[0], ProcessProof::Observed { observation: Observation { ticker: TickerProof::Running { pid, build }, .. }, .. } if pid.get() == 4321 && build == crate::VERSION)
        );
        let message = InstallOutcome {
            repositories: vec![],
            boxes: vec![],
            live_handoff_required: false,
            processes: proof,
            coordinator_hooks: vec![],
            checks: check::InstallCheck {
                before: vec![],
                after: vec![],
                result: String::new(),
            },
        }
        .message();
        assert!(message.contains("4321") && message.contains(crate::VERSION));
    }

    #[test]
    fn box_stale_ticker_is_an_install_failure_not_a_warning() {
        let proof = process_proof(
            "buildbox",
            crate::VERSION,
            Ok(ok(&status_receipt(&Observation {
                binary: format!("herdr-ade {}", crate::VERSION),
                ticker: TickerProof::Stale {
                    pid: std::num::NonZeroU32::new(42928).unwrap(),
                    build: "0.1.0+4083d1b.1".into(),
                },
            }))),
        );
        let mut outcome = empty_outcome();
        outcome.boxes.push(BoxInstall {
            machine: "buildbox".into(),
            target: "box".into(),
            settings_installed: true,
            errors: vec![],
            process: proof,
        });
        assert!(outcome.box_failed());
        assert!(outcome.warnings().contains("42928"));
    }

    #[test]
    fn box_connection_failure_cannot_pass_without_a_ticker_proof() {
        let proof = process_proof(
            "buildbox",
            crate::VERSION,
            Ok(fail(255, "connection refused")),
        );
        assert!(!proof.verified());
        assert!(
            matches!(proof, ProcessProof::Unknown { reason, .. } if reason.contains("connection refused"))
        );
    }

    fn status_receipt(observation: &Observation) -> String {
        serde_json::json!({"data": {"observation": observation}}).to_string()
    }

    fn current_receipt() -> String {
        status_receipt(&Observation {
            binary: format!("herdr-ade {}", crate::VERSION),
            ticker: TickerProof::Running {
                pid: std::num::NonZeroU32::new(42).unwrap(),
                build: crate::VERSION.into(),
            },
        })
    }

    fn empty_outcome() -> InstallOutcome {
        InstallOutcome {
            repositories: vec![],
            boxes: vec![],
            live_handoff_required: false,
            processes: vec![],
            coordinator_hooks: vec![],
            checks: check::InstallCheck {
                before: vec![],
                after: vec![],
                result: String::new(),
            },
        }
    }

    fn write_version_binary(path: &Path, version: &str, tag: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\n# {tag}\necho '{version}'\n")).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
    }

    fn dirty_then_clean_install(on_box: bool) {
        let root = tempfile::tempdir().unwrap();
        let repo = root.path().join("repo");
        std::fs::create_dir_all(repo.join("target/release")).unwrap();
        let stamp = repo.join("build.rs");
        std::fs::write(&stamp, "// build stamp\n").unwrap();
        std::fs::File::options()
            .write(true)
            .open(&stamp)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .unwrap();
        std::fs::write(repo.join("dirty"), "untracked files").unwrap();
        for bin in Kind::Plugin.binaries() {
            write_version_binary(
                &repo.join("target/release").join(bin),
                &format!("{bin} 0.1.0+abc1234-dirty.1"),
                "cached dirty build",
            );
            // macOS /bin/sh compares -nt at whole-second precision. Give the
            // cached image an older timestamp than a later refresh, while
            // keeping it newer than the untouched epoch build stamp.
            std::fs::File::options()
                .write(true)
                .open(repo.join("target/release").join(bin))
                .unwrap()
                .set_times(
                    std::fs::FileTimes::new()
                        .set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(1)),
                )
                .unwrap();
        }
        let tools = root.path().join("tools");
        std::fs::create_dir(&tools).unwrap();
        let cargo = tools.join("cargo");
        std::fs::write(
            &cargo,
            "#!/bin/sh\nset -e\n\
             # Model Cargo reusing its stamp unless a package file changes.\n\
             if [ build.rs -nt target/release/herdr-ade ]; then\n\
               dirty=\n\
               if [ -f dirty ]; then dirty=-dirty; fi\n\
               for bin in herdr-ade herdr-pi herdr-rundown; do\n\
                 printf '#!/bin/sh\\necho \"%s 0.1.0+abc1234%s.2\"\\n' \"$bin\" \"$dirty\" > \"target/release/$bin\"\n\
                 chmod 755 \"target/release/$bin\"\n\
               done\n\
             fi\n",
        )
        .unwrap();
        std::fs::set_permissions(&cargo, std::fs::Permissions::from_mode(0o755)).unwrap();
        let git = tools.join("git");
        std::fs::write(
            &git,
            "#!/bin/sh\ncase \"$1\" in\nrev-parse) echo abc1234 ;;\nstatus) if [ -f dirty ]; then echo '?? dirty'; fi ;;\nesac\nexit 0\n",
        )
        .unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:/usr/bin:/bin", tools.display());
        let env = crate::paths::Env::for_test(root.path(), &[("PATH", &path)]);
        let runner = FakeRunner::new();
        let shell_path = path.clone();
        runner.on_fn(
            |cmd| cmd.program == "ssh",
            move |cmd| {
                RealRunner.run(
                    &Cmd::new("/bin/sh", BOX_BUILD_TIMEOUT)
                        .args(["-c".into(), cmd.args.last().unwrap().clone()])
                        .env("PATH", &shell_path),
                )
            },
        );
        runner.on_fn(
            |cmd| cmd.program == "cargo",
            move |cmd| {
                RealRunner.run(&Cmd {
                    program: cargo.to_string_lossy().into_owned(),
                    ..cmd.clone()
                })
            },
        );
        runner.on_fn(|_| true, |cmd| RealRunner.run(cmd));
        let ctx = Ctx {
            env: &env,
            root: root.path().join("root"),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        let installed = root.path().join(".local/bin");
        let machine = crate::remote::MachineDeclaration {
            path,
            ade_bin: installed.join("herdr-ade").to_string_lossy().into_owned(),
            pi_bin: installed.join("herdr-pi").to_string_lossy().into_owned(),
            ..Default::default()
        };
        let install = |clean| {
            if on_box {
                assert_eq!(
                    box_build(
                        &ctx,
                        "box",
                        &machine,
                        repo.to_str().unwrap(),
                        Kind::Plugin,
                        "abc1234"
                    )
                    .unwrap(),
                    "abc1234"
                );
            } else {
                local_build(&ctx, repo.to_str().unwrap(), Kind::Plugin, clean).unwrap();
                for bin in Kind::Plugin.binaries() {
                    local_install(&ctx, repo.to_str().unwrap(), bin, "abc1234", clean).unwrap();
                }
            }
        };
        install(false);
        assert_eq!(
            std::fs::metadata(&stamp).unwrap().modified().unwrap(),
            std::time::UNIX_EPOCH
        );
        for bin in Kind::Plugin.binaries() {
            assert!(
                binary_version(&ctx, &installed.join(bin))
                    .unwrap()
                    .contains("-dirty")
            );
            // Reproduce the old installer recording a clean commit despite
            // copying the cached dirty image. The next install must repair it.
            record_installed_commit(&installed, bin, "abc1234").unwrap();
        }
        std::fs::remove_file(repo.join("dirty")).unwrap();
        install(true);
        for bin in Kind::Plugin.binaries() {
            assert_eq!(
                binary_version(&ctx, &installed.join(bin)).unwrap(),
                format!("{bin} 0.1.0+abc1234.2")
            );
        }
        let refreshed = std::fs::metadata(&stamp).unwrap().modified().unwrap();
        let inode = std::fs::metadata(installed.join("herdr-ade"))
            .unwrap()
            .ino();
        install(true);
        assert_eq!(
            std::fs::metadata(&stamp).unwrap().modified().unwrap(),
            refreshed
        );
        assert_eq!(
            std::fs::metadata(installed.join("herdr-ade"))
                .unwrap()
                .ino(),
            inode
        );
    }

    #[test]
    fn box_install_refreshes_a_cached_dirty_stamp_after_the_checkout_is_clean() {
        dirty_then_clean_install(true);
    }

    #[test]
    fn mac_install_refreshes_a_cached_dirty_stamp_after_the_checkout_is_clean() {
        dirty_then_clean_install(false);
    }

    /// Write an executable `zig` that answers `zig version` with `version`.
    fn fake_zig(path: &Path, version: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\necho {version}\n")).unwrap();
        let mut perms = std::fs::metadata(path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(path, perms).unwrap();
    }

    /// Run the box zig snippet for `box_path` with `path` as PATH and print the
    /// resolved `ZIG` on success; on failure the snippet's stderr comes back.
    fn resolve(box_path: &Path, path: &Path) -> std::process::Output {
        let script = format!(
            "{}\nprintf '%s\\n' \"$ZIG\"",
            box_zig_script(box_path.to_str().unwrap())
        );
        std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(script)
            .env("PATH", path)
            .output()
            .unwrap()
    }

    #[test]
    fn agent_runs_idempotent_ensure_not_the_loop_or_keepalive() {
        let home = Path::new("/Users/test");
        let env = crate::paths::Env::for_test(
            home,
            &[
                ("PATH", "/usr/bin"),
                ("XDG_CONFIG_HOME", "/Users/test/config"),
                ("HERDR_BIN_PATH", "/opt/herdr"),
            ],
        );
        let runner = crate::runner::RealRunner;
        let ctx = Ctx {
            env: &env,
            root: home.join(".herdr-ade"),
            config_dir: env.config_dir(),
            runner: &runner,
            detached_ticker: false,
        };
        let definition = ticker_agent_definition(&ctx);
        assert!(definition.contains("<key>StartInterval</key><integer>120</integer>"));
        assert!(definition.contains("<string>ticker</string><string>ensure</string>"));
        assert!(definition.contains("<key>RunAtLoad</key><true/>"));
        assert!(definition.contains("HERDR_ADE_TICKER_SUPERVISOR"));
        assert!(definition.contains("/Users/test/.local/bin:/bin:/usr/bin"));
        assert!(
            definition.contains("<key>XDG_CONFIG_HOME</key><string>/Users/test/config</string>")
        );
        assert!(definition.contains("<key>HERDR_BIN_PATH</key><string>/opt/herdr</string>"));
        assert!(!definition.contains("KeepAlive"));
        assert!(!definition.contains("<string>run</string>"));
    }

    #[test]
    fn an_unchanged_loaded_agent_does_not_reload_during_ticker_replacement() {
        assert!(!agent_bootout(true, false));
        assert!(!agent_bootstrap(true, false));
        assert!(agent_bootout(true, true));
        assert!(agent_bootstrap(true, true));
    }

    #[test]
    fn install_resolves_a_saved_machine_declaration_and_its_kinds_by_label() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::write(
            root.path().join("config/config.toml"),
            crate::remote::TEST_MACHINE.replace("label = \"buildbox\"", "label = \"remote-box\""),
        )
        .unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let runner = FakeRunner::new();
        runner.on(
            "machine list --json",
            ok(r#"[{"id":"example-machine","label":"remote-box","target":"saved-box","session":"default","enabled":true}]"#),
        );
        let ctx = Ctx {
            env: &env,
            root: root.path().join("root"),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };

        let (profile, declaration) = install_box(&ctx, "buildbox").unwrap();

        assert_eq!(profile.id, "example-machine");
        assert_eq!(profile.label, "remote-box");
        assert_eq!(profile.target, "saved-box");
        assert_eq!(declaration.id, "buildbox");
        assert_eq!(declaration.label, "remote-box");
        assert_eq!(declaration.build, "/home/agent/build/lanes");
        assert!(declaration.runs_kind("pi"));
        assert!(!declaration.runs_kind("claude"));
        assert!(!declaration.runs_kind("agy"));
    }

    #[test]
    fn installed_image_checks_counts_and_rolls_back_before_any_box_command() {
        for (done, total, records_load, regresses) in [
            (0, 1, true, true),
            (1, 1, false, true),
            (2, 3, true, false),
            (1, 2, true, false),
        ] {
            let home = tempfile::tempdir().unwrap();
            let root = home.path().join("root");
            let project =
                crate::project::create(&root, "demo", "private project text", vec![]).unwrap();
            let task = crate::task::Task {
                id: "job-0001".into(),
                title: "historical task".into(),
                authority: vec!["request:historical".into()],
                acceptance: vec!["done".into()],
                installed: vec![crate::task::Evidence {
                    at: String::new(),
                    command: "historical install".into(),
                    acceptance: vec![],
                    machine: None,
                    build: None,
                }],
                ..Default::default()
            };
            let tasks = project.record_dir_for_write("tasks").unwrap();
            std::fs::write(tasks.join("job-0001.toml"), toml::to_string(&task).unwrap()).unwrap();
            let plan = crate::contracts::Plan {
                steps: vec![crate::contracts::PlanStep {
                    id: "s-1".into(),
                    tasks: vec![task.id],
                    ..Default::default()
                }],
                ..Default::default()
            };
            std::fs::write(
                crate::plan::plan_path(&project),
                toml::to_string(&plan).unwrap(),
            )
            .unwrap();
            assert_eq!(crate::plan::counts(&project).unwrap(), (1, 1));
            let config_dir = home.path().join("config");
            std::fs::create_dir(&config_dir).unwrap();
            let repo = home.path().join("plugin");
            std::fs::create_dir_all(repo.join("target/release")).unwrap();
            std::fs::write(repo.join("Cargo.toml"), "[package]\nname = 'herdr-ade'\n").unwrap();
            std::fs::write(
                config_dir.join("config.toml"),
                format!("[harness]\nrepos = [{{path = '{}' }}]\n", repo.display()),
            )
            .unwrap();
            #[cfg(target_os = "macos")]
            {
                let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("mods/coordinator-handoff");
                fn copy_dir(from: &Path, to: &Path) {
                    std::fs::create_dir_all(to).unwrap();
                    for entry in std::fs::read_dir(from).unwrap() {
                        let entry = entry.unwrap();
                        if entry.file_type().unwrap().is_dir() {
                            copy_dir(&entry.path(), &to.join(entry.file_name()));
                        } else {
                            std::fs::copy(entry.path(), to.join(entry.file_name())).unwrap();
                        }
                    }
                }
                copy_dir(&source, &repo.join("mods/coordinator-handoff"));
            }
            let installed_dir = home.path().join(".local/bin");
            std::fs::create_dir_all(&installed_dir).unwrap();
            for bin in Kind::Plugin.binaries() {
                write_version_binary(
                    &installed_dir.join(bin),
                    &format!("{bin} 0.1.0+old1234.1"),
                    "old image",
                );
                record_installed_commit(&installed_dir, bin, "old1234").unwrap();
                write_version_binary(
                    &repo.join("target/release").join(bin),
                    &format!("{bin} {}", crate::VERSION),
                    "new image",
                );
            }
            // Keep Rundown unchanged: this test is about the checked installer,
            // not Herdr's pane API.
            std::fs::copy(
                installed_dir.join("herdr-rundown"),
                repo.join("target/release/herdr-rundown"),
            )
            .unwrap();
            let after = vec![check::ProjectCheck {
                project: "demo".into(),
                done,
                total,
                records_load,
            }];
            let json = serde_json::to_string(&after).unwrap();
            let source = repo.join("target/release/herdr-ade");
            let status = status_receipt(&Observation {
                binary: format!("herdr-ade {}", crate::VERSION),
                ticker: TickerProof::Running {
                    pid: std::num::NonZeroU32::new(4321).unwrap(),
                    build: crate::VERSION.into(),
                },
            });
            std::fs::write(&source, format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'herdr-ade {}'; else case \"$*\" in *'ticker status') printf '%s\\n' '{}' ;; *) printf '%s\\n' '{}' ;; esac; fi\n", crate::VERSION, status, json)).unwrap();
            let before_image = std::fs::read(installed_dir.join("herdr-ade")).unwrap();
            let runner = FakeRunner::new();
            runner.on("rev-parse HEAD", ok("new1234"));
            runner.on("status --porcelain", ok(""));
            runner.on("cargo build", ok(""));
            runner.on("touch build.rs", ok(""));
            runner.on("refresh-guard", ok(""));
            runner.on_fn(
                |cmd| {
                    cmd.program == "cp"
                        || cmd.program == "mv"
                        || cmd.args == ["--version"]
                        || cmd.display().contains("install-check")
                        || cmd.display().contains("ticker start")
                        || cmd.display().contains("ticker status")
                },
                |cmd| {
                    let output = RealRunner.run(cmd)?;
                    if cmd.display().contains("ticker start") {
                        assert!(
                            output.stdout.contains("old1234"),
                            "rollback started the wrong image"
                        );
                        assert!(
                            cmd.env
                                .contains(&("HERDR_ADE_INSTALL_TICKER".into(), "1".into()))
                        );
                    }
                    Ok(output)
                },
            );
            let env = crate::paths::Env::for_test(home.path(), &[]);
            let ctx = Ctx {
                env: &env,
                root,
                config_dir,
                runner: &runner,
                detached_ticker: regresses,
            };
            // Existing proof gate still observes the actual ticker lock holder.
            let mut holder = std::fs::File::options()
                .create(true)
                .truncate(false)
                .write(true)
                .open(crate::ticker::lock_path(&ctx.root))
                .unwrap();
            holder.lock().unwrap();
            use std::io::Write;
            write!(
                holder,
                "{{\"pid\":4321,\"version\":\"{}\"}}",
                crate::VERSION
            )
            .unwrap();
            let result = install(&ctx);
            assert_eq!(result.is_err(), regresses, "{result:?}");
            let record: serde_json::Value =
                crate::project::read_json(&ctx.config_dir.join("harness-install.json")).unwrap();
            assert_eq!(record["before"][0]["done"], 1);
            assert_eq!(record["after"][0]["done"], done, "{result:?}; {record}");
            assert!(!record.to_string().contains("private project text"));
            if regresses {
                assert!(
                    result
                        .unwrap_err()
                        .to_string()
                        .contains("rolled back on mac, boxes untouched")
                );
                for bin in Kind::Plugin.binaries() {
                    assert_eq!(
                        std::fs::read_to_string(install_record(&installed_dir, bin)).unwrap(),
                        "old1234\n"
                    );
                }
                assert_eq!(
                    std::fs::read(installed_dir.join("herdr-ade")).unwrap(),
                    before_image
                );
                assert_eq!(runner.count("ssh"), 0);
                assert_eq!(runner.count("machine list"), 0);
                assert_eq!(runner.count("ticker start"), 1);
            } else {
                let outcome = result.unwrap();
                assert!(outcome.summary().contains("records load"));
                assert!(
                    serde_json::to_value(outcome)
                        .unwrap()
                        .get("tasks")
                        .is_none()
                );
                assert_eq!(runner.count("merge-base"), 0);
                assert_eq!(crate::plan::counts(&project).unwrap(), (1, 1));
                assert_eq!(crate::task::list_with_errors(&project).0.len(), 1);
                assert_ne!(
                    std::fs::read(installed_dir.join("herdr-ade")).unwrap(),
                    before_image
                );
            }
            assert!(
                !installed_dir
                    .join(format!(".harness-previous-{}", std::process::id()))
                    .exists()
            );
        }
    }

    #[test]
    fn install_visits_both_declared_boxes_even_when_first_build_fails() {
        // Separate installs must not reuse a lock briefly inherited by a
        // concurrently forked child from another test.
        for fail_first in [true, false] {
            let root = tempfile::tempdir().unwrap();
            let config_dir = root.path().join("config");
            std::fs::create_dir_all(&config_dir).unwrap();
            std::fs::write(config_dir.join("RULES.md"), "worker rules").unwrap();
            let repo = root.path().join("fork");
            std::fs::create_dir_all(repo.join("target/release")).unwrap();
            std::fs::write(repo.join("Cargo.toml"), "[package]\nname = \"herdr\"\n").unwrap();
            std::fs::write(repo.join("target/release/herdr"), "binary").unwrap();
            let config = format!(
                "[harness]\nrepos = [{{ path = '{}', box_path = '/generic/fork' }}]\n",
                repo.display()
            );
            let machines = ["alpha", "beta"].iter().map(|label| format!(
            "\n[machines.{label}]\ntarget = '{label}'\nsession = 'default'\nhome = '/home/{label}'\nroot = '/home/{label}/ade'\nworktrees = '/home/{label}/work'\nbuild = '/home/{label}/build'\npath = '/home/{label}/bin:/usr/bin:/bin'\nade_bin = '/home/{label}/bin/herdr-ade'\npi_bin = '/home/{label}/bin/herdr-pi'\nrepos = [{{ path = '{}', box_path = '/home/{label}/fork' }}]\n",
            repo.display()
        )).collect::<String>();
            std::fs::write(
                config_dir.join("config.toml"),
                format!("{config}{machines}"),
            )
            .unwrap();
            let env = crate::paths::Env::for_test(root.path(), &[]);
            let runner = FakeRunner::new();
            runner.on("machine list --json", ok("[]"));
            runner.on("install-check", ok("[]"));
            runner.on("ticker status", ok(&current_receipt()));
            runner.on_fn(
                |cmd| cmd.program == "git" && cmd.display().contains("rev-parse HEAD"),
                |_| Ok(ok("abc123\n")),
            );
            runner.on_fn(
                |cmd| cmd.program == "git" && cmd.display().contains("status --porcelain"),
                |_| Ok(ok("")),
            );
            runner.on_fn(|cmd| cmd.program == "cargo", |_| Ok(ok("")));
            runner.on_fn(
                |cmd| cmd.program == "cp" || cmd.program == "mv",
                |cmd| RealRunner.run(cmd),
            );
            runner.on_fn(
                |cmd| cmd.args == ["--version"],
                |_| Ok(ok("herdr 0.1.0+abc123.1\n")),
            );
            runner.on_fn(
                |cmd| cmd.program == "ssh",
                move |cmd| {
                    let display = cmd.display();
                    if fail_first && display.contains("alpha") && display.contains("git fetch") {
                        Ok(fail(1, "fetch failed"))
                    } else if display.contains("ticker status") {
                        Ok(ok(&current_receipt()))
                    } else if display.contains("git fetch") {
                        Ok(ok("abc123\n"))
                    } else {
                        Ok(ok(""))
                    }
                },
            );
            let ctx = Ctx {
                env: &env,
                root: root.path().join("root"),
                config_dir: config_dir.clone(),
                runner: &runner,
                detached_ticker: false,
            };
            let outcome = install(&ctx).unwrap();
            assert_eq!(outcome.boxes.len(), 2);
            assert_eq!(outcome.box_failed(), fail_first);
            assert_eq!(
                outcome.boxes[0]
                    .errors
                    .iter()
                    .any(|e| matches!(e, InstallFailure::Build { reason, .. } if reason.contains("fetch failed"))),
                fail_first,
                "{:?}",
                outcome.boxes
            );
            assert!(outcome.boxes[1].errors.is_empty(), "{:?}", outcome.boxes[1]);
            if !fail_first {
                assert!(outcome.boxes[0].errors.is_empty(), "{:?}", outcome.boxes[0]);
            }
            assert_eq!(
                outcome.warnings().contains("box pending: alpha"),
                fail_first
            );
            assert!(
                outcome.repositories[0]
                    .boxes
                    .iter()
                    .any(|r| r.machine == "beta"
                        && r.commit == "abc123"
                        && r.path == "/home/beta/fork")
            );
            assert_eq!(runner.count("git fetch"), 2);
            assert!(outcome.boxes.iter().all(|b| b.settings_installed));
        }
    }

    #[test]
    fn box_zig_wrong_version_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let box_path = root.path().join("herdr");
        let local = box_path.join(".target/rebase/zig-0.16.0/zig");
        fake_zig(&local, "0.15.0");
        let empty = root.path().join("bin");
        std::fs::create_dir_all(&empty).unwrap();
        let out = resolve(&box_path, &empty);
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("harness_box_zig_version"), "{stderr}");
        assert!(stderr.contains("0.15.0"), "{stderr}");
        assert!(stderr.contains("0.16.0"), "{stderr}");
    }

    #[test]
    fn box_build_reports_integration_not_published_even_if_local_main_has_it() {
        let fx = crate::testkit::fixture();
        let published = fx.world.home.path().join("published.git");
        let box_clone = fx.world.home.path().join("box");
        crate::testkit::git(
            &fx.repo,
            &[
                "clone",
                "--bare",
                "-q",
                fx.repo.to_str().unwrap(),
                published.to_str().unwrap(),
            ],
        );
        crate::testkit::git(
            &fx.repo,
            &[
                "clone",
                "-q",
                published.to_str().unwrap(),
                box_clone.to_str().unwrap(),
            ],
        );
        let candidate = crate::testkit::commit_file(
            &fx.repo,
            "candidate.txt",
            "candidate",
            "unpublished integration",
        );
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.program == "ssh",
            |cmd| {
                RealRunner.run(
                    &Cmd::new("/bin/sh", BOX_BUILD_TIMEOUT).args(["-c", cmd.args.last().unwrap()]),
                )
            },
        );
        let ctx = Ctx {
            runner: &runner,
            ..fx.world.ctx()
        };
        let machine = remote::MachineDeclaration {
            path: "/usr/bin:/bin".into(),
            ade_bin: fx
                .world
                .home
                .path()
                .join("bin/herdr-ade")
                .to_string_lossy()
                .into_owned(),
            pi_bin: fx
                .world
                .home
                .path()
                .join("bin/herdr-pi")
                .to_string_lossy()
                .into_owned(),
            ..Default::default()
        };
        for candidate_in_clone in [false, true] {
            if candidate_in_clone {
                crate::testkit::git(&box_clone, &["fetch", fx.repo.to_str().unwrap(), "main"]);
                crate::testkit::git(&box_clone, &["merge", "--ff-only", "FETCH_HEAD"]);
            }
            let error = box_build(
                &ctx,
                "a2",
                &machine,
                box_clone.to_str().unwrap(),
                Kind::Plugin,
                &candidate,
            )
            .unwrap_err();
            assert!(
                error.to_string().contains("integration not published:"),
                "{error:#}"
            );
            assert!(error.to_string().contains(&candidate), "{error:#}");
            assert!(!Path::new(&machine.ade_bin).exists());
        }
    }

    #[test]
    fn box_build_for_the_fork_resolves_zig_on_the_box() {
        let root = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let runner = FakeRunner::new();
        runner.on("ssh", ok("abc1234\n"));
        let ctx = Ctx {
            env: &env,
            root: root.path().to_path_buf(),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        let machine = crate::remote::MachineDeclaration {
            path: "/bin:$HOME/.local/bin".into(),
            ade_bin: "/srv/bin/herdr-ade".into(),
            pi_bin: "/srv/bin/herdr-pi".into(),
            ..Default::default()
        };
        box_build(
            &ctx,
            "box",
            &machine,
            "/home/agent/projects/herdr",
            Kind::Fork,
            "abc1234",
        )
        .unwrap();
        let calls = runner.calls.borrow();
        let script = calls.last().unwrap().args.last().unwrap();
        assert!(script.contains("herdr_repo_zig"), "{script}");
        assert!(script.contains("command -v zig"), "{script}");
        assert!(script.contains("harness_box_zig_missing"), "{script}");
        assert!(script.contains("harness_box_zig_version"), "{script}");
        assert!(script.contains("export ZIG"), "{script}");
        assert!(
            !script.contains("ZIG=/home/agent/projects/herdr/.target"),
            "{script}"
        );
    }

    #[test]
    fn an_unreachable_box_has_unknown_process_state() {
        let root = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let runner = FakeRunner::new();
        runner.on("ssh", fail(255, "connection refused"));
        let ctx = Ctx {
            env: &env,
            root: root.path().to_path_buf(),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };

        let machine = crate::remote::MachineDeclaration {
            id: "lab".into(),
            target: "box".into(),
            root: "/srv/ade".into(),
            path: "/srv/bin:/usr/bin:/bin".into(),
            ade_bin: "/srv/bin/herdr-ade".into(),
            ..Default::default()
        };
        let proofs = box_process_proofs(&ctx, &machine, crate::VERSION);
        assert!(
            matches!(proofs, ProcessProof::Unknown { machine, reason } if machine == "lab" && reason.contains("connection refused"))
        );
    }

    #[test]
    fn box_ticker_lock_holder() {
        let Ok(path) = std::env::var("HERDR_ADE_TEST_TICKER_LOCK") else {
            return;
        };
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .unwrap();
        file.lock().unwrap();
        use std::io::{Read as _, Write as _};
        std::io::stdout().write_all(b"locked\n").unwrap();
        std::io::stdout().flush().unwrap();
        std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
    }

    fn test_box_path(_root: &Path) -> String {
        "/usr/bin:/bin".into()
    }

    #[test]
    fn a_box_ticker_from_the_same_commit_passes_with_its_exact_build() {
        let root = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let commit = crate::build::commit_version(crate::VERSION).unwrap();
        let box_build = format!("{commit}.9999999999");
        let box_binary = format!("herdr-ade {box_build}");
        let bin = root.path().join("herdr-ade");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\ncase \"$*\" in *'ticker status') echo '{}' ;; esac\n",
                status_receipt(&Observation {
                    binary: box_binary.clone(),
                    ticker: TickerProof::Running {
                        pid: std::num::NonZeroU32::new(42).unwrap(),
                        build: box_build.clone()
                    }
                })
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&bin, permissions).unwrap();
        let box_root = root.path().join("ade-root");
        std::fs::create_dir(&box_root).unwrap();
        std::fs::write(
            box_root.join(".ticker.lock"),
            format!("{{\n  \"version\": \"{box_build}\",\n  \"pid\": 42\n}}"),
        )
        .unwrap();
        // Hold the lock in a separate Rust process: this test process may
        // fork concurrently, temporarily passing its descriptors to children.
        #[cfg(target_os = "linux")]
        let executable = PathBuf::from("/proc/self/exe");
        #[cfg(not(target_os = "linux"))]
        let executable = std::env::current_exe().unwrap();
        let mut holder = std::process::Command::new(executable)
            .args([
                "--exact",
                "harness::tests::box_ticker_lock_holder",
                "--nocapture",
            ])
            .env("HERDR_ADE_TEST_TICKER_LOCK", box_root.join(".ticker.lock"))
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        use std::io::BufRead as _;
        let mut ready = String::new();
        let mut output = std::io::BufReader::new(holder.stdout.take().unwrap());
        loop {
            let read = output.read_line(&mut ready).unwrap();
            assert!(
                read > 0,
                "lock holder exited before acquiring the lock: {ready}"
            );
            if ready.contains("locked\n") {
                break;
            }
        }
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.program == "ssh",
            |cmd| {
                RealRunner.run(&Cmd {
                    program: "sh".into(),
                    args: vec!["-c".into(), cmd.args.last().unwrap().clone()],
                    ..cmd.clone()
                })
            },
        );
        let ctx = Ctx {
            env: &env,
            root: root.path().to_path_buf(),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        let machine = crate::remote::MachineDeclaration {
            id: "buildbox".into(),
            target: "box".into(),
            root: box_root.to_string_lossy().into_owned(),
            path: test_box_path(root.path()),
            ade_bin: bin.to_string_lossy().into_owned(),
            ..Default::default()
        };

        let proofs = box_process_proofs(&ctx, &machine, crate::VERSION);

        assert!(proofs.verified());
        assert!(
            matches!(&proofs, ProcessProof::Observed { observation: Observation { binary, ticker: TickerProof::Running { pid, build } }, .. } if binary == &box_binary && pid.get() == 42 && build == &box_build)
        );
        let newer = crate::VERSION.replace(commit, "other-commit");
        assert!(!box_process_proofs(&ctx, &machine, &newer).verified());
        assert!(
            matches!(observation(&box_root).ticker, TickerProof::Running { pid, build } if pid.get() == 42 && build == box_build)
        );
        drop(holder.stdin.take());
        assert!(holder.wait().unwrap().success());
        // A leftover record is not evidence of a running process.
        assert_eq!(observation(&box_root).ticker, TickerProof::NotRequired);
        let script = box_process_script(&machine);
        assert!(
            !script.contains("flock")
                && !script.contains("sed")
                && !script.contains(".ticker.lock")
        );
    }

    #[test]
    fn a_box_without_projects_still_needs_a_running_ticker() {
        let root = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let bin = root.path().join("herdr-ade");
        std::fs::write(
            &bin,
            format!(
                "#!/bin/sh\ncase \"$*\" in *'ticker status') echo '{}' ;; esac\n",
                status_receipt(&observation(&root.path().join("ade-root")))
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&bin, permissions).unwrap();
        let box_root = root.path().join("ade-root");
        std::fs::create_dir(&box_root).unwrap();
        let machine = crate::remote::MachineDeclaration {
            id: "buildbox".into(),
            target: "box".into(),
            root: box_root.to_string_lossy().into_owned(),
            path: test_box_path(root.path()),
            ade_bin: bin.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.program == "ssh",
            |cmd| {
                RealRunner.run(&Cmd {
                    program: "sh".into(),
                    args: vec!["-c".into(), cmd.args.last().unwrap().clone()],
                    ..cmd.clone()
                })
            },
        );
        let ctx = Ctx {
            env: &env,
            root: root.path().to_path_buf(),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        let proofs = box_process_proofs(&ctx, &machine, crate::VERSION);
        let result = BoxInstall {
            machine: "buildbox".into(),
            target: "box".into(),
            settings_installed: true,
            errors: vec![],
            process: proofs,
        };
        assert!(result.pending());
        assert!(
            process_proof(
                "local",
                crate::VERSION,
                Ok(ok(&status_receipt(&observation(&box_root))))
            )
            .verified()
        );
    }

    #[test]
    fn a_box_ticker_without_a_valid_pid_cannot_pass_as_running() {
        let root = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let runner = FakeRunner::new();
        runner.on("ssh", ok(&serde_json::json!({ "data": { "observation": { "binary": format!("herdr-ade {}", crate::VERSION), "ticker": {"state": "running", "pid": 0, "build": crate::VERSION} } } }).to_string()));
        let ctx = Ctx {
            env: &env,
            root: root.path().to_path_buf(),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        let machine = crate::remote::MachineDeclaration {
            id: "buildbox".into(),
            target: "box".into(),
            ..Default::default()
        };
        let proofs = box_process_proofs(&ctx, &machine, crate::VERSION);
        assert!(matches!(proofs, ProcessProof::Unknown { .. }));
    }

    #[test]
    fn an_incomplete_box_ticker_record_is_unknown_not_empty_stale() {
        let root = tempfile::tempdir().unwrap();
        let path = crate::ticker::lock_path(root.path());
        std::fs::write(&path, "{\"version\":\"0.1.0+old.1\"}").unwrap();
        let holder = std::fs::File::options()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        holder.lock().unwrap();
        assert!(matches!(
            observation(root.path()).ticker,
            TickerProof::Unknown { .. }
        ));
        for text in ["{}", "not json", "{\"data\":{}}"] {
            assert!(!process_proof("box", crate::VERSION, Ok(ok(text))).verified());
        }
    }

    #[test]
    fn old_installer_refreshes_guard_with_the_new_installed_image() {
        let home = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(home.path(), &[]);
        let ctx = Ctx {
            env: &env,
            root: home.path().join("selected root"),
            config_dir: home.path().join("config"),
            runner: &RealRunner,
            detached_ticker: false,
        };
        let layout = crate::pi::Layout {
            root: ctx.root.join("pi"),
        };
        crate::pi::install::write_guard(&layout).unwrap();
        let old = std::fs::read_to_string(layout.guard()).unwrap();
        let repo = home.path().join("repo");
        let source = repo.join("target/release/herdr-pi");
        std::fs::create_dir_all(source.parent().unwrap()).unwrap();
        // A different executable image supplies its own extension, not this
        // installer's compiled-in guard. No real install or shared root is used.
        std::fs::write(&source, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'herdr-pi 0.1.0+new1234.200'; exit; fi\n[ \"$1\" = refresh-guard ] || exit 2\nprintf '%s\\n' 'new image extension' > \"$HERDR_ADE_ROOT/pi/agent/extensions/herdr-pi-guard.ts\"\n").unwrap();
        std::fs::set_permissions(&source, std::fs::Permissions::from_mode(0o755)).unwrap();
        local_install(&ctx, repo.to_str().unwrap(), "herdr-pi", "new1234", false).unwrap();
        refresh_local_guard(&ctx).unwrap();
        let new = std::fs::read_to_string(layout.guard()).unwrap();
        assert_eq!(new, "new image extension\n");
        assert_ne!(new, old);
        std::fs::write(
            home.path().join(".local/bin/herdr-pi"),
            "#!/bin/sh\nexit 1\n",
        )
        .unwrap();
        assert!(refresh_local_guard(&ctx).is_err());
    }

    #[test]
    fn installing_the_same_clean_commit_keeps_every_installed_inode() {
        let root = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        // This checks the install decision, not scheduling external version
        // probes under load. The old shell probes could fail and silently
        // turn a skip into an install, making an inode assertion flaky.
        let runner = FakeRunner::new();
        runner.on_fn(
            |cmd| cmd.args == ["--version"],
            |cmd| {
                let bin = Path::new(&cmd.program)
                    .file_name()
                    .unwrap()
                    .to_str()
                    .unwrap();
                let version = if bin == "herdr" {
                    "0.9.1".to_string()
                } else {
                    let stamp = if cmd.program.contains("/target/release/") {
                        200
                    } else {
                        100
                    };
                    format!("0.1.0+abc1234.{stamp}")
                };
                Ok(ok(&format!("{bin} {version}\n")))
            },
        );
        let ctx = Ctx {
            env: &env,
            root: root.path().join("root"),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        let repo = root.path().join("repo");
        let release = repo.join("target/release");
        let installed_dir = root.path().join(".local/bin");
        std::fs::create_dir_all(&release).unwrap();
        std::fs::create_dir_all(&installed_dir).unwrap();
        for bin in ["herdr-ade", "herdr-pi", "herdr-rundown", "herdr"] {
            write_version_binary(&release.join(bin), "new version", "new stamp");
            let installed = installed_dir.join(bin);
            write_version_binary(&installed, "old version", "old stamp");
            if bin == "herdr" {
                std::fs::write(install_record(&installed_dir, bin), "abc1234\n").unwrap();
            }
            let before = std::fs::metadata(&installed).unwrap().ino();
            local_install(&ctx, repo.to_str().unwrap(), bin, "abc1234", true).unwrap();
            assert_eq!(
                std::fs::metadata(&installed).unwrap().ino(),
                before,
                "{bin}"
            );
            assert!(
                std::fs::read_to_string(&installed)
                    .unwrap()
                    .contains("old stamp"),
                "{bin}"
            );
        }
        assert_eq!(runner.count(" --version"), 8);
        assert_eq!(runner.count("cp "), 0);
    }

    #[test]
    fn a_dirty_or_different_commit_is_installed() {
        let root = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let runner = RealRunner;
        let ctx = Ctx {
            env: &env,
            root: root.path().join("root"),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };
        let repo = root.path().join("repo");
        let source = repo.join("target/release/herdr-ade");
        let installed = root.path().join(".local/bin/herdr-ade");
        write_version_binary(&source, "herdr-ade 0.1.0+abc1234.200", "dirty source");
        write_version_binary(
            &installed,
            "herdr-ade 0.1.0+abc1234.100",
            "installed clean source",
        );

        local_install(&ctx, repo.to_str().unwrap(), "herdr-ade", "abc1234", false).unwrap();
        assert!(
            std::fs::read_to_string(&installed)
                .unwrap()
                .contains("dirty source")
        );
        assert!(!install_record(installed.parent().unwrap(), "herdr-ade").exists());

        write_version_binary(&source, "herdr-ade 0.1.0+def5678.300", "different commit");
        local_install(&ctx, repo.to_str().unwrap(), "herdr-ade", "def5678", true).unwrap();
        assert!(
            std::fs::read_to_string(&installed)
                .unwrap()
                .contains("different commit")
        );
    }
}
