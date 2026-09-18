//! The command seam of the pi module.
//!
//! `src/pi/` compiles into two targets: the `herdr-ade` binary (`mod pi`) and
//! the thin `herdr-pi` binary (`#[path = "../pi/mod.rs"]`). It therefore uses
//! no `crate::` paths. This file is the small `Runner` seam over external
//! commands, with the scripted fake the tests drive (SPEC-pi v2 §7: the same
//! shape as `crate::runner::fake`, kept separate so the second binary builds).

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// The default timeout for a short read-only command (node, pi, herdr).
pub const SHORT: Duration = Duration::from_secs(10);
/// npm and `herdr integration install` can take a while; setup uses this.
pub const SETUP: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq)]
pub struct Cmd {
    pub program: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub cwd: Option<PathBuf>,
    /// `None` closes stdin (`</dev/null` for every doctor and check run).
    pub stdin: Option<String>,
    pub timeout: Duration,
}

impl Cmd {
    pub fn new(program: impl Into<String>, timeout: Duration) -> Self {
        Cmd {
            program: program.into(),
            args: Vec::new(),
            env: Vec::new(),
            env_remove: Vec::new(),
            cwd: None,
            stdin: None,
            timeout,
        }
    }

    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub fn stdin(mut self, text: impl Into<String>) -> Self {
        self.stdin = Some(text.into());
        self
    }

    /// The command as one line; the scripted fake matches on it.
    pub fn display(&self) -> String {
        let mut line = self.program.clone();
        for arg in &self.args {
            line.push(' ');
            line.push_str(arg);
        }
        line
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Output {
    /// `None` when the process was killed (timeout or signal).
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

impl Output {
    pub fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }

    /// stderr when it has text, else stdout, trimmed; for error messages.
    pub fn error_text(&self) -> String {
        if self.timed_out {
            return "timed out".to_string();
        }
        let text = if self.stderr.trim().is_empty() {
            self.stdout.trim()
        } else {
            self.stderr.trim()
        };
        text.to_string()
    }
}

pub trait Runner {
    /// `Err` means the command could not be spawned at all (for example the
    /// program is missing). A non-zero exit or a timeout is an `Ok(Output)`.
    fn run(&self, cmd: &Cmd) -> Result<Output>;
}

pub struct RealRunner;

impl Runner for RealRunner {
    fn run(&self, cmd: &Cmd) -> Result<Output> {
        let mut command = Command::new(&cmd.program);
        command.args(&cmd.args);
        for key in &cmd.env_remove {
            command.env_remove(key);
        }
        for (key, value) in &cmd.env {
            command.env(key, value);
        }
        if let Some(cwd) = &cmd.cwd {
            command.current_dir(cwd);
        }
        command
            .stdin(if cmd.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        // Every command runs in its own process group, so a timeout reaches
        // what `zsh -lic` or npm started too (same rule as `crate::runner`).
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("could not run `{}`", cmd.program))?;

        let stdin_thread = child
            .stdin
            .take()
            .zip(cmd.stdin.clone())
            .map(|(mut pipe, text)| {
                std::thread::spawn(move || {
                    let _ = pipe.write_all(text.as_bytes());
                })
            });
        let stdout_thread = child.stdout.take().map(read_all);
        let stderr_thread = child.stderr.take().map(read_all);

        let deadline = Instant::now() + cmd.timeout;
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if Instant::now() >= deadline {
                timed_out = true;
                kill_group(&mut child);
                break child.wait().ok();
            }
            std::thread::sleep(Duration::from_millis(20));
        };

        if let Some(thread) = stdin_thread {
            let _ = thread.join();
        }
        let stdout = stdout_thread.map(join_text).unwrap_or_default();
        let stderr = stderr_thread.map(join_text).unwrap_or_default();

        Ok(Output {
            code: if timed_out {
                None
            } else {
                status.and_then(|s| s.code())
            },
            stdout,
            stderr,
            timed_out,
        })
    }
}

/// The child leads its own group, so its pid is the pgid. Grandchildren hold
/// the pipes open; killing only the child would leave the readers hanging.
fn kill_group(child: &mut std::process::Child) {
    for signal in ["-TERM", "-KILL"] {
        let _ = Command::new("/bin/kill")
            .args([signal, "--", &format!("-{}", child.id())])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        if signal == "-TERM" {
            std::thread::sleep(Duration::from_millis(200));
        }
    }
    let _ = child.kill();
}

fn read_all<R: Read + Send + 'static>(mut pipe: R) -> std::thread::JoinHandle<Vec<u8>> {
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = pipe.read_to_end(&mut buf);
        buf
    })
}

