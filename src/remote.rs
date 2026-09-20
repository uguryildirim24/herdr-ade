//! ssh, scp and the one quoting helper, plus the box provision call and the
//! courier's scp ingress options. No other code builds a string that a shell
//! will parse.

use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;

use crate::contracts::{BOX_REPOS, BoxRepoMap, MACHINE_LOCAL, MachineProfile};
use crate::runner::{Cmd, Output, Runner};

pub const SSH_TIMEOUT: Duration = Duration::from_secs(10);
pub const SSH_START_TIMEOUT: Duration = Duration::from_secs(25);
#[allow(dead_code)]
pub const COPY_TIMEOUT: Duration = Duration::from_secs(60);
const SSH_OPTIONS: [&str; 4] = ["-o", "ConnectTimeout=5", "-o", "BatchMode=yes"];

/// Single-quote escaping: safe for any value in an `sh` command string. Plain
/// words are left bare so printed commands stay readable and stable.
pub fn quote(value: &str) -> String {
    if is_plain(value) {
        value.to_string()
    } else {
        format!("'{}'", value.replace('\'', r"'\''"))
    }
}

/// Only characters that no shell, and neither scp nor rsync in any of their
/// remote-path modes, treat specially.
pub fn is_plain(value: &str) -> bool {
    !value.is_empty()
        && !value.starts_with('-')
        && value.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | ':' | '@' | '+' | ',')
        })
}

#[derive(Debug, Clone, Deserialize)]
struct SavedMachine {
    #[serde(default)]
    id: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    target: String,
    #[serde(default)]
    session: String,
    #[serde(default)]
    enabled: bool,
}

/// The stable profile of one saved machine (SPEC-remote §4.1). `local` is a
/// real profile with no SSH target.
pub fn machine_profile(
    runner: &dyn Runner,
    herdr_bin: &str,
    config_dir: &Path,
    machine: &str,
) -> Result<MachineProfile> {
    optional_machine_profile(runner, herdr_bin, config_dir, machine)?
        .with_context(|| format!("unknown_machine: `{machine}` is not a saved profile"))
}

/// The saved profile when it exists. A missing profile is distinct from a
/// failed or malformed machine list so callers never silently skip box work.
pub fn optional_machine_profile(
    runner: &dyn Runner,
    herdr_bin: &str,
    _config_dir: &Path,
    machine: &str,
) -> Result<Option<MachineProfile>> {
    if machine.is_empty() || machine == MACHINE_LOCAL {
        return Ok(Some(MachineProfile {
            id: MACHINE_LOCAL.into(),
            label: MACHINE_LOCAL.into(),
            target: String::new(),
            session: String::new(),
        }));
    }
    let Some(found) = saved_machines(runner, herdr_bin)?
        .into_iter()
        .find(|m| m.label == machine || m.id == machine)
    else {
        return Ok(None);
    };
    if !found.enabled {
        bail!("machine_disabled: `{}` is disabled", found.label);
    }
    if found.id.is_empty() || found.target.is_empty() || found.session.is_empty() {
        bail!("machine_profile_invalid: `{machine}` is incomplete");
    }
    Ok(Some(MachineProfile {
        id: found.id,
        label: found.label,
        target: found.target,
        session: found.session,
    }))
}

fn saved_machines(runner: &dyn Runner, herdr_bin: &str) -> Result<Vec<SavedMachine>> {
    let out = runner.run(&Cmd::new(herdr_bin, SSH_TIMEOUT).args(["machine", "list", "--json"]))?;
    if !out.success() {
        bail!("machine_list_failed: {}", out.error_text());
    }
    serde_json::from_str::<Vec<SavedMachine>>(&out.stdout)
        .context("machine_list_invalid: herdr returned invalid JSON")
}

