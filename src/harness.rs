//! The harness repositories and `ha harness install` (t-0054).
//!
//! The list lives once, in `[harness]` in `config.toml`: each row is a
//! `path`/`box_path` pair, the same shape a project's `repos` rows have. Every
//! project may start lanes and open rounds on a harness repository without
//! listing it in `PROJECT.md`.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::paths::Ctx;
use crate::project::{Repo, Settings};
use crate::remote;
use crate::runner::Cmd;

/// The plugin build's tool path, exactly as the coordinator uses it by hand.
pub(crate) const DEVELOPER_DIR: &str = "/Library/Developer/CommandLineTools";

const BUILD_TIMEOUT: Duration = Duration::from_secs(1800);
const BOX_BUILD_TIMEOUT: Duration = Duration::from_secs(3600);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(120);
const VERSION_TIMEOUT: Duration = Duration::from_secs(30);
const PROCESS_WAIT: Duration = Duration::from_secs(5);
pub(crate) const BOX_WORKER_MARKER: &str = ".lane-worker";

/// The `[harness]` table of `config.toml`.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    dispatch: crate::launch::DispatchConfig,
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

/// True when a project may start a lane or open a round on `path`: the path is
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
            Kind::Plugin => &["herdr-ade", "herdr-pi", "herdr-pro", "herdr-rundown"],
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
    pub(crate) box_path: Option<String>,
    pub(crate) box_installed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) box_commit: Option<String>,
}

