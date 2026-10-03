//! Every external command (herdr, git, gh, ssh, scp, rsync, sh) goes through `Runner`.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cmd {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) env_remove: Vec<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) stdin: Option<String>,
    pub(crate) timeout: Duration,
    /// Spawn in its own process group and kill the whole group on timeout.
    pub(crate) own_group: bool,
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
            own_group: false,
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

    pub(crate) fn env_remove(mut self, key: impl Into<String>) -> Self {
        self.env_remove.push(key.into());
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

    pub(crate) fn own_group(mut self) -> Self {
        self.own_group = true;
        self
    }

    /// The command as one line; the scripted fake matches on it.
    #[cfg(test)]
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

    /// Decode a boolean probe: zero is yes, one with no diagnostics is no.
    /// `None` is a failed probe, including timeouts/signals.
    pub(crate) fn boolean_answer(&self) -> Option<bool> {
        if self.timed_out {
            return None;
        }
        match self.code {
            Some(0) => Some(true),
            Some(1) if self.stderr.is_empty() => Some(false),
            _ => None,
        }
    }

    pub(crate) fn merge_tree_conflict(&self) -> bool {
        !self.timed_out
            && self.code == Some(1)
            && (self.stdout.contains("CONFLICT (") || self.stderr.contains("CONFLICT ("))
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

/// Byte counts describe raw pipe bytes, before UTF-8 decoding. `seen` is only
/// what was drained by the snapshot, not an estimate of output after a deadline.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct StreamEvidence {
    pub(crate) seen: u64,
    pub(crate) retained: usize,
    pub(crate) complete: bool,
    pub(crate) error: Option<String>,
    pub(crate) log: Option<PathBuf>,
}

impl StreamEvidence {
    pub(crate) fn omitted(&self) -> u64 {
        self.seen.saturating_sub(self.retained as u64)
    }

    fn diagnostic(&self, name: &str) -> String {
        format!(
            "{name}: seen={} retained={} omitted={} complete={}{}",
            self.seen,
            self.retained,
            self.omitted(),
            self.complete,
            self.error
                .as_ref()
                .map(|e| format!(" error={e}"))
                .unwrap_or_default()
        )
    }
}

/// Full logs go directly to operation-owned artifacts, never through an
/// unbounded in-memory buffer. Paths must be new and distinct.
pub(crate) struct OutputLogs {
    pub(crate) stdout: PathBuf,
    pub(crate) stderr: PathBuf,
}

#[derive(Debug)]
pub(crate) struct Capture {
    /// Original process status; bounded text is diagnostic, not a machine reply.
    /// Receipt success requires `complete() && output.success()`.
    pub(crate) output: Output,
    pub(crate) stdout: StreamEvidence,
    pub(crate) stderr: StreamEvidence,
}

impl Capture {
    /// Every byte is accounted for in retained text or a finished full log.
    /// A deadline snapshot or failed artifact is not a complete receipt.
    pub(crate) fn complete(&self) -> bool {
        [&self.stdout, &self.stderr].into_iter().all(|stream| {
            stream.complete
                && stream.error.is_none()
                && (stream.omitted() == 0 || stream.log.is_some())
        })
    }

    fn from_output(output: Output) -> Self {
        let evidence = |text: &str| StreamEvidence {
            seen: text.len() as u64,
            retained: text.len(),
            complete: !output.timed_out,
            ..Default::default()
        };
        Self {
            stdout: evidence(&output.stdout),
            stderr: evidence(&output.stderr),
            output,
        }
    }

    fn into_complete_output(self) -> Result<Output> {
        // Ordinary callers expect complete machine responses. A valid JSON or
        // TOML prefix must never reach their parser as an apparently good reply.
        if self.stdout.omitted() > 0
            || self.stderr.omitted() > 0
            || self.stdout.error.is_some()
            || self.stderr.error.is_some()
            || (!self.output.timed_out && !self.complete())
        {
            return Err(IncompleteOutput { capture: self }.into());
        }
        Ok(self.output)
    }
}

/// Downcastable capture failure: keep exit, signal/timeout identity and bounded
/// diagnostics even when a complete response cannot be returned.
#[derive(Debug)]
pub(crate) struct IncompleteOutput {
    pub(crate) capture: Capture,
}

impl std::fmt::Display for IncompleteOutput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "incomplete command output (output limit or pipe/log error; code={:?} timed_out={}): {}; {}",
            self.capture.output.code,
            self.capture.output.timed_out,
            self.capture.stdout.diagnostic("stdout"),
            self.capture.stderr.diagnostic("stderr")
        )
    }
}