/// Names of every enabled saved machine. An explicit `--machine` may place
/// work on any one of these, even when no current thread uses it.
pub fn registered_machine_names(runner: &dyn Runner, herdr_bin: &str) -> Result<Vec<String>> {
    saved_machines(runner, herdr_bin)?
        .into_iter()
        .filter(|machine| machine.enabled)
        .map(|machine| {
            if !machine.label.is_empty() {
                Ok(machine.label)
            } else if !machine.id.is_empty() {
                Ok(machine.id)
            } else {
                bail!("machine_profile_invalid: an enabled saved machine has no id or label")
            }
        })
        .collect()
}

/// The Mac→box row whose `mac` path is `mac_path` (SPEC-remote §4.1). The box
/// path is never derived from the Mac path.
pub fn box_repo_for(mac_path: &str) -> Option<&'static BoxRepoMap> {
    BOX_REPOS.iter().find(|row| row.mac == mac_path)
}

/// The URL-matched remote name in `repo`, never by remote name alone. The
/// second lane's courier calls this to fetch the lane commit (SPEC-remote
/// §4.3); the start side pushes by URL directly.
pub fn remote_for_url(runner: &dyn Runner, repo: &str, url: &str) -> Result<String> {
    let out = runner.run(&Cmd::new("git", SSH_TIMEOUT).args(["-C", repo, "remote"]))?;
    if !out.success() {
        bail!("git remote in {repo}: {}", out.error_text());
    }
    for name in out.stdout.lines().map(str::trim).filter(|l| !l.is_empty()) {
        let got = runner
            .run(&Cmd::new("git", SSH_TIMEOUT).args(["-C", repo, "remote", "get-url", name]))?;
        let got = got.stdout.trim().to_string();
        if same_url(&got, url) {
            return Ok(name.to_string());
        }
    }
    bail!("no_url_remote: {repo} has no remote whose URL is {url}")
}

/// A remote URL reduced to the repository it names: surrounding whitespace,
/// every trailing `/` and every trailing `.git` removed. `https://…/repo` and
/// `https://…/repo.git` are the same remote, however it was written down.
pub fn normalize_url(url: &str) -> String {
    url.trim()
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .to_string()
}

fn same_url(a: &str, b: &str) -> bool {
    normalize_url(a) == normalize_url(b)
}

/// The box-side twin of [`normalize_url`], as a shell function the provision
/// script calls on both the wanted URL and every remote it finds. A test runs
/// this exact snippet against the Rust function, so the two cannot drift.
const NORM_URL_SH: &str = "\
norm() {\n\
u=$1\n\
while [ \"${u#[[:space:]]}\" != \"$u\" ]; do u=${u#[[:space:]]}; done\n\
while [ \"${u%[[:space:]]}\" != \"$u\" ]; do u=${u%[[:space:]]}; done\n\
while [ \"${u%/}\" != \"$u\" ]; do u=${u%/}; done\n\
while [ \"${u%.git}\" != \"$u\" ]; do u=${u%.git}; done\n\
printf '%s' \"$u\"\n\
}\n";

fn check_target(target: &str) -> Result<()> {
    // A target is `user@host` or a host alias; it must never look like an option.
    if target.is_empty()
        || target.starts_with('-')
        || target.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        bail!("`{target}` is not a usable SSH target");
    }
    Ok(())
}

/// Runs `script` on the machine with `sh -c`. The script is one argument; every
/// value inside it must already have gone through `quote`.
pub fn ssh(
    runner: &dyn Runner,
    target: &str,
    script: &str,
    stdin: Option<&str>,
    timeout: Duration,
) -> Result<Output> {
    check_target(target)?;
    let script = crate::contracts::with_box_path(script);
    let mut cmd = Cmd::new("ssh", timeout).args(SSH_OPTIONS).args([
        "--",
        target,
        &format!("sh -c {}", quote(&script)),
    ]);
    if let Some(text) = stdin {
        cmd = cmd.stdin(text);
    }
    runner.run(&cmd)
}

