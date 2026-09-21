//! The harness repositories and `ha harness install` (t-0054).
//!
//! The list lives once, in `[harness]` in `config.toml`: each row is a
//! `path`/`box_path` pair, the same shape a project's `repos` rows have. Every
//! project may start lanes and open rounds on a harness repository without
//! listing it in `PROJECT.md`.

use std::path::{Path, PathBuf};
use std::time::Duration;

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
pub(crate) const BOX_WORKER_MARKER: &str = ".lane-worker";

/// The `[harness]` table of `config.toml`.
#[derive(Debug, Default, Deserialize)]
struct RawConfig {
    #[serde(default)]
    harness: HarnessConfig,
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
pub(crate) fn is_harness_repo(config_dir: &Path, path: &str) -> bool {
    let target = canonical_or(path);
    repos(config_dir)
        .unwrap_or_default()
        .iter()
        .any(|repo| canonical_or(&repo.path) == target)
}

/// True when a project may start a lane or open a round on `path`: the path is
/// one of its own listed repositories, or a harness repository.
pub(crate) fn allowed_repo(settings: &Settings, config_dir: &Path, path: &str) -> bool {
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
    pub(crate) box_path: Option<String>,
    pub(crate) box_installed: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(crate) struct InstallOutcome {
    pub(crate) repositories: Vec<InstalledRepo>,
    pub(crate) box_target: Option<String>,
    pub(crate) box_settings_installed: bool,
    pub(crate) live_handoff_required: bool,
    #[serde(skip)]
    warnings: Vec<String>,
}

impl InstallOutcome {
    pub(crate) fn message(&self) -> String {
        let mut message = self
            .repositories
            .iter()
            .flat_map(|repo| repo.binaries.iter())
            .map(|binary| format!("{}\n", binary.version))
            .collect::<String>();
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

fn local_install(ctx: &Ctx, repo: &str, bin: &str) -> Result<()> {
    let dir = ctx.env.home.join(".local/bin");
    std::fs::create_dir_all(&dir)?;
    let from = Path::new(repo).join("target/release").join(bin);
    let to = dir.join(bin);
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
) -> Result<()> {
    let zig = if kind == Kind::Fork {
        format!("\n{}", box_zig_script(box_path))
    } else {
        String::new()
    };
    let mut installs = String::new();
    for bin in kind.binaries() {
        installs.push_str(&format!(
            "\ninstall_to={to}\n\
             install_tmp=\"${{install_to}}.install.$$\"\n\
             mkdir -p \"$(dirname \"$install_to\")\"\n\
             cp target/release/{bin} \"$install_tmp\"\n\
             chmod 755 \"$install_tmp\"\n\
             mv -f \"$install_tmp\" \"$install_to\"",
            to = remote::quote(&box_binary(machine, bin)?),
        ));
    }
    let script = format!(
        "set -e\n\
         cd {path}\n\
         git fetch --quiet\n\
         git merge --ff-only @{{u}}\n\
         export PATH={build_path}\n\
         export DEVELOPER_DIR={DEVELOPER_DIR}{zig}\n\
         cargo build --release --locked{installs}",
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
    Ok(())
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
        let path = std::fs::canonicalize(&current)
            .with_context(|| format!("could not resolve {}", current.display()))?;
        let bytes = std::fs::read(&path)
            .with_context(|| format!("could not fingerprint {}", path.display()))?;
        Ok(Running {
            path,
            hash: crate::thread::sha256_hex(&bytes),
        })
    }
}

/// `ha harness install` builds and installs the plugin, then keeps running the
/// image it started as. When the file just installed is this process's own
/// executable and its bytes changed, the rest of this run would still use the
/// old installer logic: stop and name the command that picks up the new binary.
fn notice_stale_self(installed: &Path, running: &Running) -> Result<()> {
    let same = std::fs::canonicalize(installed).is_ok_and(|path| path == running.path);
    if !same {
        return Ok(());
    }
    let bytes = std::fs::read(installed)
        .with_context(|| format!("could not read {}", installed.display()))?;
    if crate::thread::sha256_hex(&bytes) == running.hash {
        return Ok(());
    }
    Err(crate::refusal::error(format!(
        "harness_install_stale_self: this run installed a newer {} but is still the old process; run `ha harness install` again",
        installed.display()
    )))
}

/// `ha harness install`: build every harness repository after a merge and
/// install it into `~/.local/bin`, then the same on the saved box.
pub(crate) fn install(ctx: &Ctx) -> Result<InstallOutcome> {
    let repos = repos(&ctx.config_dir)?;
    if repos.is_empty() {
        bail!(
            "harness_repos_missing: add `[harness] repos` to {}",
            ctx.config_dir.join("config.toml").display()
        );
    }
    let _lock = lock(&ctx.config_dir)?;
    let running = Running::capture()?;
    let config_text =
        std::fs::read_to_string(ctx.config_dir.join("config.toml")).unwrap_or_default();
    let dispatch = toml::from_str::<RawConfig>(&config_text)
        .context("config.toml does not parse")?
        .dispatch
        .machine;
    let box_profile = if dispatch.is_empty() || dispatch == crate::contracts::MACHINE_LOCAL {
        None
    } else {
        remote::optional_machine_profile(
            ctx.runner,
            &ctx.env.herdr_bin(),
            &ctx.config_dir,
            &dispatch,
        )?
    };
    let box_target = box_profile.as_ref().map(|profile| profile.target.clone());
    let box_paths = box_profile
        .as_ref()
        .map(|profile| remote::machine_declaration(&ctx.config_dir, &profile.label))
        .transpose()?;
    let mut fork = false;
    let mut installed = Vec::new();
    let mut warnings = Vec::new();
    for repo in &repos {
        let kind = kind(&repo.path)?;
        fork |= kind == Kind::Fork;
        local_build(ctx, &repo.path, kind)?;
        for bin in kind.binaries() {
            local_install(ctx, &repo.path, bin)?;
            notice_stale_self(&ctx.env.home.join(".local/bin").join(bin), &running)?;
        }
        let mut binaries = Vec::new();
        for bin in kind.binaries() {
            binaries.push(installed_version(ctx, bin)?);
        }
        let box_installed = match (&box_target, &repo.box_path) {
            (Some(target), Some(box_path)) => {
                box_build(
                    ctx,
                    target,
                    box_paths
                        .as_ref()
                        .context("machine path declaration is missing")?,
                    box_path,
                    kind,
                )?;
                true
            }
            (Some(_), None) => {
                warnings.push(format!(
                    "note: {} has no box_path; skipped the box step",
                    repo.path
                ));
                false
            }
            (None, _) => false,
        };
        installed.push(InstalledRepo {
            path: repo.path.clone(),
            kind: kind.name().into(),
            binaries,
            box_path: repo.box_path.clone(),
            box_installed,
        });
    }
    if let (Some(target), Some(machine)) = (&box_target, &box_paths) {
        box_settings(ctx, target, machine)?;
    }
    Ok(InstallOutcome {
        repositories: installed,
        box_settings_installed: box_target.is_some(),
        box_target,
        live_handoff_required: fork,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::fake::{FakeRunner, ok};
    use std::os::unix::fs::PermissionsExt;

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
            "/home/ubuntu/projects/herdr",
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
            !script.contains("ZIG=/home/ubuntu/projects/herdr/.target"),
            "{script}"
        );
    }

    #[test]
    fn the_installer_notices_it_replaced_its_own_binary() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("herdr-ade");
        std::fs::write(&exe, b"old image").unwrap();
        let running = Running {
            path: std::fs::canonicalize(&exe).unwrap(),
            hash: crate::thread::sha256_hex(b"old image"),
        };

        // The installed bytes are unchanged: nothing to report.
        notice_stale_self(&exe, &running).unwrap();

        // The install replaced this process's own file: refuse to continue on
        // the old image and name the command that runs the new one.
        std::fs::write(&exe, b"new image").unwrap();
        let error = notice_stale_self(&exe, &running).unwrap_err();
        assert!(crate::refusal::is(&error));
        let message = error.to_string();
        assert!(message.contains("harness_install_stale_self"), "{message}");
        assert!(message.contains("ha harness install"), "{message}");

        // A sibling binary is not this process.
        let sibling = dir.path().join("herdr-pi");
        std::fs::write(&sibling, b"new image").unwrap();
        notice_stale_self(&sibling, &running).unwrap();
    }
}