impl std::error::Error for IncompleteOutput {}

pub(crate) trait Runner {
    /// Only the production runner may be used by doctor's independent box
    /// snapshot worker; scripted runners preserve their deterministic calls.
    fn is_real(&self) -> bool {
        false
    }

    /// Complete response, or an explicit spawn/incomplete-output error. A
    /// non-zero exit or timeout with bounded, intact diagnostics is `Ok(Output)`.
    fn run(&self, cmd: &Cmd) -> Result<Output>;

    /// Diagnostic/receipt capture. Callers must inspect stream completeness and
    /// errors as well as process status. Only this path can request full logs;
    /// clipped text is never a substitute for the operation's log artifacts.
    fn capture(&self, cmd: &Cmd, logs: Option<&OutputLogs>) -> Result<Capture> {
        anyhow::ensure!(
            logs.is_none(),
            "runner does not support full output artifacts"
        );
        self.run(cmd).map(Capture::from_output)
    }

    /// Independent commands. Production runs them side by side; scripted
    /// runners keep deterministic order unless they opt into concurrency.
    fn run_parallel(&self, commands: &[Cmd]) -> Vec<Result<Output>> {
        commands.iter().map(|command| self.run(command)).collect()
    }
}

/// Gives every child an explicit stable working directory. Commands that
/// already name a directory keep it (git and lane-specific commands depend on
/// that); everything else is detached from the caller's possibly disposable
/// worktree.
pub(crate) struct CwdRunner<'a> {
    inner: &'a dyn Runner,
    cwd: PathBuf,
}

impl<'a> CwdRunner<'a> {
    pub(crate) fn new(inner: &'a dyn Runner, cwd: impl Into<PathBuf>) -> Self {
        Self {
            inner,
            cwd: cwd.into(),
        }
    }

    fn rooted(&self, cmd: &Cmd) -> Cmd {
        let mut cmd = cmd.clone();
        if cmd.cwd.is_none() {
            cmd.cwd = Some(self.cwd.clone());
        }
        cmd
    }
}

impl Runner for CwdRunner<'_> {
    fn is_real(&self) -> bool {
        self.inner.is_real()
    }

    fn run(&self, cmd: &Cmd) -> Result<Output> {
        self.inner.run(&self.rooted(cmd))
    }

    fn capture(&self, cmd: &Cmd, logs: Option<&OutputLogs>) -> Result<Capture> {
        self.inner.capture(&self.rooted(cmd), logs)
    }

    fn run_parallel(&self, commands: &[Cmd]) -> Vec<Result<Output>> {
        let commands: Vec<_> = commands.iter().map(|cmd| self.rooted(cmd)).collect();
        self.inner.run_parallel(&commands)
    }
}

pub(crate) struct RealRunner;

const POLL: Duration = Duration::from_millis(20);
/// Fixed per-stream diagnostic budget; not a general resource-policy setting.
const RETAIN_BYTES: usize = 1024 * 1024;

impl Runner for RealRunner {
    fn is_real(&self) -> bool {
        true
    }

    fn run(&self, cmd: &Cmd) -> Result<Output> {
        self.capture(cmd, None)?.into_complete_output()
    }