/// One box start's git effect (SPEC-remote §4.2 step 3): the box fetches the
/// lane branch, verifies `FETCH_HEAD = B`, and creates the worktree from it
/// under the box clone. One SSH call; nothing is copied.
pub struct Provision<'a> {
    pub box_repo: &'a str,
    pub worktree: &'a str,
    pub branch: &'a str,
    pub base: &'a str,
    pub publish_url: &'a str,
}

pub fn provision(runner: &dyn Runner, target: &str, req: &Provision<'_>) -> Result<()> {
    let script = format!(
        "set -e\n\
         cd {repo} || exit 3\n\
         git rev-parse --show-toplevel >/dev/null || exit 3\n\
         {norm}\
         wanted=$(norm {url})\n\
         matched=\n\
         found=\n\
         for name in $(git remote); do\n\
           actual=$(git remote get-url \"$name\")\n\
           found=\"$found $actual\"\n\
           if [ \"$(norm \"$actual\")\" = \"$wanted\" ]; then matched=1; fi\n\
         done\n\
         test -n \"$matched\" || {{ printf 'box_clone_url_mismatch: wanted %s; box has:%s\\n' {url} \"$found\" >&2; exit 4; }}\n\
         git fetch --quiet {url} {branch} || exit 4\n\
         test \"$(git rev-parse FETCH_HEAD)\" = {base} || {{ echo fetch_head_mismatch >&2; exit 5; }}\n\
         if [ -e {wt} ]; then\n\
           test -d {wt} || {{ echo worktree_not_directory >&2; exit 6; }}\n\
         elif git show-ref --verify --quiet {ref}; then\n\
           test \"$(git rev-parse {ref})\" = {base} || {{ echo local_branch_mismatch >&2; exit 6; }}\n\
           git worktree add --quiet {wt} {branch}\n\
         else\n\
           git worktree add --quiet -b {branch} {wt} FETCH_HEAD\n\
         fi\n\
         test \"$(git -C {wt} rev-parse HEAD)\" = {base} || {{ echo worktree_head_mismatch >&2; exit 7; }}\n\
         git -C {wt} rev-parse HEAD\n",
        norm = NORM_URL_SH,
        repo = quote(req.box_repo),
        wt = quote(req.worktree),
        branch = quote(req.branch),
        base = quote(req.base),
        url = quote(req.publish_url),
        ref = quote(&format!("refs/heads/{}", req.branch)),
    );
    let out = ssh(runner, target, &script, None, SSH_START_TIMEOUT)?;
    if !out.success() {
        bail!("box provision on {target} failed: {}", out.error_text());
    }
    if out.stdout.trim() != req.base {
        bail!(
            "box provision on {target} returned {}, expected {}",
            out.stdout.trim(),
            req.base
        );
    }
    Ok(())
}

/// The box's lane card: created after the pane id exists (SPEC-remote §4.2
/// step 5). One SSH call, card bytes on stdin. A minimal `PROJECT.md` is
/// written when the box has none, so the box's own `ha` can resolve it.
pub fn provision_card(
    runner: &dyn Runner,
    target: &str,
    slug: &str,
    path: &str,
    card: &str,
) -> Result<()> {
    let lanes_dir = path.rsplit_once('/').map_or("", |(d, _)| d);
    let project_dir = lanes_dir.rsplit_once('/').map_or(lanes_dir, |(d, _)| d);
    let project_md = format!("{project_dir}/PROJECT.md");
    let state_dir = format!("{project_dir}/.state");
    let script = format!(
        "set -e\n\
         mkdir -p \"$(dirname {path})\"\n\
         mkdir -p {state_dir}\n\
         cat > {path}\n\
         pm={pm}\n\
         if [ ! -f \"$pm\" ]; then\n\
           printf '+++\\nname = \"%s\"\\n+++\\n' {slug} > \"$pm\"\n\
         fi",
        path = quote(path),
        state_dir = quote(&state_dir),
        pm = quote(&project_md),
        slug = quote(slug),
    );
    let out = ssh(runner, target, &script, Some(card), SSH_START_TIMEOUT)?;
    if !out.success() {
        bail!(
            "could not write the lane card on {target}: {}",
            out.error_text()
        );
    }
    Ok(())
}