fn join_text(thread: std::thread::JoinHandle<Vec<u8>>) -> String {
    String::from_utf8_lossy(&thread.join().unwrap_or_default()).into_owned()
}

/// Run one command through the login shell, the way the user's terminal would
/// see it (`zsh -lic`). Doctor and check use this, never a bare `sh -c`.
pub fn login_shell(runner: &dyn Runner, script: &str) -> Result<Output> {
    runner.run(&Cmd::new("zsh", SHORT).args(["-lic", script]))
}

/// A `zsh -lic` command's first output line, trimmed.
pub fn first_line(output: &Output) -> String {
    let text = if output.stdout.trim().is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    text.lines().next().unwrap_or("").trim().to_string()
}

/// Expand a leading `~` against `home`; everything else is left alone.
pub fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        return home.to_path_buf();
    }
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
pub mod fake {
    use super::*;
    use std::cell::RefCell;

    type Matcher = Box<dyn Fn(&Cmd) -> bool>;

    /// A scripted runner: the first rule whose matcher accepts the command
    /// answers it. Every command is recorded, matched or not.
    #[derive(Default)]
    #[allow(clippy::type_complexity)]
    pub struct FakeRunner {
        rules: RefCell<Vec<(Matcher, Box<dyn Fn(&Cmd) -> Result<Output>>)>>,
        pub calls: RefCell<Vec<Cmd>>,
    }

    impl FakeRunner {
        pub fn new() -> Self {
            Self::default()
        }

        /// Answer commands whose display line contains `needle`.
        pub fn on(&self, needle: &str, output: Output) -> &Self {
            let needle = needle.to_string();
            self.rules.borrow_mut().push((
                Box::new(move |cmd| cmd.display().contains(&needle)),
                Box::new(move |_| Ok(output.clone())),
            ));
            self
        }

        pub fn on_fn(
            &self,
            matcher: impl Fn(&Cmd) -> bool + 'static,
            answer: impl Fn(&Cmd) -> Result<Output> + 'static,
        ) -> &Self {
            self.rules
                .borrow_mut()
                .push((Box::new(matcher), Box::new(answer)));
            self
        }

        pub fn count(&self, needle: &str) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|cmd| cmd.display().contains(needle))
                .count()
        }
    }

    pub fn ok(stdout: &str) -> Output {
        Output {
            code: Some(0),
            stdout: stdout.to_string(),
            ..Output::default()
        }
    }

    pub fn fail(code: i32, stderr: &str) -> Output {
        Output {
            code: Some(code),
            stderr: stderr.to_string(),
            ..Output::default()
        }
    }

    pub fn timeout() -> Output {
        Output {
            timed_out: true,
            ..Output::default()
        }
    }

    impl Runner for FakeRunner {
        fn run(&self, cmd: &Cmd) -> Result<Output> {
            self.calls.borrow_mut().push(cmd.clone());
            for (matcher, answer) in self.rules.borrow().iter() {
                if matcher(cmd) {
                    return answer(cmd);
                }
            }
            anyhow::bail!("FakeRunner: no rule for `{}`", cmd.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_output_and_exit_code() {
        let out = RealRunner
            .run(
                &Cmd::new("sh", Duration::from_secs(5))
                    .args(["-c", "echo hi; echo err >&2; exit 3"]),
            )
            .unwrap();
        assert_eq!(out.code, Some(3));
        assert_eq!(out.stdout, "hi\n");
        assert_eq!(out.stderr, "err\n");
        assert!(!out.success());
    }

    #[test]
    fn missing_stdin_is_closed_and_times_out_alone() {
        // `cat` reads stdin; with `stdin: None` it sees EOF at once.
        let out = RealRunner
            .run(&Cmd::new("cat", Duration::from_secs(5)))
            .unwrap();
        assert_eq!(out.code, Some(0));
        assert_eq!(out.stdout, "");
    }

    #[test]
    fn a_timeout_kills_the_grandchildren_too() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let script = format!("(sleep 2; touch '{}') & wait", marker.display());
        let start = Instant::now();
        let out = RealRunner
            .run(&Cmd::new("sh", Duration::from_millis(300)).args(["-c", &script]))
            .unwrap();
        assert!(out.timed_out);
        assert!(start.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(2300));
        assert!(!marker.exists(), "a grandchild outlived the timeout");
    }

    #[test]
    fn expand_tilde_only_at_the_front() {
        let home = Path::new("/h/me");
        assert_eq!(expand_tilde("~/x", home), PathBuf::from("/h/me/x"));
        assert_eq!(expand_tilde("~", home), PathBuf::from("/h/me"));
        assert_eq!(expand_tilde("/abs", home), PathBuf::from("/abs"));
        assert_eq!(expand_tilde("rel/x~", home), PathBuf::from("rel/x~"));
    }
}