    fn capture(&self, cmd: &Cmd, logs: Option<&OutputLogs>) -> Result<Capture> {
        // Open before spawning: failure must not leave a running child behind.
        let (stdout_log, stderr_log) = match logs {
            Some(logs) => {
                anyhow::ensure!(
                    logs.stdout != logs.stderr,
                    "stdout and stderr logs must be distinct"
                );
                (Some(open_log(&logs.stdout)?), Some(open_log(&logs.stderr)?))
            }
            None => (None, None),
        };
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
        #[cfg(unix)]
        if cmd.own_group {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }

        let mut child = command
            .spawn()
            .with_context(|| format!("could not run `{}`", cmd.program))?;

        // Readers and the writer run on their own threads so a full pipe in
        // either direction cannot deadlock against the deadline loop below.
        let stdin_thread = child
            .stdin
            .take()
            .zip(cmd.stdin.clone())
            .map(|(mut pipe, text)| {
                std::thread::spawn(move || {
                    let _ = pipe.write_all(text.as_bytes());
                })
            });
        let stdout_thread = child
            .stdout
            .take()
            .map(|pipe| read_bounded(pipe, stdout_log));
        let stderr_thread = child
            .stderr
            .take()
            .map(|pipe| read_bounded(pipe, stderr_log));

        let deadline = Instant::now() + cmd.timeout;
        let mut timed_out = false;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if Instant::now() >= deadline {
                timed_out = true;
                kill(&mut child, cmd.own_group);
                break child.wait().ok();
            }
            std::thread::sleep(POLL);
        };

        // A parent can exit while a descendant still holds its pipes. Do not
        // turn the command deadline into an unbounded reader-thread join.
        while !timed_out
            && (stdin_thread.as_ref().is_some_and(|t| !t.is_finished())
                || stdout_thread.as_ref().is_some_and(|t| !t.is_finished())
                || stderr_thread.as_ref().is_some_and(|t| !t.is_finished()))
            && Instant::now() < deadline
        {
            std::thread::sleep(POLL);
        }
        if stdin_thread.as_ref().is_some_and(|t| !t.is_finished())
            || stdout_thread.as_ref().is_some_and(|t| !t.is_finished())
            || stderr_thread.as_ref().is_some_and(|t| !t.is_finished())
        {
            timed_out = true;
            kill(&mut child, cmd.own_group);
        }
        if let Some(thread) = stdin_thread
            && thread.is_finished()
        {
            let _ = thread.join();
        }
        let (stdout, stdout_evidence) = stdout_thread.map(PipeReader::snapshot).unwrap_or_default();
        let (stderr, stderr_evidence) = stderr_thread.map(PipeReader::snapshot).unwrap_or_default();
        let mut capture = Capture {
            output: Output {
                code: if timed_out {
                    None
                } else {
                    status.and_then(|s| s.code())
                },
                stdout,
                stderr,
                timed_out,
            },
            stdout: stdout_evidence,
            stderr: stderr_evidence,
        };
        if capture.stdout.omitted() > 0 || capture.stderr.omitted() > 0 || !capture.complete() {
            // Also make diagnostic text self-describing for callers rendering
            // it without the evidence fields. Do not alter either artifact.
            capture.output.stderr.push_str(&format!(
                "\n[incomplete retained output; {}; {}]\n",
                capture.stdout.diagnostic("stdout"),
                capture.stderr.diagnostic("stderr")
            ));
        }
        Ok(capture)
    }

    fn run_parallel(&self, commands: &[Cmd]) -> Vec<Result<Output>> {
        std::thread::scope(|scope| {
            let handles: Vec<_> = commands
                .iter()
                .map(|command| scope.spawn(|| self.run(command)))
                .collect();
            handles
                .into_iter()
                .map(|handle| {
                    handle
                        .join()
                        .unwrap_or_else(|_| Err(anyhow::anyhow!("parallel command panicked")))
                })
                .collect()
        })
    }
}

#[derive(Default)]
struct PipeContent {
    bytes: Vec<u8>,
    evidence: StreamEvidence,
}

struct PipeReader {
    content: Arc<Mutex<PipeContent>>,
    thread: std::thread::JoinHandle<()>,
}

impl PipeReader {
    fn is_finished(&self) -> bool {
        self.thread.is_finished()
    }