/// The courier's helper call over its multiplexed connection (SPEC-remote
/// §4.3): the box-local helper runs `sh -c <script>` and the following
/// `scp` reuses the same control socket.
pub fn ssh_courier(
    runner: &dyn Runner,
    target: &str,
    control_dir: &Path,
    script: &str,
    cursor: &str,
    timeout: Duration,
) -> Result<Output> {
    check_target(target)?;
    let mut args = multiplex_options(control_dir);
    args.extend(SSH_OPTIONS.iter().map(|s| (*s).to_string()));
    let script = crate::contracts::with_box_path(script);
    args.push("--".into());
    args.push(target.to_string());
    args.push(format!("sh -c {}", quote(&script)));
    runner.run(&Cmd::new("ssh", timeout).args(args).stdin(cursor))
}

/// The courier's multiplexing options (SPEC-remote §4.3): one SSH handshake per
/// pass. The courier passes these on its helper and `scp` calls.
pub fn multiplex_options(control_dir: &Path) -> Vec<String> {
    let dir = control_dir.join("ssh");
    let _ = std::fs::create_dir_all(&dir);
    vec![
        "-o".into(),
        "ControlMaster=auto".into(),
        "-o".into(),
        format!("ControlPath={}/%C", dir.display()),
        "-o".into(),
        "ControlPersist=10".into(),
    ]
}

/// Copies one remote file to a local path with `scp`. A path scp cannot carry
/// unchanged in every mode (spaces, quotes, globs) is fetched with `ssh cat`
/// through the quoting helper instead. The second lane's ingress calls this.
#[allow(dead_code)]
pub fn fetch_file(
    runner: &dyn Runner,
    target: &str,
    remote_path: &str,
    local_path: &Path,
) -> Result<()> {
    check_target(target)?;
    if is_plain(remote_path) {
        let out = runner.run(&Cmd::new("scp", COPY_TIMEOUT).args(SSH_OPTIONS).args([
            "-q",
            "--",
            &format!("{target}:{remote_path}"),
            &local_path.to_string_lossy(),
        ]))?;
        if !out.success() {
            bail!("scp from {target}: {}", out.error_text());
        }
        return Ok(());
    }
    let out = ssh(
        runner,
        target,
        &format!("cat -- {}", quote(remote_path)),
        None,
        COPY_TIMEOUT,
    )?;
    if !out.success() {
        bail!("ssh {target} cat: {}", out.error_text());
    }
    std::fs::write(local_path, out.stdout.as_bytes())?;
    Ok(())
}

