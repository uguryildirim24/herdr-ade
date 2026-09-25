//! Every external command (herdr, git, gh, ssh, scp, rsync, sh) goes through `Runner`.

use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

/// The caller's contract, not a command-name or diagnostic allowlist.
/// Ambiguous probes (for example SSH checks) must keep `Required`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExitMeaning {
    Required,
    /// The caller checks a postcondition before interpreting a nonzero exit
    /// (for example a branch removed concurrently). The intermediate exit
    /// alone is not evidence of failure.
    Deferred,
    /// The command answered with its documented JSON result or refusal.
    Structured,
    /// Zero means yes; one with empty stderr means no. Every other outcome
    /// is an error. Only for commands with this precise contract (git ancestry).
    Boolean,
    /// `git merge-tree --write-tree`: exit 1 with conflict diagnostics is a
    /// valid conflict answer. Other nonzero exits are command failures.
    MergeTree,
}

impl ExitMeaning {
    pub(crate) fn answered(self, out: &Output) -> bool {
        out.success()
            || (self == Self::Deferred && out.code.is_some() && !out.timed_out)
            || (self == Self::Structured
                && out.code.is_some()
                && !out.timed_out
                && [&out.stdout, &out.stderr].into_iter().any(|text| {
                    serde_json::from_str::<serde_json::Value>(text.trim())
                        .ok()
                        .is_some_and(|reply| {
                            reply.get("result").is_some() || reply.get("error").is_some()
                        })
                }))
            || (self == Self::Boolean && out.boolean_answer().is_some())
            || (self == Self::MergeTree && out.merge_tree_conflict())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cmd {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) env: Vec<(String, String)>,
    pub(crate) env_remove: Vec<String>,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) stdin: Option<String>,
    pub(crate) timeout: Duration,
    pub(crate) exit_meaning: ExitMeaning,
    /// Stable identity for a recurring check. The literal command remains in
    /// failure evidence while recovery follows this identity across argv fixes.
    pub(crate) ledger_subject: Option<String>,
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
            exit_meaning: ExitMeaning::Required,
            ledger_subject: None,
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

    pub(crate) fn exit_meaning(mut self, meaning: ExitMeaning) -> Self {
        self.exit_meaning = meaning;
        self
    }

    #[cfg(test)]
    pub(crate) fn ledger_subject(mut self, subject: impl Into<String>) -> Self {
        self.ledger_subject = Some(subject.into());
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

    /// Decode the `ExitMeaning::Boolean` contract. `None` is a failed probe,
    /// including timeouts/signals, not a negative answer.
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

pub(crate) trait Runner {
    /// Only the production runner may be used by doctor's independent box
    /// snapshot worker; scripted runners preserve their deterministic calls.
    fn is_real(&self) -> bool {
        false
    }

    /// `Err` means the command could not be spawned at all (for example the
    /// program is missing). A non-zero exit or a timeout is an `Ok(Output)`.
    fn run(&self, cmd: &Cmd) -> Result<Output>;

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

    fn run_parallel(&self, commands: &[Cmd]) -> Vec<Result<Output>> {
        let commands: Vec<_> = commands.iter().map(|cmd| self.rooted(cmd)).collect();
        self.inner.run_parallel(&commands)
    }
}

pub(crate) struct RealRunner;

const POLL: Duration = Duration::from_millis(20);

impl Runner for RealRunner {
    fn is_real(&self) -> bool {
        true
    }

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
        // Even if a descendant kept the pipe open past the deadline, retain
        // bytes already drained instead of discarding the entire answer.
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

        /// Canned herdr replies for the ADE verbs named in SPEC-ADE §1.3.
        /// No plugin behaviour is implemented yet; later lanes match on these needles.
        pub(crate) fn on_ade_new_verbs(&self) -> &Self {
            for (needle, stdout) in ADE_NEW_VERB_REPLIES {
                self.on(needle, ok(stdout));
            }
            self
        }
    }

    /// Scenario names in SPEC-ADE §1.3 for the new verbs.
    pub(crate) const ADE_NEW_VERB_SCENARIOS: [&str; 8] = [
        "thread_start_parent",
        "ha_done",
        "ha_waiting",
        "round_open",
        "round_review",
        "checkpoint",
        "ask",
        "say",
    ];

    /// Needle → canned stdout for each new verb's herdr or git call (SPEC-ADE §1.3).
    /// More specific needles come first so FakeRunner's first-match rule is stable.
    const ADE_NEW_VERB_REPLIES: &[(&str, &str)] = &[
        (
            "--parent",
            r#"{"result":{"agent":{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2"}}}"#,
        ),
        (
            "HERDR_ADE_LAUNCH",
            r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2","cwd":"/wt"}}}"#,
        ),
        ("git status --short", ""),
        (
            "rev-parse HEAD",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n",
        ),
        ("git ls-tree", ""),
        ("worktree add", ""),
        ("update-ref", ""),
        ("workspace report-metadata", r#"{"result":{}}"#),
        ("notification show", r#"{"result":{}}"#),
        ("agent prompt", r#"{"result":{}}"#),
        (
            "tab create",
            r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t2","pane_id":"w1:p2","cwd":"/wt"}}}"#,
        ),
        (
            "agent start",
            r#"{"result":{"agent":{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","name":"lane-t-0001"}}}"#,
        ),
    ];

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
    fn passes_stdin() {
        let out = RealRunner
            .run(&Cmd::new("cat", Duration::from_secs(5)).stdin("hello"))
            .unwrap();
        assert_eq!(out.stdout, "hello");
    }

    #[test]
    fn missing_program_is_an_error() {
        assert!(
            RealRunner
                .run(&Cmd::new("hp-no-such-program", Duration::from_secs(1)))
                .is_err()
        );
    }

    #[test]
    fn times_out_a_chatty_child() {
        // `yes` fills the pipe far past its buffer; the reader threads keep it
        // drained so the deadline still fires.
        let start = Instant::now();
        let out = RealRunner
            .run(&Cmd::new("yes", Duration::from_millis(300)))
            .unwrap();
        assert!(out.timed_out);
        assert!(!out.success());
        assert!(start.elapsed() < Duration::from_secs(5));
        assert!(out.stdout.len() > 65_536);
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