    fn snapshot(self) -> (String, StreamEvidence) {
        if self.thread.is_finished() {
            let _ = self.thread.join();
        }
        // Never join a descendant-held pipe past the deadline. Both the live
        // reader and this snapshot stay bounded even if it keeps draining.
        let content = self.content.lock().unwrap();
        (
            String::from_utf8_lossy(&content.bytes).into_owned(),
            content.evidence.clone(),
        )
    }
}

fn open_log(path: &PathBuf) -> Result<(File, PathBuf)> {
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .with_context(|| format!("could not create output artifact `{}`", path.display()))?;
    Ok((file, path.clone()))
}

fn read_bounded<R: Read + Send + 'static>(mut pipe: R, log: Option<(File, PathBuf)>) -> PipeReader {
    let mut initial = PipeContent::default();
    initial.evidence.log = log.as_ref().map(|(_, path)| path.clone());
    let content = Arc::new(Mutex::new(initial));
    let saved = Arc::clone(&content);
    let thread = std::thread::spawn(move || {
        let mut log = log.map(|(file, _)| file);
        let mut chunk = [0; 8192];
        loop {
            let len = match pipe.read(&mut chunk) {
                Ok(0) => {
                    let error = log.as_ref().and_then(|file| file.sync_all().err());
                    let mut content = saved.lock().unwrap();
                    if let Some(error) = error {
                        content.evidence.error = Some(format!("log sync: {error}"));
                    }
                    content.evidence.complete = content.evidence.error.is_none();
                    break;
                }
                Ok(len) => len,
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    saved.lock().unwrap().evidence.error = Some(format!("pipe read: {error}"));
                    break;
                }
            };
            {
                let mut content = saved.lock().unwrap();
                content.evidence.seen = content.evidence.seen.saturating_add(len as u64);
                let keep = len.min(RETAIN_BYTES - content.bytes.len());
                content.bytes.extend_from_slice(&chunk[..keep]);
                content.evidence.retained = content.bytes.len();
            }
            // File I/O never holds the snapshot mutex. Even a stuck artifact
            // write cannot defeat the command deadline. A failed log still
            // drains both streams; it is explicitly not a complete receipt.
            if let Some(file) = log.as_mut()
                && let Err(error) = file.write_all(&chunk[..len])
            {
                saved.lock().unwrap().evidence.error = Some(format!("log write: {error}"));
                log = None;
            }
        }
    });
    PipeReader { content, thread }
}

fn kill(child: &mut std::process::Child, own_group: bool) {
    if own_group {
        // The child is its group's leader, so its pid is the pgid. Grandchildren
        // hold the pipes open; killing only the child would leave readers hanging.
        #[cfg(unix)]
        unsafe {
            unsafe extern "C" {
                fn kill(pid: i32, sig: i32) -> i32;
            }
            // Negative pid addresses the process group, including git's SSH
            // child and descendants that inherited the output pipes.
            let _ = kill(-(child.id() as i32), 9); // SIGKILL on Unix
        }
    }
    let _ = child.kill();
}

/// Default deadline for short login-shell and provider checks.
pub(crate) const SHORT: Duration = Duration::from_secs(10);
/// npm installation can take a while.
pub(crate) const SETUP: Duration = Duration::from_secs(600);