/// The courier's batched `scp` over its multiplexed connection (SPEC-remote
/// §4.3): every plain path in one call. A path scp cannot carry safely is
/// refused here and fetched with [`fetch_file`]. The second lane's courier
/// calls this; the start side never does.
#[allow(dead_code)]
pub fn fetch_batch(
    runner: &dyn Runner,
    target: &str,
    control_dir: &Path,
    remote_paths: &[String],
    local_dir: &Path,
) -> Result<()> {
    check_target(target)?;
    if remote_paths.is_empty() {
        return Ok(());
    }
    if let Some(bad) = remote_paths.iter().find(|path| !is_plain(path)) {
        bail!("the box path `{bad}` has characters scp cannot carry safely");
    }
    std::fs::create_dir_all(local_dir)?;
    let mut args = multiplex_options(control_dir);
    args.extend(SSH_OPTIONS.iter().map(|s| (*s).to_string()));
    args.push("-q".into());
    args.push("--".into());
    for path in remote_paths {
        args.push(format!("{target}:{path}"));
    }
    args.push(format!("{}/", local_dir.display()));
    let out = runner.run(&Cmd::new("scp", COPY_TIMEOUT).args(args))?;
    if !out.success() {
        bail!("scp from {target}: {}", out.error_text());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runner::RealRunner;
    use crate::runner::fake::{FakeRunner, fail, ok};

    #[test]
    fn plain_words_stay_bare() {
        assert_eq!(quote("/Users/me/.dev-root"), "/Users/me/.dev-root");
        assert_eq!(quote(""), "''");
        assert_eq!(quote("a b"), "'a b'");
        assert_eq!(quote("it's"), r"'it'\''s'");
        assert_eq!(quote("-n"), "'-n'");
    }

    const HOSTILE: [&str; 10] = [
        "$(touch /tmp/hp-pwned)",
        "`id`",
        "a'; rm -rf ~; echo '",
        "x\ny",
        "~/x",
        "-n",
        "a\\b\"c",
        "*",
        "!!",
        "a b  c",
    ];

    #[test]
    fn hostile_values_survive_a_real_shell_unchanged() {
        for hostile in HOSTILE {
            let out = RealRunner
                .run(
                    &Cmd::new("sh", Duration::from_secs(5))
                        .args(["-c".to_string(), format!("printf %s {}", quote(hostile))]),
                )
                .unwrap();
            assert_eq!(out.stdout, hostile);
        }
    }

    #[test]
    fn hostile_values_survive_the_double_shell_of_an_ssh_command() {
        // ssh hands its argument to the remote login shell, which runs our
        // `sh -c <quoted script>`: two layers of parsing. `sh -c` stands in for ssh.
        for hostile in HOSTILE {
            let script = format!("printf %s {}", quote(hostile));
            let remote_command = format!("sh -c {}", quote(&script));
            let out = RealRunner
                .run(&Cmd::new("sh", Duration::from_secs(5)).args(["-c", &remote_command]))
                .unwrap();
            assert_eq!(out.stdout, hostile, "{remote_command}");
        }
    }

    #[test]
    fn the_card_script_works_against_a_real_directory_with_a_hostile_path() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("it's a $(box)");
        let card = dir.join("lanes/t-0001.toml");
        let card_s = card.to_string_lossy().into_owned();
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
        provision_card(&runner, "box", "demo", &card_s, "thread = \"t-0001\"\n").unwrap();
        assert_eq!(
            std::fs::read_to_string(&card).unwrap(),
            "thread = \"t-0001\"\n"
        );
        let project = std::fs::read_to_string(dir.join("PROJECT.md")).unwrap();
        assert!(project.contains("name = \"demo\""), "{project}");
    }

    #[test]
    fn ssh_always_uses_batch_mode_a_connect_timeout_and_a_separator() {
        let runner = FakeRunner::new();
        runner.on("ssh", ok(""));
        ssh(&runner, "user@host", "true", None, SSH_TIMEOUT).unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(
            &calls[0].args[..6],
            [
                "-o",
                "ConnectTimeout=5",
                "-o",
                "BatchMode=yes",
                "--",
                "user@host"
            ]
        );
        drop(calls);
        assert!(ssh(&runner, "-oProxyCommand=evil", "true", None, SSH_TIMEOUT).is_err());
        assert!(ssh(&runner, "host; rm -rf ~", "true", None, SSH_TIMEOUT).is_err());
    }

    #[test]
    fn every_ssh_script_carries_the_box_path() {
        let expected = format!(
            "sh -c 'PATH={}; export PATH\ntrue'",
            crate::contracts::BOX_PATH
        );
        let runner = FakeRunner::new();
        runner.on("ssh", ok(""));
        ssh(&runner, "box", "true", None, SSH_TIMEOUT).unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(calls[0].args.last().unwrap(), &expected);
        drop(calls);

        let runner = FakeRunner::new();
        runner.on("ssh", ok(""));
        let dir = tempfile::tempdir().unwrap();
        ssh_courier(&runner, "box", dir.path(), "true", "", SSH_TIMEOUT).unwrap();
        let calls = runner.calls.borrow();
        assert_eq!(calls[0].args.last().unwrap(), &expected);
        drop(calls);
    }

    #[test]
    fn machine_profiles_come_only_from_enabled_saved_machines() {
        let config = tempfile::tempdir().unwrap();
        let runner = FakeRunner::new();
        runner.on(
            "machine list --json",
            ok(r#"[{"id":"abc","label":"m1","target":"m1.local","session":"default","enabled":true}]"#),
        );
        let profile = machine_profile(&runner, "herdr", config.path(), "m1").unwrap();
        assert_eq!(profile.id, "abc");
        assert_eq!(profile.label, "m1");
        assert_eq!(profile.target, "m1.local");
        assert_eq!(profile.session, "default");
        assert!(
            machine_profile(&runner, "herdr", config.path(), "local")
                .unwrap()
                .is_local()
        );
        assert!(machine_profile(&runner, "herdr", config.path(), "box").is_err());

        let broken = FakeRunner::new();
        broken.on("machine list --json", fail(1, "no"));
        assert!(machine_profile(&broken, "herdr", config.path(), "box").is_err());
    }

    #[test]
    fn the_box_repo_map_is_path_exact_and_the_url_remote_is_chosen_by_url() {
        let row = box_repo_for("/Users/rolfie/projects/herdr").unwrap();
        assert_eq!(row.box_path, "/home/ubuntu/projects/herdr");
        assert!(box_repo_for("/Users/rolfie/projects/herdr-ade").is_some());
        assert!(box_repo_for("/Users/rolfie/projects/other").is_none());

        let runner = FakeRunner::new();
        runner.on(
            "git -C /repo remote get-url fork",
            ok("https://github.com/uguryildirim24/herdr.git\n"),
        );
        runner.on(
            "git -C /repo remote get-url origin",
            ok("https://github.com/herdrdev/herdr.git\n"),
        );
        runner.on("git -C /repo remote", ok("fork\norigin\n"));
        assert_eq!(
            remote_for_url(
                &runner,
                "/repo",
                "https://github.com/uguryildirim24/herdr.git"
            )
            .unwrap(),
            "fork"
        );
    }

    #[test]
    fn provision_verifies_fetch_head_and_creates_the_worktree() {
        let runner = FakeRunner::new();
        runner.on("ssh", ok("b0b0\n"));
        let req = Provision {
            box_repo: "/home/ubuntu/projects/herdr",
            worktree: "/home/ubuntu/projects/herdr/.worktrees/t-0001",
            branch: "hp/demo/t-0001",
            base: "b0b0",
            publish_url: "https://github.com/uguryildirim24/herdr.git",
        };
        provision(&runner, "box", &req).unwrap();
        let calls = runner.calls.borrow();
        let script = calls[0].args.last().unwrap();
        assert!(script.contains("git fetch --quiet"));
        assert!(script.contains("FETCH_HEAD"));
        assert!(script.contains("git worktree add"));
        drop(calls);
    }

    #[test]
    fn the_box_shell_url_normalization_matches_the_mac_rule() {
        let urls = [
            "https://github.com/user/repo",
            "https://github.com/user/repo.git",
            "https://github.com/user/repo/",
            "https://github.com/user/repo.git/",
            "https://github.com/user/repo.git.git",
            "  https://github.com/user/repo.git  ",
            "git@github.com:user/repo.git",
            "a/b/.git",
        ];
        for url in urls {
            let script = format!("{}\nprintf '%s\\n' \"$(norm {})\"", NORM_URL_SH, quote(url));
            let out = RealRunner
                .run(&Cmd::new("sh", Duration::from_secs(5)).args(["-c".to_string(), script]))
                .unwrap();
            assert_eq!(
                out.stdout.trim_end_matches('\n'),
                normalize_url(url),
                "for {url}"
            );
        }
    }

    /// A box fake for the provision script: run it with the local `sh` instead
    /// of over ssh, so a real git clone on disk exercises the real script.
    fn run_ssh_locally(runner: &FakeRunner) {
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
    }

    #[test]
    fn provision_accepts_a_git_suffix_difference_and_names_both_urls_on_mismatch() {
        let root = tempfile::tempdir().unwrap();
        let git = |args: &[&str], cwd: &Path| {
            let out = RealRunner
                .run(
                    &Cmd::new("git", Duration::from_secs(10))
                        .args(args.iter().map(|arg| arg.to_string()))
                        .cwd(cwd),
                )
                .unwrap();
            assert!(out.success(), "git {args:?}: {}", out.error_text());
            out.stdout
        };
        let origin = root.path().join("origin.git");
        std::fs::create_dir_all(&origin).unwrap();
        git(&["init", "-q", "--bare"], &origin);
        let work = root.path().join("work");
        git(
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                work.to_str().unwrap(),
            ],
            root.path(),
        );
        git(&["config", "user.email", "lane@example.invalid"], &work);
        git(&["config", "user.name", "lane"], &work);
        std::fs::write(work.join("file"), "content").unwrap();
        git(&["add", "file"], &work);
        git(&["commit", "-qm", "init"], &work);
        git(&["branch", "-M", "lane"], &work);
        git(&["push", "-q", "origin", "lane"], &work);
        let base = git(&["rev-parse", "HEAD"], &work).trim().to_string();

        // A URL without `.git` reaches the same bare repository.
        let short = root.path().join("origin");
        std::os::unix::fs::symlink("origin.git", &short).unwrap();

        // The box clone's remote keeps the `.git` spelling.
        let box_clone = root.path().join("box");
        git(
            &[
                "clone",
                "-q",
                origin.to_str().unwrap(),
                box_clone.to_str().unwrap(),
            ],
            root.path(),
        );
        git(
            &["remote", "set-url", "origin", origin.to_str().unwrap()],
            &box_clone,
        );

        let worktree = box_clone.join(".worktrees/t-0001");
        let runner = FakeRunner::new();
        run_ssh_locally(&runner);
        provision(
            &runner,
            "box",
            &Provision {
                box_repo: box_clone.to_str().unwrap(),
                worktree: worktree.to_str().unwrap(),
                branch: "lane",
                base: &base,
                publish_url: short.to_str().unwrap(),
            },
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(worktree.join("file")).unwrap(),
            "content"
        );

        // A different URL fails, naming the wanted URL and the one the box has.
        let runner = FakeRunner::new();
        run_ssh_locally(&runner);
        // Keep the wanted URL hostile enough to prove that printing the
        // diagnostic does not evaluate URL contents as shell syntax.
        let wanted = root.path().join("wanted $(printf PWNED >&2).git");
        let error = provision(
            &runner,
            "box",
            &Provision {
                box_repo: box_clone.to_str().unwrap(),
                worktree: worktree.to_str().unwrap(),
                branch: "lane",
                base: &base,
                publish_url: wanted.to_str().unwrap(),
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("box_clone_url_mismatch"), "{error}");
        assert!(error.contains(wanted.to_str().unwrap()), "{error}");
        assert!(error.contains(origin.to_str().unwrap()), "{error}");
    }

    #[test]
    fn unsafe_remote_paths_never_reach_scp() {
        let runner = FakeRunner::new();
        runner.on("ssh", ok("file body"));
        runner.on("scp", ok(""));
        let dir = tempfile::tempdir().unwrap();
        fetch_file(
            &runner,
            "box",
            "/wt/my repo/report.md",
            &dir.path().join("r"),
        )
        .unwrap();
        assert_eq!(runner.count("scp"), 0);
        assert_eq!(
            std::fs::read_to_string(dir.path().join("r")).unwrap(),
            "file body"
        );
        fetch_file(&runner, "box", "/wt/repo/report.md", &dir.path().join("r2")).unwrap();
        assert_eq!(runner.count("scp"), 1);
        assert!(
            fetch_batch(
                &runner,
                "box",
                dir.path(),
                &["/wt/my repo/report.md".into()],
                dir.path()
            )
            .is_err()
        );
        assert_eq!(runner.count("scp"), 1);
    }
}