/// One installed or long-running process checked after installation. `build`
/// is absent only when the evidence cannot identify the running image.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct ProcessProof {
    pub(crate) machine: String,
    pub(crate) process: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) pid: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) build: Option<String>,
    pub(crate) state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct TaskInstallProof {
    pub(crate) project: String,
    pub(crate) task: String,
    pub(crate) machine: String,
    pub(crate) build: String,
    pub(crate) running_processes: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct InstallOutcome {
    pub(crate) repositories: Vec<InstalledRepo>,
    pub(crate) box_target: Option<String>,
    pub(crate) box_settings_installed: bool,
    pub(crate) live_handoff_required: bool,
    pub(crate) processes: Vec<ProcessProof>,
    pub(crate) tasks: Vec<TaskInstallProof>,
    pub(crate) coordinator_hooks: Vec<String>,
    #[serde(skip)]
    pub(crate) warnings: Vec<String>,
}

impl InstallOutcome {
    pub(crate) fn message(&self) -> String {
        let mut message = self
            .repositories
            .iter()
            .flat_map(|repo| repo.binaries.iter())
            .map(|binary| format!("{}\n", binary.version))
            .collect::<String>();
        for hook in &self.coordinator_hooks {
            message.push_str(&format!("coordinator hook rebound: {hook}\n"));
        }
        for process in &self.processes {
            let identity = process
                .pid
                .map_or_else(String::new, |pid| format!(" pid {pid}"));
            match (&process.build, &process.reason) {
                (Some(build), Some(reason)) => message.push_str(&format!(
                    "{} {}{identity}: {} ({}; {reason})\n",
                    process.machine, process.process, build, process.state
                )),
                (Some(build), None) => message.push_str(&format!(
                    "{} {}{identity}: {} ({})\n",
                    process.machine, process.process, build, process.state
                )),
                (None, Some(reason)) => message.push_str(&format!(
                    "{} {}{identity}: {} ({reason})\n",
                    process.machine, process.process, process.state
                )),
                (None, None) => message.push_str(&format!(
                    "{} {}{identity}: {}\n",
                    process.machine, process.process, process.state
                )),
            }
        }
        if self.live_handoff_required {
            message.push_str("the running server keeps its image; a live handoff is Rolf's call\n");
        }
        message
    }

    pub(crate) fn warnings(&self) -> String {
        self.warnings
            .iter()
            .map(|warning| format!("{warning}\n"))
            .collect()
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
        "herdr-pro" | "herdr-rundown" => Path::new(&machine.ade_bin)
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
) -> Result<Option<String>> {
    let zig = if kind == Kind::Fork {
        format!("\n{}", box_zig_script(box_path))
    } else {
        String::new()
    };
    let mut installs = String::new();
    for bin in kind.binaries() {
        installs.push_str(&format!(
            "\ninstall_to={to}\n\
             install_record=\"$(dirname \"$install_to\")/.{bin}.installed-commit\"\n\
             install_tmp=\"${{install_to}}.install.$$\"\n\
             record_tmp=\"${{install_record}}.$$\"\n\
             mkdir -p \"$(dirname \"$install_to\")\"\n\
             if [ -z \"$source_dirty\" ] && [ -x \"$install_to\" ] && [ \"$(cat \"$install_record\" 2>/dev/null || :)\" = \"$source_head\" ]; then\n\
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
         cd {path}\n\
         git fetch --quiet\n\
         git merge --ff-only @{{u}}\n\
         # The box clone's files arrive by sync, but its index does not.\n\
         git read-tree HEAD && git update-index -q --refresh\n\
         source_head=\"$(git rev-parse HEAD)\"\n\
         source_dirty=\"$(git status --porcelain --untracked-files=normal)\"\n\
         export PATH={build_path}\n\
         export DEVELOPER_DIR={DEVELOPER_DIR}{zig}\n\
         cargo build --release --locked{installs}\n\
         printf 'HERDR_ADE_INSTALLED_HEAD=%s\\n' \"$source_head\"",
        path = remote::quote(box_path),
        build_path = remote::quote(&machine.path),
    );
    let out = remote::ssh(ctx.runner, target, &script, None, BOX_BUILD_TIMEOUT)?;
    if !out.success() {
        bail!(
            "harness_box_failed: build and install on {target} in {box_path}: {}",
            out.error_text()
        );
    }
    Ok(out.stdout.lines().find_map(|line| {
        line.strip_prefix("HERDR_ADE_INSTALLED_HEAD=")
            .map(str::trim)
            .filter(|head| !head.is_empty())
            .map(str::to_string)
    }))
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

/// This process's own binary, fingerprinted before any install starts. Once
/// `local_install` replaces that file, the running image is still the old one;
/// this snapshot is the only way to see that the file changed underneath us.
struct Running {
    /// The canonical path of the running executable.
    path: PathBuf,
    /// The SHA-256 of its bytes as this process started.
    hash: String,
}

impl Running {
    fn capture() -> Result<Running> {
        let current = std::env::current_exe().context("could not find the running executable")?;
        let path = match std::fs::canonicalize(&current) {
            Ok(path) => path,
            Err(error) => {
                // Linux appends this suffix when a concurrent build unlinks the
                // running image. Its inode remains readable via /proc/self/exe.
                #[cfg(target_os = "linux")]
                if let Some(path) = current.to_str().and_then(|s| s.strip_suffix(" (deleted)")) {
                    PathBuf::from(path)
                } else {
                    return Err(error)
                        .with_context(|| format!("could not resolve {}", current.display()));
                }
                #[cfg(not(target_os = "linux"))]
                return Err(error)
                    .with_context(|| format!("could not resolve {}", current.display()));
            }
        };
        #[cfg(target_os = "linux")]
        let image = Path::new("/proc/self/exe");
        #[cfg(not(target_os = "linux"))]
        let image = path.as_path();
        let bytes = std::fs::read(image)
            .with_context(|| format!("could not fingerprint {}", image.display()))?;
        Ok(Running {
            path,
            hash: crate::thread::sha256_hex(&bytes),
        })
    }
}

fn replaced_self(installed: &Path, running: &Running) -> Result<bool> {
    let same = std::fs::canonicalize(installed).is_ok_and(|path| path == running.path);
    if !same {
        return Ok(false);
    }
    let bytes = std::fs::read(installed)
        .with_context(|| format!("could not read {}", installed.display()))?;
    Ok(crate::thread::sha256_hex(&bytes) != running.hash)
}

/// Continue the same invocation in the image it just installed. The install
/// lock is close-on-exec, so the new image starts the transaction again and
/// completes it with its own code; no shell retry is involved.
fn reexec_if_replaced(installed: &Path, running: &Running) -> Result<()> {
    if !replaced_self(installed, running)? {
        return Ok(());
    }
    use std::os::unix::process::CommandExt;
    let error = std::process::Command::new(installed)
        .args(std::env::args_os().skip(1))
        .exec();
    bail!(
        "harness_reexec_failed: could not continue installation in {}: {error}",
        installed.display()
    )
}

fn repo_head(ctx: &Ctx, repo: &str) -> Result<String> {
    let out = ctx.runner.run(&Cmd::new("git", VERSION_TIMEOUT).args([
        "-C",
        repo,
        "rev-parse",
        "HEAD",
    ]))?;
    if !out.success() || out.stdout.trim().is_empty() {
        bail!(
            "harness_build_head: could not read HEAD in {repo}: {}",
            out.error_text()
        );
    }
    Ok(out.stdout.trim().to_string())
}

fn repo_clean(ctx: &Ctx, repo: &str) -> Result<bool> {
    let out = ctx.runner.run(&Cmd::new("git", VERSION_TIMEOUT).args([
        "-C",
        repo,
        "status",
        "--porcelain",
        "--untracked-files=normal",
    ]))?;
    if !out.success() {
        bail!(
            "harness_build_status: could not inspect {repo}: {}",
            out.error_text()
        );
    }
    Ok(out.stdout.trim().is_empty())
}

fn local_process_proofs(ctx: &Ctx, plugin_version: Option<&str>) -> Result<Vec<ProcessProof>> {
    let mut proofs = Vec::new();
    if let Some(version) = plugin_version {
        proofs.push(ProcessProof {
            machine: "local".into(),
            process: "herdr-ade binary".into(),
            pid: None,
            build: Some(version.to_string()),
            state: "installed".into(),
            reason: None,
        });
    }

    if !crate::project::list_slugs(&ctx.root).is_empty() {
        crate::ticker::start_for_install(ctx).context("harness_ticker_failed: local ticker")?;
        let ticker: Result<crate::ticker::Info> = {
            let deadline = Instant::now() + PROCESS_WAIT;
            loop {
                if let crate::ticker::LockState::Held(info) = crate::ticker::lock_state(&ctx.root)
                    && info.pid != 0
                    && crate::build::same_commit(&info.version, crate::VERSION)
                {
                    break Ok(info);
                }
                if Instant::now() >= deadline {
                    break Err(anyhow::anyhow!(
                        "ticker did not report build {}",
                        crate::VERSION
                    ));
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        proofs.push(match ticker {
            Ok(info) => ProcessProof {
                machine: "local".into(),
                process: "ticker".into(),
                pid: Some(info.pid),
                build: Some(info.version),
                state: "running".into(),
                reason: None,
            },
            Err(error) => bail!("harness_ticker_failed: local ticker did not take the lock: {error:#}; lock state: {:?}", crate::ticker::lock_state(&ctx.root)),
        });
    }
    Ok(proofs)
}

fn box_process_script(
    machine: &crate::remote::MachineDeclaration,
    attempts: u32,
    delay_seconds: &str,
) -> String {
    format!(
        "set -e\n\
         export PATH={path}\n\
         bin={bin}\n\
         root={root}\n\
         version=\"$($bin --version)\"\n\
         printf 'HERDR_ADE_BOX_BINARY=%s\\n' \"$version\"\n\
         expected={expected}\n\
         seen=\n\
         pid=\n\
         n=0\n\
         while [ $n -lt {attempts} ]; do\n\
           $bin --root \"$root\" ticker start || :\n\
           seen=\n\
           pid=\n\
           if [ -r \"$root/.ticker.lock\" ] && ! ( flock -n 9 ) 9<>\"$root/.ticker.lock\"; then\n\
             snapshot=$(cat \"$root/.ticker.lock\" 2>/dev/null) || snapshot=\n\
             candidate_seen=$(printf '%s\\n' \"$snapshot\" | sed -n 's/.*\"version\":[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p')\n\
             candidate_pid=$(printf '%s\\n' \"$snapshot\" | sed -n 's/.*\"pid\":[[:space:]]*\\([0-9][0-9]*\\).*/\\1/p')\n\
             if [ -n \"$candidate_seen\" ] && [ -n \"$candidate_pid\" ]; then\n\
               seen=$candidate_seen\n\
               pid=$candidate_pid\n\
               case \"$seen\" in\n\
                 \"$expected\"|\"$expected\".*)\n\
                   printf 'HERDR_ADE_BOX_TICKER=%s:%s\\n' \"$pid\" \"$seen\"\n\
                   exit 0\n\
                   ;;\n\
               esac\n\
             fi\n\
           fi\n\
           n=$((n+1)); sleep {delay_seconds}\n\
         done\n\
         if [ -n \"$seen\" ] && [ -n \"$pid\" ]; then\n\
           printf 'HERDR_ADE_BOX_TICKER_STALE=%s:%s\\n' \"$pid\" \"$seen\"\n\
         else\n\
           printf 'HERDR_ADE_BOX_TICKER_UNKNOWN=ticker lock did not contain a complete build record\\n'\n\
         fi",
        path = remote::quote(&machine.path),
        bin = remote::quote(&machine.ade_bin),
        root = remote::quote(&machine.root),
        expected =
            remote::quote(crate::build::commit_version(crate::VERSION).unwrap_or(crate::VERSION)),
    )
}

fn box_process_proofs(ctx: &Ctx, machine: &crate::remote::MachineDeclaration) -> Vec<ProcessProof> {
    let script = box_process_script(machine, 30, "0.1");
    let target = &machine.target;
    let out = match remote::ssh(ctx.runner, target, &script, None, Duration::from_secs(140)) {
        Ok(out) if out.success() => out,
        Ok(out) => {
            let reason = out.error_text();
            return vec![ProcessProof {
                machine: machine.id.clone(),
                process: "box binary and ticker".into(),
                pid: None,
                build: None,
                state: "unknown".into(),
                reason: Some(reason),
            }];
        }
        Err(error) => {
            return vec![ProcessProof {
                machine: machine.id.clone(),
                process: "box binary and ticker".into(),
                pid: None,
                build: None,
                state: "unknown".into(),
                reason: Some(format!("{error:#}")),
            }];
        }
    };
    let mut proofs = Vec::new();
    if let Some(version) = out
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("HERDR_ADE_BOX_BINARY="))
    {
        let current = crate::build::same_commit(version, crate::VERSION);
        proofs.push(ProcessProof {
            machine: machine.id.clone(),
            process: "herdr-ade binary".into(),
            pid: None,
            build: Some(version.to_string()),
            state: if current { "installed" } else { "stale" }.into(),
            reason: (!current).then(|| {
                format!(
                    "the box binary does not report installed build {}",
                    crate::VERSION
                )
            }),
        });
    } else {
        proofs.push(ProcessProof {
            machine: machine.id.clone(),
            process: "herdr-ade binary".into(),
            pid: None,
            build: None,
            state: "unknown".into(),
            reason: Some("the box did not report its binary version".into()),
        });
    }
    if let Some(value) = out
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("HERDR_ADE_BOX_TICKER="))
    {
        let (pid, build) = value.split_once(':').unwrap_or(("", value));
        let pid = pid.parse::<u32>().ok().filter(|pid| *pid != 0);
        let current = crate::build::same_commit(build, crate::VERSION);
        proofs.push(ProcessProof {
            machine: machine.id.clone(),
            process: "ticker".into(),
            pid,
            build: Some(build.to_string()),
            state: if !current {
                "stale"
            } else if pid.is_some() {
                "running"
            } else {
                "unknown"
            }
            .into(),
            reason: if pid.is_none() {
                Some("the box ticker did not report a valid pid".into())
            } else {
                (!current).then(|| {
                    format!(
                        "the box ticker does not report installed build {}",
                        crate::VERSION
                    )
                })
            },
        });
    } else if let Some(value) = out
        .stdout
        .lines()
        .find_map(|line| line.strip_prefix("HERDR_ADE_BOX_TICKER_STALE="))
    {
        let (pid, build) = value.split_once(':').unwrap_or(("", value));
        proofs.push(ProcessProof {
            machine: machine.id.clone(),
            process: "ticker".into(),
            pid: pid.parse().ok(),
            build: Some(build.to_string()),
            state: "stale".into(),
            reason: Some(format!(
                "the box ticker does not report installed build {}",
                crate::VERSION
            )),
        });
    } else {
        proofs.push(ProcessProof {
            machine: machine.id.clone(),
            process: "ticker".into(),
            pid: None,
            build: None,
            state: "unknown".into(),
            reason: Some(
                out.stdout
                    .lines()
                    .find_map(|line| line.strip_prefix("HERDR_ADE_BOX_TICKER_UNKNOWN="))
                    .unwrap_or("the box did not report ticker evidence")
                    .to_string(),
            ),
        });
    }
    proofs
}

fn require_running_tickers(processes: &[ProcessProof], expected: &[&str]) -> Result<()> {
    for machine in expected {
        if !processes
            .iter()
            .any(|proof| proof.machine == *machine && proof.process == "ticker")
        {
            let evidence = processes
                .iter()
                .filter(|proof| proof.machine == *machine)
                .collect::<Vec<_>>();
            bail!("harness_ticker_failed: {machine} did not report a ticker: {evidence:?}");
        }
    }
    for proof in processes {
        if proof.process == "ticker" && proof.state != "running" {
            bail!(
                "harness_ticker_failed: {} ticker pid {:?}, build {:?}: {}",
                proof.machine,
                proof.pid,
                proof.build,
                proof.reason.as_deref().unwrap_or(&proof.state)
            );
        }
    }
    Ok(())
}

#[derive(Debug, Clone)]
struct InstalledBuild {
    repo: String,
    machine: String,
    head: String,
}

fn round_in_build(ctx: &Ctx, repo: &str, round_head: &str, build_head: &str) -> Result<bool> {
    let out = ctx.runner.run(
        &Cmd::new("git", VERSION_TIMEOUT)
            .args([
                "-C",
                repo,
                "merge-base",
                "--is-ancestor",
                round_head,
                build_head,
            ])
            .exit_meaning(crate::runner::ExitMeaning::Boolean),
    )?;
    out.boolean_answer().context("git ancestry check failed")
}

fn record_task_proofs(
    ctx: &Ctx,
    builds: &[InstalledBuild],
    processes: &[ProcessProof],
) -> Result<Vec<TaskInstallProof>> {
    let processes_pass = !processes.is_empty()
        && processes
            .iter()
            .all(|proof| proof.state == "running" || proof.state == "installed");
    let process_lines: Vec<String> = processes
        .iter()
        .map(|proof| {
            format!(
                "{}:{}:{} ({})",
                proof.machine,
                proof.process,
                proof.build.as_deref().unwrap_or("unknown"),
                proof.state
            )
        })
        .collect();
    let mut recorded = Vec::new();
    for slug in crate::project::list_slugs(&ctx.root) {
        let project = crate::project::Project::load(&ctx.root, &slug)?;
        let mut changed = false;
        let evidence = crate::task::EvidenceSnapshot::load(&project);
        let writes = (|| -> Result<()> {
            for task in crate::task::list_with_errors(&project).0 {
                if !task.dropped.is_empty() {
                    continue;
                }
                if !crate::task::required_states_with_evidence(&project, &task, &evidence)?
                    .iter()
                    .any(|state| state == "installed")
                {
                    continue;
                }
                let rounds: Vec<_> = task
                    .rounds
                    .iter()
                    .filter_map(|id| crate::round::load(&project, id).ok())
                    .filter(|round| round.phase == crate::contracts::RoundPhase::Merged)
                    .collect();
                let mut task_builds = Vec::new();
                for build in builds {
                    let mut carried = false;
                    for round in &rounds {
                        let same_repo = canonical_or(&round.repo) == canonical_or(&build.repo);
                        let head = round.merge.as_ref().and_then(|merge| merge.head.as_deref());
                        if same_repo
                            && let Some(head) = head
                            && round_in_build(ctx, &build.repo, head, &build.head)?
                        {
                            carried = true;
                            break;
                        }
                    }
                    if carried {
                        crate::task::record_installed_deferred(
                            &project,
                            &task.id,
                            &build.machine,
                            &build.head,
                        )?;
                        changed = true;
                        task_builds.push(build.clone());
                        recorded.push(TaskInstallProof {
                            project: slug.clone(),
                            task: task.id.clone(),
                            machine: build.machine.clone(),
                            build: build.head.clone(),
                            running_processes: processes_pass,
                        });
                    }
                }
                if processes_pass && !task_builds.is_empty() {
                    let task_machines = task_builds
                        .iter()
                        .map(|build| build.machine.clone())
                        .collect::<BTreeSet<_>>()
                        .into_iter()
                        .collect();
                    crate::task::record_running_deferred(
                        &project,
                        &task.id,
                        task_machines,
                        process_lines.clone(),
                    )?;
                }
            }
            Ok(())
        })();
        let refreshed = if changed {
            crate::project::refresh_page(&project)
        } else {
            Ok(())
        };
        writes?;
        refreshed?;
    }
    Ok(recorded)
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
    dispatch: &str,
) -> Result<Option<(crate::contracts::MachineProfile, remote::MachineDeclaration)>> {
    if dispatch.is_empty() || dispatch == crate::contracts::MACHINE_LOCAL {
        return Ok(None);
    }
    let Some(profile) = remote::optional_machine_profile(
        ctx.runner,
        &ctx.env.herdr_bin(),
        &ctx.config_dir,
        dispatch,
    )?
    else {
        return Ok(None);
    };
    let declaration = remote::machine_declaration(&ctx.config_dir, &profile.label)?;
    Ok(Some((profile, declaration)))
}

/// `ha harness install`: build every harness repository after a merge and
/// install it into `~/.local/bin`, then the same on the saved box.
pub(crate) fn install(ctx: &Ctx) -> Result<InstallOutcome> {
    let running = Running::capture()?;
    install_with_reexec(ctx, |installed| reexec_if_replaced(installed, &running))
}

pub(crate) fn install_with_reexec(
    ctx: &Ctx,
    mut reexec: impl FnMut(&Path) -> Result<()>,
) -> Result<InstallOutcome> {
    let repos = repos(&ctx.config_dir)?;
    if repos.is_empty() {
        bail!(
            "harness_repos_missing: add `[harness] repos` to {}",
            ctx.config_dir.join("config.toml").display()
        );
    }
    let _lock = lock(&ctx.config_dir)?;
    let mut fork = false;
    let mut kinds = Vec::new();
    let mut installed = Vec::new();
    let mut builds = Vec::new();
    for repo in &repos {
        let kind = kind(&repo.path)?;
        fork |= kind == Kind::Fork;
        let commit = repo_head(ctx, &repo.path)?;
        let clean_before = repo_clean(ctx, &repo.path)?;
        local_build(ctx, &repo.path, kind)?;
        let after_build = repo_head(ctx, &repo.path)?;
        let source_clean = clean_before && repo_clean(ctx, &repo.path)?;
        if after_build != commit {
            bail!(
                "harness_build_changed: {} moved from {commit} to {after_build} while it was building",
                repo.path
            );
        }
        for bin in kind.binaries() {
            local_install(ctx, &repo.path, bin, &commit, source_clean)?;
            reexec(&ctx.env.home.join(".local/bin").join(bin))?;
        }
        builds.push(InstalledBuild {
            repo: repo.path.clone(),
            machine: "local".into(),
            head: commit.clone(),
        });
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
            box_path: repo.box_path.clone(),
            box_installed: false,
            box_commit: None,
        });
    }
    if kinds.contains(&Kind::Plugin) {
        crate::pi::install::write_guard(&crate::pi::Layout {
            root: ctx.root.join("pi"),
        })?;
    }
    let mut warnings = Vec::new();
    for (repo, kind) in repos.iter().zip(&kinds) {
        if *kind == Kind::Plugin
            && let Err(error) = crate::rundown::link_plugin(ctx, &repo.path)
        {
            warnings.push(format!(
                "note: the Rundown tab plugin is not linked: {error:#}"
            ));
        }
    }

    // Machine resolution belongs to the installer image built above. If that
    // image replaced this process, `reexec` never returns and the new image
    // restarts the transaction before any box lookup or box command occurs.
    let document = crate::config::Document::read(&ctx.config_dir)?;
    let dispatch = document.decode::<RawConfig>()?.dispatch.machine;
    let box_machine = install_box(ctx, &dispatch)?;
    let box_target = box_machine
        .as_ref()
        .map(|(profile, _)| profile.target.clone());
    let box_paths = box_machine.map(|(_, declaration)| declaration);
    let mut box_plugin_installed = false;
    for ((repo, kind), installed_repo) in repos.iter().zip(kinds).zip(&mut installed) {
        match (&box_target, &repo.box_path) {
            (Some(target), Some(box_path)) => {
                let machine = box_paths
                    .as_ref()
                    .context("machine path declaration is missing")?;
                let box_commit = box_build(ctx, target, machine, box_path, kind)?;
                if let Some(head) = &box_commit {
                    builds.push(InstalledBuild {
                        repo: repo.path.clone(),
                        machine: machine.id.clone(),
                        head: head.clone(),
                    });
                }
                installed_repo.box_installed = true;
                installed_repo.box_commit = box_commit;
                if kind == Kind::Plugin {
                    box_plugin_installed = true;
                }
            }
            (Some(_), None) => warnings.push(format!(
                "note: {} has no box_path; skipped the box step",
                repo.path
            )),
            (None, _) => {}
        }
    }
    if let (Some(target), Some(machine)) = (&box_target, &box_paths) {
        box_settings(ctx, target, machine)?;
        if box_plugin_installed {
            refresh_box_guard(ctx, target, machine)?;
        }
    }
    let coordinator_hooks = crate::hook::reinstall_open(ctx)?;
    let plugin_version = installed
        .iter()
        .flat_map(|repo| &repo.binaries)
        .find(|binary| binary.name == "herdr-ade")
        .map(|binary| binary.version.clone());
    let mut processes = local_process_proofs(ctx, plugin_version.as_deref())?;
    if let Some(machine) = &box_paths {
        processes.extend(box_process_proofs(ctx, machine));
    }
    let mut expected = Vec::new();
    if !crate::project::list_slugs(&ctx.root).is_empty() {
        expected.push("local");
    }
    if let Some(machine) = &box_paths {
        expected.push(machine.id.as_str());
    }
    require_running_tickers(&processes, &expected)?;
    let tasks = record_task_proofs(ctx, &builds, &processes)?;
    Ok(InstallOutcome {
        repositories: installed,
        box_settings_installed: box_target.is_some(),
        box_target,
        live_handoff_required: fork,
        processes,
        tasks,
        coordinator_hooks,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::{FakeRunner, fail, ok};
    use crate::runner::{RealRunner, Runner};
    use std::os::unix::fs::MetadataExt;
    use std::os::unix::fs::PermissionsExt;

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
        let ctx = Ctx {
            env: &env,
            root,
            config_dir,
            runner: &runner,
            detached_ticker: true,
        };
        let proof = local_process_proofs(&ctx, Some(crate::VERSION)).unwrap();
        let ticker = proof
            .iter()
            .find(|proof| proof.process == "ticker")
            .unwrap();
        assert_eq!(ticker.pid, Some(4321));
        assert_eq!(ticker.build.as_deref(), Some(crate::VERSION));
        assert_eq!(ticker.state, "running");
        let message = InstallOutcome {
            repositories: vec![],
            box_target: None,
            box_settings_installed: false,
            live_handoff_required: false,
            processes: proof,
            tasks: vec![],
            coordinator_hooks: vec![],
            warnings: vec![],
        }
        .message();
        assert!(message.contains(&format!(
            "local ticker pid 4321: {} (running)",
            crate::VERSION
        )));
    }

    #[test]
    fn box_stale_ticker_is_an_install_failure_not_a_warning() {
        let error = require_running_tickers(
            &[ProcessProof {
                machine: "buildbox".into(),
                process: "ticker".into(),
                pid: Some(42928),
                build: Some("0.1.0+4083d1b.1".into()),
                state: "stale".into(),
                reason: Some("old ticker did not release the lock".into()),
            }],
            &["buildbox"],
        )
        .unwrap_err()
        .to_string();
        assert!(
            error.contains("buildbox") && error.contains("42928") && error.contains("4083d1b"),
            "{error}"
        );
    }

    #[test]
    fn box_connection_failure_cannot_pass_without_a_ticker_proof() {
        let error = require_running_tickers(
            &[ProcessProof {
                machine: "buildbox".into(),
                process: "box binary and ticker".into(),
                pid: None,
                build: None,
                state: "unknown".into(),
                reason: Some("connection refused".into()),
            }],
            &["buildbox"],
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("connection refused"), "{error}");
    }

    #[test]
    fn an_unreadable_config_is_not_an_empty_repository_list() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("config.toml")).unwrap();
        let error = repos(dir.path()).unwrap_err().to_string();
        assert!(error.contains("could not read"), "{error}");
    }

    fn write_version_binary(path: &Path, version: &str, tag: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, format!("#!/bin/sh\n# {tag}\necho '{version}'\n")).unwrap();
        let mut permissions = std::fs::metadata(path).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(path, permissions).unwrap();
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
    fn install_resolves_a_saved_machine_declaration_and_its_kinds_by_label() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("config")).unwrap();
        std::fs::write(
            root.path().join("config/config.toml"),
            crate::remote::TEST_MACHINE,
        )
        .unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let runner = FakeRunner::new();
        runner.on(
            "machine list --json",
            ok(r#"[{"id":"example-machine","label":"buildbox","target":"saved-box","session":"default","enabled":true}]"#),
        );
        let ctx = Ctx {
            env: &env,
            root: root.path().join("root"),
            config_dir: root.path().join("config"),
            runner: &runner,
            detached_ticker: false,
        };

        let (profile, declaration) = install_box(&ctx, "buildbox").unwrap().unwrap();

        assert_eq!(profile.id, "example-machine");
        assert_eq!(profile.label, "buildbox");
        assert_eq!(profile.target, "saved-box");
        assert_eq!(declaration.id, "buildbox");
        assert_eq!(declaration.label, "buildbox");
        assert_eq!(declaration.build, "/home/agent/build/lanes");
        assert!(declaration.runs_kind("pi"));
        assert!(!declaration.runs_kind("claude"));
        assert!(!declaration.runs_kind("agy"));
    }

    #[test]
    fn box_zig_prefers_the_repository_local_tool() {
        let root = tempfile::tempdir().unwrap();
        let box_path = root.path().join("herdr");
        let local = box_path.join(".target/rebase/zig-0.16.0/zig");
        fake_zig(&local, "0.16.0");
        let bin = root.path().join("bin");
        fake_zig(&bin.join("zig"), "0.16.0");
        let out = resolve(&box_path, &bin);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            local.to_str().unwrap()
        );
    }

    #[test]
    fn box_zig_falls_back_to_the_box_path() {
        let root = tempfile::tempdir().unwrap();
        let box_path = root.path().join("herdr");
        std::fs::create_dir_all(&box_path).unwrap();
        let bin = root.path().join("bin");
        let zig = bin.join("zig");
        fake_zig(&zig, "0.16.0");
        let out = resolve(&box_path, &bin);
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            zig.to_str().unwrap()
        );
    }

    #[test]
    fn box_zig_missing_names_both_places() {
        let root = tempfile::tempdir().unwrap();
        let box_path = root.path().join("herdr");
        std::fs::create_dir_all(&box_path).unwrap();
        let empty = root.path().join("bin");
        std::fs::create_dir_all(&empty).unwrap();
        let out = resolve(&box_path, &empty);
        assert!(!out.status.success());
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(stderr.contains("harness_box_zig_missing"), "{stderr}");
        let local = box_path.join(".target/rebase/zig-0.16.0/zig");
        assert!(stderr.contains(local.to_str().unwrap()), "{stderr}");
        assert!(stderr.contains("PATH"), "{stderr}");
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
    fn box_build_for_the_fork_resolves_zig_on_the_box() {
        let root = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let runner = FakeRunner::new();
        runner.on("ssh", ok(""));
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
        let proofs = box_process_proofs(&ctx, &machine);
        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].machine, "lab");
        assert_eq!(proofs[0].state, "unknown");
        assert_eq!(proofs[0].build, None);
        assert!(
            proofs[0]
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("connection refused"))
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

    /// The box has util-linux `flock`; macOS does not. Give the local
    /// shell-driven box tests a real nonblocking lock command on macOS so a
    /// missing executable cannot masquerade as a held ticker lock.
    fn test_box_path(root: &Path) -> String {
        #[cfg(target_os = "macos")]
        {
            let flock = root.join("flock");
            std::fs::write(
                &flock,
                "#!/bin/sh\n[ \"$1\" = -n ] && [ \"$2\" = 9 ] || exit 2\nexec /usr/bin/perl -e 'open my $fd, \"+<&=9\" or exit 2; flock($fd, 2 | 4) or exit 1'\n",
            )
            .unwrap();
            let mut permissions = std::fs::metadata(&flock).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(&flock, permissions).unwrap();
            format!("{}:/usr/bin:/bin", root.display())
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = root;
            "/usr/bin:/bin".into()
        }
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
            format!("#!/bin/sh\nif [ \"$1\" = --version ]; then echo '{box_binary}'; fi\n"),
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

        let proofs = box_process_proofs(&ctx, &machine);

        assert_eq!(proofs.len(), 2);
        assert_eq!(proofs[0].state, "installed");
        assert_eq!(proofs[0].build.as_deref(), Some(box_binary.as_str()));
        assert_eq!(proofs[1].state, "running");
        assert_eq!(proofs[1].pid, Some(42));
        assert_eq!(proofs[1].build.as_deref(), Some(box_build.as_str()));

        drop(holder.stdin.take());
        assert!(holder.wait().unwrap().success());
        let proofs = box_process_proofs(&ctx, &machine);
        assert_eq!(proofs[1].state, "unknown");
        assert_eq!(proofs[1].pid, None);
    }

    #[test]
    fn a_box_ticker_without_a_valid_pid_cannot_pass_as_running() {
        let root = tempfile::tempdir().unwrap();
        let env = crate::paths::Env::for_test(root.path(), &[]);
        let runner = FakeRunner::new();
        runner.on(
            "ssh",
            ok(&format!(
                "HERDR_ADE_BOX_BINARY=herdr-ade {}\nHERDR_ADE_BOX_TICKER=0:{}\n",
                crate::VERSION,
                crate::VERSION
            )),
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
            ..Default::default()
        };
        let proofs = box_process_proofs(&ctx, &machine);
        assert_eq!(proofs[1].state, "unknown");
        assert!(require_running_tickers(&proofs, &["buildbox"]).is_err());
    }

    #[test]
    fn an_incomplete_box_ticker_record_is_unknown_not_empty_stale() {
        let root = tempfile::tempdir().unwrap();
        let bin = root.path().join("herdr-ade");
        std::fs::write(
            &bin,
            "#!/bin/sh\nif [ \"$1\" = --version ]; then echo 'herdr-ade 0.1.0+old.1'; fi\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&bin).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&bin, permissions).unwrap();
        let box_root = root.path().join("ade-root");
        std::fs::create_dir(&box_root).unwrap();
        std::fs::write(
            box_root.join(".ticker.lock"),
            "{\n  \"version\": \"0.1.0+old.1\"\n}",
        )
        .unwrap();
        let machine = crate::remote::MachineDeclaration {
            root: box_root.to_string_lossy().into_owned(),
            path: test_box_path(root.path()),
            ade_bin: bin.to_string_lossy().into_owned(),
            ..Default::default()
        };

        let out = RealRunner
            .run(
                &Cmd::new("sh", VERSION_TIMEOUT)
                    .args(["-c".into(), box_process_script(&machine, 2, "0.01")]),
            )
            .unwrap();

        assert!(out.success(), "{}", out.error_text());
        assert!(
            out.stdout.contains(
                "HERDR_ADE_BOX_TICKER_UNKNOWN=ticker lock did not contain a complete build record"
            ),
            "{}",
            out.stdout
        );
        assert!(!out.stdout.contains("HERDR_ADE_BOX_TICKER_STALE="));
    }

    #[test]
    fn installing_the_same_clean_commit_keeps_every_installed_inode() {
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
        let release = repo.join("target/release");
        let installed_dir = root.path().join(".local/bin");
        std::fs::create_dir_all(&release).unwrap();
        std::fs::create_dir_all(&installed_dir).unwrap();

        for bin in ["herdr-ade", "herdr-pi", "herdr-pro"] {
            let source = release.join(bin);
            let installed = installed_dir.join(bin);
            write_version_binary(&source, &format!("{bin} 0.1.0+abc1234.200"), "new stamp");
            write_version_binary(&installed, &format!("{bin} 0.1.0+abc1234.100"), "old stamp");
            let before = std::fs::metadata(&installed).unwrap().ino();

            local_install(&ctx, repo.to_str().unwrap(), bin, "abc1234", true).unwrap();

            assert_eq!(
                std::fs::metadata(&installed).unwrap().ino(),
                before,
                "{bin}"
            );
        }

        let source = release.join("herdr");
        let installed = installed_dir.join("herdr");
        write_version_binary(&source, "herdr 0.9.1", "rebuilt fork");
        write_version_binary(&installed, "herdr 0.9.1", "installed fork");
        std::fs::write(install_record(&installed_dir, "herdr"), "abc1234\n").unwrap();
        let before = std::fs::metadata(&installed).unwrap().ino();

        local_install(&ctx, repo.to_str().unwrap(), "herdr", "abc1234", true).unwrap();

        assert_eq!(std::fs::metadata(&installed).unwrap().ino(), before);
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

    #[test]
    fn the_installer_reexecutes_when_it_replaced_its_own_binary() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("herdr-ade");
        std::fs::write(&exe, b"old image").unwrap();
        let running = Running {
            path: std::fs::canonicalize(&exe).unwrap(),
            hash: crate::thread::sha256_hex(b"old image"),
        };

        assert!(!replaced_self(&exe, &running).unwrap());

        // The same invocation must continue in the new image rather than
        // refusing and asking for a second shell command.
        std::fs::write(&exe, b"new image").unwrap();
        assert!(replaced_self(&exe, &running).unwrap());

        // A sibling binary is not this process.
        let sibling = dir.path().join("herdr-pi");
        std::fs::write(&sibling, b"new image").unwrap();
        assert!(!replaced_self(&sibling, &running).unwrap());
    }

    #[test]
    fn install_evidence_lands_only_on_tasks_carried_by_the_build() {
        use crate::contracts::{MergeIntent, MergePhase, RoundPhase, RoundRecord};

        let world = crate::scenarios::World::new();
        let project = world.project("demo", "a.sock");
        let repo = world.home.path().join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        let (mut settings, body) = project.read_project_md().unwrap();
        settings.task_states = crate::task::STATES
            .iter()
            .map(|state| state.to_string())
            .collect();
        std::fs::write(
            project.project_md(),
            format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        let task_dir = project.state_dir().join("tasks");
        std::fs::create_dir_all(&task_dir).unwrap();
        for (id, round) in [("job-0001", "r1"), ("job-0002", "r2")] {
            let attempt = crate::thread::allocate(&project, |thread| {
                thread.repo = repo.to_string_lossy().into_owned();
                thread.base = "base".into();
            })
            .unwrap();
            let task = crate::task::Task {
                schema: 1,
                id: id.into(),
                title: format!("Install {id}."),
                authority: vec!["request:q-1".into()],
                acceptance: vec!["The installed build carries the change.".into()],
                attempts: vec![attempt.id],
                rounds: vec![round.into()],
                repo: Some(repo.to_string_lossy().into_owned()),
                created: crate::project::now(),
                ..crate::task::Task::default()
            };
            std::fs::write(
                task_dir.join(format!("{id}.toml")),
                toml::to_string(&task).unwrap(),
            )
            .unwrap();
        }
        let rounds = project.state_dir().join("rounds");
        std::fs::create_dir_all(&rounds).unwrap();
        for (id, head) in [("r1", "yes-head"), ("r2", "no-head")] {
            let round = RoundRecord {
                phase: RoundPhase::Merged,
                round: id.into(),
                branch: "main".into(),
                repo: repo.to_string_lossy().into_owned(),
                merge: Some(MergeIntent {
                    op: "op".into(),
                    expected_old: "old".into(),
                    candidate: "candidate".into(),
                    verdict: "verdict".into(),
                    phase: MergePhase::Checkpointed,
                    merged: Some(head.into()),
                    checkpoint: None,
                    head: Some(head.into()),
                }),
                ..RoundRecord::default()
            };
            std::fs::write(
                rounds.join(format!("{id}.toml")),
                toml::to_string(&round).unwrap(),
            )
            .unwrap();
        }
        // One ledger read selects the tasks, and one refresh renders all proofs.
        let events = project.record_dir_for_write("events").unwrap();
        let event = crate::contracts::Event {
            id: "t-0001-1-1".into(),
            op: "t-0001-1-1".into(),
            thread: "t-0001".into(),
            attempt: 1,
            round: None,
            recipient: crate::contracts::Recipient::default(),
            created: crate::project::now(),
            payload: crate::contracts::EventPayload::default(),
        };
        std::fs::write(
            events.join("t-0001-1-1.toml"),
            toml::to_string(&event).unwrap(),
        )
        .unwrap();
        world.runner.on("yes-head installed-head", ok(""));
        world.runner.on("no-head installed-head", fail(1, ""));
        let mut proofs = None;
        let reads = crate::events::count_event_reads(|| {
            proofs = Some(
                record_task_proofs(
                    &world.ctx(),
                    &[
                        InstalledBuild {
                            repo: repo.to_string_lossy().into_owned(),
                            machine: "local".into(),
                            head: "installed-head".into(),
                        },
                        InstalledBuild {
                            repo: "/unrelated/repository".into(),
                            machine: "buildbox".into(),
                            head: "other-head".into(),
                        },
                    ],
                    &[ProcessProof {
                        machine: "local".into(),
                        process: "ticker".into(),
                        pid: Some(42),
                        build: Some("installed-head".into()),
                        state: "running".into(),
                        reason: None,
                    }],
                )
                .unwrap(),
            )
        });
        assert_eq!(reads, 2, "one scan for selection and one for the page");
        let proofs = proofs.unwrap();

        assert_eq!(proofs.len(), 1);
        assert_eq!(proofs[0].task, "job-0001");
        let carried = crate::task::load(&project, "job-0001").unwrap();
        assert_eq!(carried.installed.len(), 1);
        assert_eq!(carried.running[0].machines, ["local"]);
        assert!(
            crate::task::load(&project, "job-0002")
                .unwrap()
                .installed
                .is_empty()
        );

        // If a later ancestry check fails, the first deferred write must
        // still be reflected in the page before the error is returned.
        world.runner.on("yes-head broken-head", ok(""));
        world.runner.on("no-head broken-head", fail(128, "bad git"));
        let mut result = None;
        let reads = crate::events::count_event_reads(|| {
            result = Some(record_task_proofs(
                &world.ctx(),
                &[InstalledBuild {
                    repo: repo.to_string_lossy().into_owned(),
                    machine: "buildbox".into(),
                    head: "broken-head".into(),
                }],
                &[],
            ));
        });
        assert!(result.unwrap().is_err());
        assert_eq!(reads, 2, "the partial write still gets one page rebuild");
        assert_eq!(
            crate::task::load(&project, "job-0001")
                .unwrap()
                .installed
                .len(),
            2
        );
    }
}