/// The machine's interactive login shell (zsh is absent on the box).
pub(crate) fn shell() -> String {
    std::env::var("SHELL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "zsh".into())
}

/// Probe through the login shell; the deadline includes its descendants.
pub(crate) fn login_shell(runner: &dyn Runner, script: &str) -> Result<Output> {
    runner.run(&Cmd::new(shell(), SHORT).args(["-lic", script]).own_group())
}

pub(crate) fn first_line(output: &Output) -> String {
    let text = if output.stdout.trim().is_empty() {
        &output.stderr
    } else {
        &output.stdout
    };
    text.lines().next().unwrap_or("").trim().to_string()
}

#[cfg(test)]
pub(crate) mod fake {
    use super::*;
    use std::cell::RefCell;

    type Matcher = Box<dyn Fn(&Cmd) -> bool>;
    type Answer = Box<dyn Fn(&Cmd) -> Result<Output>>;

    /// A scripted runner: the first rule whose matcher accepts the command
    /// answers it. Every command is recorded, matched or not.
    #[derive(Default)]
    pub(crate) struct FakeRunner {
        rules: RefCell<Vec<(Matcher, Answer)>>,
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
            // Fake coordinator panes start with an empty editor unless a test
            // scripts a visible terminal frame explicitly.
            if cmd.display().contains("pane read ") && cmd.display().contains("--source visible") {
                return Ok(ok("❯ \n"));
            }
            anyhow::bail!("FakeRunner: no rule for `{}`", cmd.display())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn stable_cwd_is_added_without_overriding_an_explicit_directory() {
        let fake = fake::FakeRunner::new();
        fake.on("probe inherited", fake::ok(""));
        fake.on("probe explicit", fake::ok(""));
        let rooted = CwdRunner::new(&fake, "/stable");
        rooted
            .run(&Cmd::new("probe", Duration::from_secs(1)).arg("inherited"))
            .unwrap();
        rooted
            .run(
                &Cmd::new("probe", Duration::from_secs(1))
                    .arg("explicit")
                    .cwd("/named"),
            )
            .unwrap();
        let calls = fake.calls.borrow();
        assert_eq!(calls[0].cwd.as_deref(), Some(Path::new("/stable")));
        assert_eq!(calls[1].cwd.as_deref(), Some(Path::new("/named")));
    }

    #[test]
    fn independent_commands_run_side_by_side() {
        let commands = [
            Cmd::new("sleep", Duration::from_secs(2)).arg("0.5"),
            Cmd::new("sleep", Duration::from_secs(2)).arg("0.5"),
            Cmd::new("sleep", Duration::from_secs(2)).arg("0.5"),
        ];
        let started = Instant::now();
        let results = RealRunner.run_parallel(&commands);
        assert!(results.into_iter().all(|result| result.unwrap().success()));
        assert!(
            started.elapsed() < Duration::from_millis(1100),
            "three starts ran serially: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn times_out_a_chatty_child() {
        let start = Instant::now();
        let capture = RealRunner
            .capture(&Cmd::new("yes", Duration::from_millis(300)), None)
            .unwrap();
        assert!(capture.output.timed_out);
        assert!(!capture.output.success());
        assert!(start.elapsed() < Duration::from_secs(5));
        assert_eq!(capture.stdout.retained, RETAIN_BYTES);
        assert!(capture.stdout.omitted() > 0);
        assert!(capture.output.stderr.contains("omitted="));
        let error = capture.into_complete_output().unwrap_err();
        assert!(
            error
                .downcast_ref::<IncompleteOutput>()
                .unwrap()
                .capture
                .output
                .timed_out
        );
    }

    #[test]
    fn bounds_both_streams_when_an_exited_parent_leaves_a_pipe_holder() {
        // No private process group: the finite pipe holder survives the parent
        // and would expose an unconditional join as a missed deadline.
        let script = "sleep 3 & dd if=/dev/zero bs=65536 count=48 2>/dev/null; \
                      dd if=/dev/zero bs=65536 count=48 >&2 2>/dev/null; exit 0";
        let start = Instant::now();
        let capture = RealRunner
            .capture(
                &Cmd::new("sh", Duration::from_secs(1)).args(["-c", script]),
                None,
            )
            .unwrap();
        assert!(capture.output.timed_out);
        assert_eq!(capture.output.code, None);
        assert!(start.elapsed() < Duration::from_secs(2));
        for evidence in [&capture.stdout, &capture.stderr] {
            assert_eq!(evidence.seen, 3 * RETAIN_BYTES as u64);
            assert_eq!(evidence.retained, RETAIN_BYTES);
            assert_eq!(evidence.omitted(), 2 * RETAIN_BYTES as u64);
        }
        assert!(!capture.stdout.complete);
        assert!(!capture.stderr.complete);
        assert_eq!(capture.output.stdout.len(), RETAIN_BYTES);
        assert!(capture.output.stderr.len() < RETAIN_BYTES + 1024);
        assert!(capture.output.stderr.contains("incomplete retained output"));
    }

    #[test]
    fn valid_but_clipped_machine_responses_return_an_explicit_error() {
        // The retained prefix is valid JSON/TOML: trailing whitespace is what
        // exceeds the budget. Parser failure alone cannot protect the caller.
        for reply in ["{\"ok\":true}", "ok = true\\n"] {
            let script = format!(
                "printf '{reply}'; dd if=/dev/zero bs=65536 count=32 2>/dev/null | tr '\\000' ' '"
            );
            let error = RealRunner
                .run(
                    &Cmd::new("sh", Duration::from_secs(5))
                        .args(["-c", &script])
                        .own_group(),
                )
                .unwrap_err();
            let failure = error.downcast_ref::<IncompleteOutput>().unwrap();
            assert_eq!(failure.capture.output.code, Some(0));
            assert!(!failure.capture.output.timed_out);
            assert!(failure.capture.stdout.complete);
            assert!(!failure.capture.complete());
            assert_eq!(failure.capture.stdout.retained, RETAIN_BYTES);
            assert!(failure.capture.stdout.omitted() > 0);
            let text = &failure.capture.output.stdout;
            if reply.starts_with('{') {
                assert!(serde_json::from_str::<serde_json::Value>(text).is_ok());
            } else {
                assert!(toml::from_str::<toml::Value>(text).is_ok());
            }
            assert!(error.to_string().contains("incomplete command output"));
            assert!(error.to_string().contains("omitted="));
        }
        let error = RealRunner
            .run(&Cmd::new("sh", Duration::from_secs(5)).args([
                "-c",
                "printf '{\"ok\":true}'; dd if=/dev/zero bs=65536 count=32 >&2 2>/dev/null",
            ]))
            .unwrap_err();
        let failure = error.downcast_ref::<IncompleteOutput>().unwrap();
        assert_eq!(failure.capture.output.code, Some(0));
        assert_eq!(failure.capture.stdout.omitted(), 0);
        assert!(failure.capture.stderr.omitted() > 0);
        assert!(!failure.capture.complete());
    }

    #[test]
    fn full_artifacts_keep_all_bytes_and_original_exit_identity() {
        for code in [Some(0), Some(17), None] {
            let dir = tempfile::tempdir().unwrap();
            let logs = OutputLogs {
                stdout: dir.path().join("stdout.log"),
                stderr: dir.path().join("stderr.log"),
            };
            let end = code
                .map(|code| format!("exit {code}"))
                .unwrap_or_else(|| "kill -TERM $$".into());
            let script = format!(
                "dd if=/dev/zero bs=65536 count=32 2>/dev/null; printf stdout-end; \
                 dd if=/dev/zero bs=65536 count=32 >&2 2>/dev/null; printf stderr-end >&2; {end}"
            );
            let capture = RealRunner
                .capture(
                    &Cmd::new("sh", Duration::from_secs(5))
                        .args(["-c", &script])
                        .own_group(),
                    Some(&logs),
                )
                .unwrap();
            assert_eq!(capture.output.code, code);
            assert!(capture.complete());
            assert_eq!(capture.output.success(), code == Some(0));
            assert!(!capture.output.timed_out);
            for (evidence, path, tail) in [
                (&capture.stdout, &logs.stdout, b"stdout-end"),
                (&capture.stderr, &logs.stderr, b"stderr-end"),
            ] {
                let bytes = std::fs::read(path).unwrap();
                assert_eq!(bytes.len(), 2 * RETAIN_BYTES + tail.len());
                assert!(bytes[..2 * RETAIN_BYTES].iter().all(|b| *b == 0));
                assert!(bytes.ends_with(tail));
                assert_eq!(evidence.seen, bytes.len() as u64);
                assert_eq!(evidence.retained, RETAIN_BYTES);
                assert_eq!(evidence.log.as_ref(), Some(path));
                assert!(evidence.complete);
                assert!(evidence.error.is_none());
            }
            // Having a full artifact never authorizes parsing clipped text,
            // and even that error keeps nonzero and signal identity intact.
            let error = capture.into_complete_output().unwrap_err();
            let failure = error.downcast_ref::<IncompleteOutput>().unwrap();
            assert_eq!(failure.capture.output.code, code);
            assert!(!failure.capture.output.timed_out);
        }
    }

    #[test]
    fn normal_output_keeps_exit_signal_and_probe_identity() {
        for code in [0, 1, 23] {
            let script = format!("printf stdout; printf stderr >&2; exit {code}");
            let out = RealRunner
                .run(&Cmd::new("sh", Duration::from_secs(5)).args(["-c", &script]))
                .unwrap();
            assert_eq!(out.code, Some(code));
            assert_eq!(out.stdout, "stdout");
            assert_eq!(out.stderr, "stderr");
            assert!(!out.timed_out);
            assert_eq!(out.success(), code == 0);
            assert_eq!(
                out.boolean_answer(),
                if code == 0 { Some(true) } else { None }
            );
        }
        let no = RealRunner
            .run(&Cmd::new("sh", Duration::from_secs(5)).args(["-c", "exit 1"]))
            .unwrap();
        assert_eq!(no.boolean_answer(), Some(false));
        let signal = RealRunner
            .run(&Cmd::new("sh", Duration::from_secs(5)).args(["-c", "kill -TERM $$"]))
            .unwrap();
        assert_eq!(signal.code, None);
        assert!(!signal.timed_out);
        assert!(!signal.success());
    }

    #[test]
    fn bounded_reader_counts_raw_bytes_and_reports_read_errors() {
        let reader = read_bounded(std::io::Cursor::new(vec![0xff; 2 * RETAIN_BYTES]), None);
        while !reader.is_finished() {
            std::thread::sleep(POLL);
        }
        // Inspect allocation, not just the length of the decoded string.
        assert!(reader.content.lock().unwrap().bytes.capacity() <= RETAIN_BYTES);
        let (text, evidence) = reader.snapshot();
        assert_eq!(text.len(), 3 * RETAIN_BYTES); // lossy UTF-8 is still bounded
        assert_eq!(evidence.seen, 2 * RETAIN_BYTES as u64);
        assert_eq!(evidence.omitted(), RETAIN_BYTES as u64);
        assert!(evidence.complete);

        struct BrokenPipe;
        impl Read for BrokenPipe {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::other("broken test pipe"))
            }
        }
        let reader = read_bounded(BrokenPipe, None);
        while !reader.is_finished() {
            std::thread::sleep(POLL);
        }
        let (_, evidence) = reader.snapshot();
        assert!(!evidence.complete);
        assert!(evidence.error.unwrap().contains("broken test pipe"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn log_write_failure_is_explicit_and_still_drains_the_stream() {
        let log = File::options().write(true).open("/dev/full").unwrap();
        let reader = read_bounded(
            std::io::Cursor::new(vec![b'x'; 2 * RETAIN_BYTES]),
            Some((log, PathBuf::from("/dev/full"))),
        );
        while !reader.is_finished() {
            std::thread::sleep(POLL);
        }
        let (_, evidence) = reader.snapshot();
        assert_eq!(evidence.seen, 2 * RETAIN_BYTES as u64);
        assert!(!evidence.complete);
        assert!(evidence.error.unwrap().contains("log write:"));
    }

    #[test]
    fn missing_stdin_is_closed() {
        let out = RealRunner
            .run(&Cmd::new("cat", Duration::from_secs(5)))
            .unwrap();
        assert_eq!(out.code, Some(0));
        assert_eq!(out.stdout, "");
    }

    #[test]
    fn group_kill_reaches_grandchildren() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("survived");
        let script = format!("(sleep 2; touch '{}') & wait", marker.display());
        let start = Instant::now();
        let out = RealRunner
            .run(
                &Cmd::new("sh", Duration::from_millis(300))
                    .args(["-c", &script])
                    .own_group(),
            )
            .unwrap();
        assert!(out.timed_out);
        assert!(start.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(2300));
        assert!(!marker.exists(), "grandchild outlived the group kill");
    }
}
