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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// The default timeout for a short read-only command (node, pi, herdr).
pub(crate) const SHORT: Duration = Duration::from_secs(10);
/// npm and `herdr integration install` can take a while; setup uses this.
pub(crate) const SETUP: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cmd {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) env_remove: Vec<String>,
    pub(crate) cwd: Option<PathBuf>,
    /// `None` closes stdin (`</dev/null` for every doctor and check run).
    pub(crate) stdin: Option<String>,
    pub(crate) timeout: Duration,
}

impl Cmd {
    pub(crate) fn new(program: impl Into<String>, timeout: Duration) -> Self {
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

    pub(crate) fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub(crate) fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.args.extend(args.into_iter().map(Into::into));
        self
    }

    pub(crate) fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub(crate) fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = Some(cwd.into());
        self
    }

    pub(crate) fn stdin(mut self, text: impl Into<String>) -> Self {
        self.stdin = Some(text.into());
        self
    }

    /// The command as one line; the scripted fake matches on it.
    pub(crate) fn display(&self) -> String {
        let mut line = self.program.clone();
        for arg in &self.args {
            line.push(' ');
            line.push_str(arg);
        }
        line
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Output {
    /// `None` when the process was killed (timeout or signal).
    pub(crate) code: Option<i32>,
    pub(crate) stdout: String,
    pub(crate) stderr: String,
    pub(crate) timed_out: bool,
}

impl Output {
    pub(crate) fn success(&self) -> bool {
        self.code == Some(0) && !self.timed_out
    }

    /// stderr when it has text, else stdout, trimmed; for error messages.
    pub(crate) fn error_text(&self) -> String {
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

pub(crate) trait Runner {
    /// `Err` means the command could not be spawned at all (for example the
    /// program is missing). A non-zero exit or a timeout is an `Ok(Output)`.
    fn run(&self, cmd: &Cmd) -> Result<Output>;
}

pub(crate) struct RealRunner;

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

        // The wrapper can exit before its network child releases the pipes.
        // Reader joins must share the same command deadline.
        while !timed_out
            && (stdin_thread.as_ref().is_some_and(|t| !t.is_finished())
                || stdout_thread.as_ref().is_some_and(|t| !t.is_finished())
                || stderr_thread.as_ref().is_some_and(|t| !t.is_finished()))
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(20));
        }
        if stdin_thread.as_ref().is_some_and(|t| !t.is_finished())
            || stdout_thread.as_ref().is_some_and(|t| !t.is_finished())
            || stderr_thread.as_ref().is_some_and(|t| !t.is_finished())
        {
            timed_out = true;
            kill_group(&mut child);
        }
        if let Some(thread) = stdin_thread
            && thread.is_finished()
        {
            let _ = thread.join();
        }
        let stdout = stdout_thread.map(PipeReader::text).unwrap_or_default();
        let stderr = stderr_thread.map(PipeReader::text).unwrap_or_default();

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
    #[cfg(unix)]
    unsafe {
        unsafe extern "C" {
            fn kill(pid: i32, sig: i32) -> i32;
        }
        let _ = kill(-(child.id() as i32), 9); // SIGKILL on Unix
    }
    let _ = child.kill();
}

struct PipeReader {
    content: Arc<Mutex<Vec<u8>>>,
    thread: std::thread::JoinHandle<()>,
}

impl PipeReader {
    fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    fn text(self) -> String {
        if self.thread.is_finished() {
            let _ = self.thread.join();
        }
        // Preserve output already read even when a descendant keeps the pipe
        // open past the deadline.
        String::from_utf8_lossy(&self.content.lock().unwrap()).into_owned()
    }
}

fn read_all<R: Read + Send + 'static>(mut pipe: R) -> PipeReader {
    let content = Arc::new(Mutex::new(Vec::new()));
    let saved = Arc::clone(&content);
    let thread = std::thread::spawn(move || {
        let mut chunk = [0; 8192];
        while let Ok(len) = pipe.read(&mut chunk) {
            if len == 0 {
                break;
            }
            saved.lock().unwrap().extend_from_slice(&chunk[..len]);
        }
    });
    PipeReader { content, thread }
}

/// The interactive login shell a pane starts in `auto` mode: `$SHELL`, with
/// zsh as the fallback. SPEC-remote §3.3 replaces the hard-coded `zsh -lic`
/// probe with the machine's own shell (zsh is absent on the box).
pub(crate) fn shell() -> String {
    std::env::var("SHELL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "zsh".into())
}

/// Run one command through the login shell, the way the user's terminal would
/// see it (`$SHELL -lic`). Doctor and check use this, never a bare `sh -c`.
pub(crate) fn login_shell(runner: &dyn Runner, script: &str) -> Result<Output> {
    runner.run(&Cmd::new(shell(), SHORT).args(["-lic", script]))
}

/// A `$SHELL -lic` command's first output line, trimmed.
pub(crate) fn first_line(output: &Output) -> String {
    let text = if output.stdout.trim().is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    text.lines().next().unwrap_or("").trim().to_string()
}

/// Expand a leading `~` against `home`; everything else is left alone.
pub(crate) fn expand_tilde(path: &str, home: &Path) -> PathBuf {
    if path == "~" {
        return home.to_path_buf();
    }
    match path.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None => PathBuf::from(path),
    }
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::cell::RefCell;

    type Matcher = Box<dyn Fn(&Cmd) -> bool>;

    /// A scripted runner: the first rule whose matcher accepts the command
    /// answers it. Every command is recorded, matched or not.
    #[derive(Default)]
    #[allow(clippy::type_complexity)]
    pub(crate) struct FakeRunner {
        rules: RefCell<Vec<(Matcher, Box<dyn Fn(&Cmd) -> Result<Output>>)>>,
        pub(crate) calls: RefCell<Vec<Cmd>>,
    }

    impl FakeRunner {
        pub(crate) fn new() -> Self {
            Self::default()
        }

        /// Answer commands whose display line contains `needle`.
        pub(crate) fn on(&self, needle: &str, output: Output) -> &Self {
            let needle = needle.to_string();
            self.rules.borrow_mut().push((
                Box::new(move |cmd| cmd.display().contains(&needle)),
                Box::new(move |_| Ok(output.clone())),
            ));
            self
        }

        pub(crate) fn on_fn(
            &self,
            matcher: impl Fn(&Cmd) -> bool + 'static,
            answer: impl Fn(&Cmd) -> Result<Output> + 'static,
        ) -> &Self {
            self.rules
                .borrow_mut()
                .push((Box::new(matcher), Box::new(answer)));
            self
        }

        pub(crate) fn count(&self, needle: &str) -> usize {
            self.calls
                .borrow()
                .iter()
                .filter(|cmd| cmd.display().contains(needle))
                .count()
        }
    }

    pub(crate) fn ok(stdout: &str) -> Output {
        Output {
            code: Some(0),
            stdout: stdout.to_string(),
            ..Output::default()
        }
    }

    pub(crate) fn fail(code: i32, stderr: &str) -> Output {
        Output {
            code: Some(code),
            stderr: stderr.to_string(),
            ..Output::default()
        }
    }

    pub(crate) fn timeout() -> Output {
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
