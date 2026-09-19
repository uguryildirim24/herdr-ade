//! `herdr-pro serve`: the local relay that makes Pro a plain pi lane.
//!
//! pi speaks the OpenAI Responses API to this server. Each request becomes one
//! headless `codex exec --json` turn in the shared Pro Codex home, pointed at
//! the installed bridge, so Codex itself writes the native turn metadata the
//! bridge requires. Conversation continuity is one Codex thread per pi session:
//! the first turn is `codex exec`, later turns are
//! `codex exec resume <thread_id>`.
//!
//! The server is loopback only and refuses anything but the per-install bearer
//! token in `serve.json`. In-flight rules: one turn per session at a time and
//! the same overall limit a Pro turn uses. A failed turn feeds the two-hour
//! breaker.

use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddrV4, TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::sh::Runner;
use super::{BRIDGE_PORT, Env, FAILURE_WINDOW, Layout, TURN_TIMEOUT, lane, state};

/// The model id `GET /v1/models` lists and the relay accepts.
pub const MODEL_ID: &str = super::provider::MODEL_ID;

/// Read timeout while waiting for a request header block.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

unsafe extern "C" {
    fn setsid() -> i32;
}

/// `serve.json`: how pi and the doctor find the relay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ServeState {
    pub port: u16,
    pub pid: u32,
    pub started: String,
    pub token: String,
}

impl ServeState {
    pub fn read(layout: &Layout) -> Option<ServeState> {
        let text = std::fs::read_to_string(layout.serve_state()).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn write(&self, layout: &Layout) -> Result<()> {
        let path = layout.serve_state();
        let text = serde_json::to_string_pretty(self).context("could not serialize serve.json")?;
        state::write_atomic(&path, &format!("{text}\n"))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("could not protect {}", path.display()))
    }

    pub fn remove(layout: &Layout) {
        let _ = std::fs::remove_file(layout.serve_state());
    }
}

/// True when a process with this pid exists.
pub fn pid_alive(pid: u32) -> bool {
    Command::new("/bin/kill")
        .arg("-0")
        .arg(pid.to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

/// The open liveness endpoint must answer for the exact process in
/// `serve.json`; a reused pid is not an already-running relay.
fn relay_healthy(state: &ServeState) -> bool {
    let address: std::net::SocketAddr = SocketAddrV4::new(Ipv4Addr::LOCALHOST, state.port).into();
    let Ok(mut stream) = TcpStream::connect_timeout(&address, Duration::from_millis(500)) else {
        return false;
    };
    let timeout = Some(Duration::from_millis(500));
    let _ = stream.set_read_timeout(timeout);
    let _ = stream.set_write_timeout(timeout);
    if stream
        .write_all(b"GET /healthz HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n")
        .is_err()
    {
        return false;
    }
    let mut response = String::new();
    if stream.read_to_string(&mut response).is_err() {
        return false;
    }
    let Some((head, body)) = response.split_once("\r\n\r\n") else {
        return false;
    };
    if !head.starts_with("HTTP/1.1 200 ") {
        return false;
    }
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| value.get("pid").and_then(Value::as_u64))
        == Some(state.pid as u64)
}

/// `herdr-pro serve`: start the daemon unless one is already running.
pub fn start(layout: &Layout) -> Result<bool> {
    if let Some(state) = ServeState::read(layout)
        && pid_alive(state.pid)
    {
        if !relay_healthy(&state) {
            bail!(
                "serve.json names live pid {}, but its relay does not answer on port {}; run `herdr-pro stop-serve` first",
                state.pid,
                state.port
            );
        }
        println!(
            "relay already running on port {} (pid {})",
            state.port, state.pid
        );
        return Ok(false);
    }
    layout.ensure()?;
    let exe = std::env::current_exe().context("could not find this binary's own path")?;
    let mut command = Command::new(exe);
    command
        .env("HERDR_PRO_STATE_DIR", layout.root.display().to_string())
        .args(["serve-run"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    // SAFETY: setsid is async-signal-safe and touches no memory.
    unsafe {
        command.pre_exec(|| {
            setsid();
            Ok(())
        });
    }
    let child = command.spawn().context("could not start the relay")?;
    let pid = child.id();
    for _ in 0..100 {
        if let Some(state) = ServeState::read(layout)
            && state.pid == pid
            && relay_healthy(&state)
        {
            println!(
                "relay started on port {} (pid {pid}); token in {}",
                state.port,
                layout.serve_state().display()
            );
            return Ok(true);
        }
        if !pid_alive(pid) {
            bail!("the relay process exited before it became ready");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!(
        "the relay did not write {} within five seconds",
        layout.serve_state().display()
    )
}

/// `herdr-pro serve-run`: the foreground server (started detached by `serve`).
pub fn run(layout: &Layout, env: &Env) -> Result<()> {
    layout.ensure()?;
    // One relay per state dir: a stale pid lets a fresh one take over.
    if let Some(state) = ServeState::read(layout)
        && state.pid != std::process::id()
        && pid_alive(state.pid)
    {
        bail!("relay pid {} is already running", state.pid);
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).context("could not bind 127.0.0.1")?;
    let port = listener
        .local_addr()
        .context("could not read the relay port")?
        .port();
    let token = random_token()?;
    ServeState {
        port,
        pid: std::process::id(),
        started: super::now_rfc3339(),
        token: token.clone(),
    }
    .write(layout)?;
    // The relay's own cwd, trusted so Codex never prompts for it.
    let _ = super::home::trust(layout, &layout.root);
    write_provider(layout, port, &token)?;
    let bridge_port = BRIDGE_PORT;
    let relay = Arc::new(Relay {
        env: env.clone(),
        layout: layout.clone(),
        token,
        port,
        bridge_port,
        sessions: Mutex::new(HashMap::new()),
        inflight: Mutex::new(HashMap::new()),
        seq: AtomicU64::new(0),
        failures: Mutex::new(Vec::new()),
    });
    for incoming in listener.incoming() {
        let Ok(stream) = incoming else {
            continue;
        };
        let relay = Arc::clone(&relay);
        std::thread::spawn(move || {
            let _ = relay.handle(stream);
        });
    }
    Ok(())
}

/// `herdr-pro stop-serve`: end the daemon and remove `serve.json`.
pub fn stop(layout: &Layout) -> Result<bool> {
    let Some(state) = ServeState::read(layout) else {
        println!("no relay is running");
        return Ok(false);
    };
    if pid_alive(state.pid) {
        let _ = Command::new("/bin/kill")
            .arg("-TERM")
            .arg(state.pid.to_string())
            .status();
        for _ in 0..40 {
            if !pid_alive(state.pid) {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    ServeState::remove(layout);
    println!("relay on port {} stopped", state.port);
    Ok(true)
}

/// `GET /v1/models` with the token; the doctor's liveness row.
pub fn models_health(runner: &dyn Runner, layout: &Layout) -> Result<String> {
    let state = ServeState::read(layout)
        .context("no serve.json; the relay was never started (`herdr-pro serve`)")?;
    let output = runner.run(&super::sh::Cmd::new("curl", Duration::from_secs(10)).args([
        "-sS",
        "--fail-with-body",
        "--max-time",
        "5",
        "-H",
        &format!("Authorization: Bearer {}", state.token),
        &format!("http://127.0.0.1:{}/v1/models", state.port),
    ]))?;
    if !output.success() {
        bail!(
            "the relay on port {} did not answer /v1/models: {}",
            state.port,
            output.error_text()
        );
    }
    let body: Value = serde_json::from_str(output.stdout.trim())
        .context("the relay /v1/models did not answer JSON")?;
    let has_pro = body
        .get("data")
        .and_then(Value::as_array)
        .is_some_and(|list| {
            list.iter()
                .any(|m| m.get("id").and_then(Value::as_str) == Some(MODEL_ID))
        });
    if !has_pro {
        bail!("the relay /v1/models does not list `{MODEL_ID}`");
    }
    Ok(format!("port {}", state.port))
}

/// Point the shared pi folder's `models.json` at this relay.
pub fn write_provider(layout: &Layout, port: u16, token: &str) -> Result<()> {
    let path = layout.pi_models();
    super::provider::write_merged(&path, &super::provider::base_url(port), token)
        .with_context(|| format!("could not write the pi provider {}", path.display()))
}

fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")
        .context("could not open /dev/urandom")
        .and_then(|mut file| {
            file.read_exact(&mut bytes)
                .context("could not read /dev/urandom")
        })?;
    Ok(hex(&bytes))
}

fn hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// One running Codex thread per pi session.
struct Session {
    codex_id: Option<String>,
}

struct Relay {
    env: Env,
    layout: Layout,
    token: String,
    port: u16,
    bridge_port: u16,
    sessions: Mutex<HashMap<String, Session>>,
    inflight: Mutex<HashMap<String, ()>>,
    seq: AtomicU64,
    failures: Mutex<Vec<Instant>>,
}

impl Relay {
    fn handle(&self, mut stream: TcpStream) -> Result<()> {
        stream.set_read_timeout(Some(REQUEST_TIMEOUT))?;
        let request = match read_request(&mut stream) {
            Ok(request) => request,
            Err(_) => return Ok(()),
        };
        let _ = stream.set_read_timeout(None);
        if request.method == "GET" && request.path == "/healthz" {
            return write_json(
                &mut stream,
                200,
                &json!({"status":"ok","port":self.port,"pid":std::process::id()}),
            );
        }
        if !self.authorized(&request) {
            return write_json_error(&mut stream, 401, "missing or wrong bearer token");
        }
        match (request.method.as_str(), request.path.as_str()) {
            ("GET", "/v1/models") => write_models(&mut stream),
            ("POST", "/v1/responses") => self.responses(&mut stream, &request),
            ("GET", path) => write_json_error(&mut stream, 404, &format!("no route {path}")),
            _ => write_json_error(&mut stream, 405, "method not allowed"),
        }
    }

    fn authorized(&self, request: &Request) -> bool {
        request
            .headers
            .get("authorization")
            .is_some_and(|value| value == &format!("Bearer {}", self.token))
    }

    fn responses(&self, stream: &mut TcpStream, request: &Request) -> Result<()> {
        let body: Value = match serde_json::from_slice(&request.body) {
            Ok(body) => body,
            Err(error) => {
                return write_json_error(stream, 400, &format!("body is not JSON: {error}"));
            }
        };
        let want_stream = body.get("stream").and_then(Value::as_bool).unwrap_or(false);
        let key = session_key(&request.headers, &body);

        // Fail closed before spending a Codex turn.
        if state::cooldown_active(&self.layout, jiff::Timestamp::now()) {
            let until = state::cooldown_until(&self.layout)
                .map(|u| u.to_string())
                .unwrap_or_default();
            return write_json_error(
                stream,
                503,
                &format!(
                    "the Pro breaker is active until {until}; clear it with `herdr-pro resume-bridge`"
                ),
            );
        }
        let limit = self.env.inflight_limit().unwrap_or(super::INFLIGHT_DEFAULT);
        {
            let mut inflight = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
            if inflight.contains_key(&key) {
                return write_json_error(
                    stream,
                    429,
                    "a turn is already running for this conversation",
                );
            }
            if inflight.len() >= limit {
                return write_json_error(
                    stream,
                    429,
                    &format!("{limit} turns are already in flight"),
                );
            }
            inflight.insert(key.clone(), ());
        }
        let guard = InflightGuard {
            relay: self,
            key: key.clone(),
        };

        let resume = self
            .sessions
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&key)
            .and_then(|session| session.codex_id.clone());
        let first = resume.is_none();
        let prompt = build_prompt(&body, first);

        let result = if want_stream {
            self.stream_turn(stream, &key, resume.as_deref(), &prompt)
        } else {
            self.json_turn(stream, &key, resume.as_deref(), &prompt)
        };
        drop(guard);
        result
    }

    fn stream_turn(
        &self,
        stream: &mut TcpStream,
        key: &str,
        resume: Option<&str>,
        prompt: &str,
    ) -> Result<()> {
        write_sse_headers(stream)?;
        let mut sse = Sse { stream };
        let mut sink = |kind: &str, data: &Value| sse.send(kind, data);
        match self.run_codex(resume, prompt, &mut sink) {
            Ok(outcome) => {
                self.remember(key, &outcome);
                sse.end()
            }
            Err(error) => {
                self.record_failure();
                let _ = sse.send(
                    "response.failed",
                    &json!({
                        "type":"response.failed",
                        "response":{"status":"failed","error":{"code":"codex_error","message":format!("{error:#}")}}
                    }),
                );
                sse.end()
            }
        }
    }

    fn json_turn(
        &self,
        stream: &mut TcpStream,
        key: &str,
        resume: Option<&str>,
        prompt: &str,
    ) -> Result<()> {
        let mut noop = |_: &str, _: &Value| Ok(());
        match self.run_codex(resume, prompt, &mut noop) {
            Ok(outcome) => {
                self.remember(key, &outcome);
                write_json(
                    stream,
                    200,
                    &json!({
                        "id": outcome.response_id,
                        "object": "response",
                        "status": "completed",
                        "output": [{
                            "id": outcome.message_id,
                            "type": "message",
                            "role": "assistant",
                            "status": "completed",
                            "content": [{"type": "output_text", "text": outcome.answer, "annotations": []}]
                        }],
                        "usage": outcome.usage,
                    }),
                )
            }
            Err(error) => {
                self.record_failure();
                write_json(
                    stream,
                    200,
                    &json!({
                        "status": "failed",
                        "error": {"code": "codex_error", "message": format!("{error:#}")}
                    }),
                )
            }
        }
    }

    /// Store the Codex thread id for the next request on this session.
    fn remember(&self, key: &str, outcome: &CodexOutcome) {
        if let Some(codex_id) = &outcome.codex_id {
            self.sessions
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .insert(
                    key.to_string(),
                    Session {
                        codex_id: Some(codex_id.clone()),
                    },
                );
        }
    }

    fn record_failure(&self) {
        let now = Instant::now();
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        failures.retain(|at| now.duration_since(*at) <= FAILURE_WINDOW);
        failures.push(now);
        if failures.len() >= 2 {
            let until = jiff::Timestamp::now()
                + jiff::SignedDuration::from_secs(super::COOLDOWN.as_secs() as i64);
            let _ = state::set_cooldown(&self.layout, until);
        }
    }

    fn run_codex(
        &self,
        resume: Option<&str>,
        prompt: &str,
        sink: &mut dyn FnMut(&str, &Value) -> Result<()>,
    ) -> Result<CodexOutcome> {
        let response_id = format!(
            "resp_{}",
            hex(&std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
                .to_be_bytes())
        );
        sink(
            "response.created",
            &json!({"type":"response.created","response":{"id":response_id,"status":"in_progress"}}),
        )?;

        let relay_dir = self.layout.root.join("relay");
        std::fs::create_dir_all(&relay_dir)
            .with_context(|| format!("could not create {}", relay_dir.display()))?;
        let out_file = relay_dir.join(format!(
            "out-{}-{}.txt",
            std::process::id(),
            self.seq.fetch_add(1, Ordering::SeqCst)
        ));
        let mut child = self.spawn_codex(resume, prompt, &out_file)?;
        if let Some(mut stdin) = child.stdin.take()
            && let Err(error) = stdin.write_all(prompt.as_bytes())
        {
            let _ = child.kill();
            let _ = child.wait();
            let _ = std::fs::remove_file(&out_file);
            return Err(error).context("could not write the prompt to codex");
        }
        let stdout = child.stdout.take().context("codex has no stdout")?;
        let stderr = child.stderr.take();
        let stderr_reader = stderr.map(|mut pipe| {
            std::thread::spawn(move || {
                let mut text = String::new();
                let _ = pipe.read_to_string(&mut text);
                text
            })
        });

        let watchdog = watchdog(&child);
        let mut state = StreamState::new(response_id.clone());
        let reader = BufReader::new(stdout);
        let stream_result = (|| -> Result<()> {
            for line in reader.lines() {
                let line = line.context("could not read codex stdout")?;
                if line.trim().is_empty() {
                    continue;
                }
                let Ok(event) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                state.on_event(&event, sink)?;
            }
            Ok(())
        })();
        if stream_result.is_err() {
            let _ = child.kill();
        }
        let status_result = child.wait();
        watchdog.store(true, Ordering::SeqCst);
        let stderr_text = stderr_reader
            .and_then(|handle| handle.join().ok())
            .unwrap_or_default();
        stream_result?;
        let status = status_result.context("could not wait for codex")?;

        if state.failure.is_none() && !status.success() {
            state.failure = Some(if stderr_text.trim().is_empty() {
                format!("codex exited {}", status.code().unwrap_or(-1))
            } else {
                stderr_text.trim().to_string()
            });
        }
        // The `-o` file is the authoritative last message when the stream did
        // not carry one.
        if state.failure.is_none()
            && state.answer.trim().is_empty()
            && let Ok(text) = std::fs::read_to_string(&out_file)
            && !text.trim().is_empty()
        {
            state.push_text("msg_codex_final", text.trim(), sink)?;
            state.close_open(sink)?;
        }
        if state.failure.is_none() && !state.completed {
            state.failure = Some("codex ended without a completed turn".into());
        }
        if state.failure.is_none() && state.answer.trim().is_empty() {
            state.failure = Some("codex completed without an agent message".into());
        }
        let _ = std::fs::remove_file(&out_file);
        if let Some(message) = state.failure.clone() {
            return Err(anyhow::anyhow!("{message}"));
        }
        state.finish(sink)?;
        Ok(state.into_outcome())
    }

    fn spawn_codex(&self, resume: Option<&str>, _prompt: &str, out_file: &Path) -> Result<Child> {
        let mut args: Vec<String> = vec!["exec".into()];
        if let Some(id) = resume {
            args.push("resume".into());
            args.push(id.to_string());
        }
        args.push("--json".into());
        args.push("--skip-git-repo-check".into());
        args.push("-o".into());
        args.push(out_file.display().to_string());
        args.extend(lane::codex_args(self.bridge_port));
        // The prompt arrives on stdin as `-`: no argv limits and no quoting.
        args.push("-".into());
        let path = child_path(&self.env);
        Command::new("codex")
            .args(args)
            .current_dir(&self.layout.root)
            .env("CODEX_HOME", self.layout.codex_home().display().to_string())
            .env("PATH", path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .context("could not start `codex exec`; is codex on PATH?")
    }
}

struct InflightGuard<'a> {
    relay: &'a Relay,
    key: String,
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.relay
            .inflight
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.key);
    }
}

/// Kill a Codex turn that outlasts the two-hour turn window.
fn watchdog(child: &Child) -> Arc<AtomicBool> {
    let done = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&done);
    let pid = child.id();
    std::thread::spawn(move || {
        let deadline = Instant::now() + TURN_TIMEOUT;
        while Instant::now() < deadline {
            if flag.load(Ordering::SeqCst) {
                return;
            }
            std::thread::sleep(Duration::from_secs(5));
        }
        let _ = Command::new("/bin/kill")
            .arg("-KILL")
            .arg(pid.to_string())
            .status();
    });
    done
}

/// The child's PATH: what the parent has, plus the usual install folders, so a
/// plugin startup with a minimal environment still finds codex.
fn child_path(env: &Env) -> String {
    let current = std::env::var("PATH").unwrap_or_default();
    let mut dirs: Vec<PathBuf> = std::env::split_paths(&current).collect();
    for extra in [
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
        env.home.join(".local/bin"),
        env.home.join(".cargo/bin"),
    ] {
        if !dirs.contains(&extra) {
            dirs.push(extra);
        }
    }
    std::env::join_paths(dirs)
        .map(|joined| joined.to_string_lossy().into_owned())
        .unwrap_or(current)
}

/// What one Codex turn produced.
#[derive(Debug, Clone, PartialEq)]
pub struct CodexOutcome {
    pub response_id: String,
    pub message_id: String,
    pub answer: String,
    pub codex_id: Option<String>,
    pub usage: Value,
}

/// The Responses-API view of one `codex exec --json` stream.
struct StreamState {
    response_id: String,
    open_id: Option<String>,
    open_index: usize,
    texts: BTreeMap<String, String>,
    order: Vec<String>,
    answer: String,
    codex_id: Option<String>,
    usage: Value,
    completed: bool,
    failure: Option<String>,
}

impl StreamState {
    fn new(response_id: String) -> StreamState {
        StreamState {
            response_id,
            open_id: None,
            open_index: 0,
            texts: BTreeMap::new(),
            order: Vec::new(),
            answer: String::new(),
            codex_id: None,
            usage: json!({}),
            completed: false,
            failure: None,
        }
    }

    fn on_event(
        &mut self,
        event: &Value,
        sink: &mut dyn FnMut(&str, &Value) -> Result<()>,
    ) -> Result<()> {
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "thread.started" => {
                self.codex_id = event
                    .get("thread_id")
                    .and_then(Value::as_str)
                    .map(str::to_string);
            }
            "item.started" | "item.updated" | "item.completed" => {
                let item = &event["item"];
                if item.get("type").and_then(Value::as_str) != Some("agent_message") {
                    return Ok(());
                }
                let id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("msg")
                    .to_string();
                let text = item
                    .get("text")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                self.push_text(&id, &text, sink)?;
                if event.get("type").and_then(Value::as_str) == Some("item.completed") {
                    self.close_open(sink)?;
                }
            }
            "turn.completed" => {
                self.completed = true;
                if let Some(usage) = event.get("usage") {
                    self.usage = usage.clone();
                }
                self.close_open(sink)?;
            }
            "turn.failed" => {
                self.failure = Some(
                    event
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .or_else(|| event.get("message").and_then(Value::as_str))
                        .unwrap_or("the Codex turn failed")
                        .to_string(),
                );
            }
            "error" => {
                self.failure = Some(
                    event
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("a Codex stream error")
                        .to_string(),
                );
            }
            _ => {}
        }
        Ok(())
    }

    fn push_text(
        &mut self,
        id: &str,
        text: &str,
        sink: &mut dyn FnMut(&str, &Value) -> Result<()>,
    ) -> Result<()> {
        if self.open_id.as_deref() != Some(id) {
            self.close_open(sink)?;
            self.open_index = self.order.len();
            self.order.push(id.to_string());
            self.open_id = Some(id.to_string());
            sink(
                "response.output_item.added",
                &json!({
                    "type":"response.output_item.added",
                    "output_index": self.open_index,
                    "item":{"id":id,"type":"message","role":"assistant","status":"in_progress","content":[]}
                }),
            )?;
        }
        let previous = self.texts.get(id).cloned().unwrap_or_default();
        let (delta, next) = if let Some(rest) = text.strip_prefix(&previous) {
            (rest.to_string(), text.to_string())
        } else {
            (text.to_string(), format!("{previous}{text}"))
        };
        self.texts.insert(id.to_string(), next.clone());
        self.answer = next;
        if !delta.is_empty() {
            sink(
                "response.output_text.delta",
                &json!({
                    "type":"response.output_text.delta",
                    "output_index": self.open_index,
                    "delta": delta
                }),
            )?;
        }
        Ok(())
    }

    fn close_open(&mut self, sink: &mut dyn FnMut(&str, &Value) -> Result<()>) -> Result<()> {
        let Some(id) = self.open_id.take() else {
            return Ok(());
        };
        let text = self.texts.get(&id).cloned().unwrap_or_default();
        sink(
            "response.output_item.done",
            &json!({
                "type":"response.output_item.done",
                "output_index": self.open_index,
                "item":{
                    "id":id,"type":"message","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":text,"annotations":[]}]
                }
            }),
        )
    }

    fn finish(&mut self, sink: &mut dyn FnMut(&str, &Value) -> Result<()>) -> Result<()> {
        self.close_open(sink)?;
        let output: Vec<Value> = self
            .order
            .iter()
            .map(|id| {
                let text = self.texts.get(id).cloned().unwrap_or_default();
                json!({
                    "id":id,"type":"message","role":"assistant","status":"completed",
                    "content":[{"type":"output_text","text":text,"annotations":[]}]
                })
            })
            .collect();
        sink(
            "response.completed",
            &json!({
                "type":"response.completed",
                "response":{
                    "id": self.response_id,
                    "object": "response",
                    "status": "completed",
                    "output": output,
                    "usage": self.usage,
                }
            }),
        )
    }

    fn into_outcome(self) -> CodexOutcome {
        let message_id = self
            .order
            .last()
            .cloned()
            .unwrap_or_else(|| "msg_codex".to_string());
        CodexOutcome {
            response_id: self.response_id,
            message_id,
            answer: self.answer,
            codex_id: self.codex_id,
            usage: self.usage,
        }
    }
}

/// The session key: an explicit `conversation`, else the pi session header,
/// else the `instructions` hash plus `X-Herdr-Lane`.
pub fn session_key(headers: &HashMap<String, String>, body: &Value) -> String {
    if let Some(conversation) = body.get("conversation") {
        if let Some(id) = conversation.as_str()
            && !id.is_empty()
        {
            return format!("conversation:{id}");
        }
        if let Some(id) = conversation.get("id").and_then(Value::as_str)
            && !id.is_empty()
        {
            return format!("conversation:{id}");
        }
    }
    for header in ["session_id", "x-client-request-id"] {
        if let Some(value) = headers.get(header)
            && !value.is_empty()
        {
            return format!("session:{value}");
        }
    }
    let instructions = body
        .get("instructions")
        .and_then(Value::as_str)
        .unwrap_or("");
    let lane = headers
        .get("x-herdr-lane")
        .map(String::as_str)
        .unwrap_or("");
    format!(
        "fallback:{}",
        sha256_hex(&format!("{instructions}\u{0}{lane}"))
    )
}

fn sha256_hex(text: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// The prompt for one turn: the first turn carries the instructions, later
/// turns carry only the new user message (Codex already holds the thread).
pub fn build_prompt(body: &Value, first: bool) -> String {
    let user = last_user_text(body);
    if !first {
        return user;
    }
    let instructions = body
        .get("instructions")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    match (instructions.is_empty(), user.trim().is_empty()) {
        (true, true) => String::new(),
        (true, false) => user,
        (false, true) => instructions.to_string(),
        (false, false) => format!("{instructions}\n\n{user}"),
    }
}

fn item_text(item: &Value) -> String {
    let content = &item["content"];
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    let mut parts: Vec<String> = Vec::new();
    if let Some(parts_array) = content.as_array() {
        for part in parts_array {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
                parts.push(text.to_string());
            }
        }
    }
    parts.join("\n")
}

fn last_user_text(body: &Value) -> String {
    match body.get("input") {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(items)) => {
            let mut last_user = String::new();
            let mut last_any = String::new();
            for item in items {
                let text = item_text(item);
                if !text.trim().is_empty() {
                    last_any = text.clone();
                }
                let role = item.get("role").and_then(Value::as_str).unwrap_or("");
                if role == "user" && !text.trim().is_empty() {
                    last_user = text;
                }
            }
            if !last_user.is_empty() {
                last_user
            } else {
                last_any
            }
        }
        _ => String::new(),
    }
}

// ---------------------------------------------------------------------------
// A very small HTTP/1.1 server. Loopback, one request per connection, chunked
// SSE for the streaming path.
// ---------------------------------------------------------------------------

struct Request {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Result<Request> {
    let mut reader = BufReader::new(stream.try_clone().context("could not clone the socket")?);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    if method.is_empty() || path.is_empty() {
        bail!("not an HTTP request line");
    }
    let mut headers = HashMap::new();
    loop {
        let mut raw = String::new();
        if reader.read_line(&mut raw)? == 0 {
            break;
        }
        if raw == "\r\n" || raw == "\n" {
            break;
        }
        if let Some((name, value)) = raw.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    let length = headers
        .get("content-length")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader
        .read_exact(&mut body)
        .context("could not read the request body")?;
    Ok(Request {
        method,
        path,
        headers,
        body,
    })
}

struct Sse<'a> {
    stream: &'a mut TcpStream,
}

impl Sse<'_> {
    fn send(&mut self, kind: &str, data: &Value) -> Result<()> {
        let payload = format!("event: {kind}\ndata: {data}\n\n");
        self.chunk(payload.as_bytes())
    }

    fn chunk(&mut self, bytes: &[u8]) -> Result<()> {
        write!(self.stream, "{:x}\r\n", bytes.len())?;
        self.stream.write_all(bytes)?;
        self.stream.write_all(b"\r\n")?;
        self.stream.flush()?;
        Ok(())
    }

    fn end(&mut self) -> Result<()> {
        self.stream.write_all(b"0\r\n\r\n")?;
        self.stream.flush()?;
        Ok(())
    }
}

fn write_sse_headers(stream: &mut TcpStream) -> Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: close\r\nTransfer-Encoding: chunked\r\n\r\n",
    )?;
    stream.flush()?;
    Ok(())
}

fn write_json(stream: &mut TcpStream, status: u16, body: &Value) -> Result<()> {
    let text = serde_json::to_string(body).unwrap_or_else(|_| "{}".into());
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        503 => "Service Unavailable",
        _ => "Error",
    };
    write!(
        stream,
        "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
        text.len()
    )?;
    stream.flush()?;
    Ok(())
}

fn write_json_error(stream: &mut TcpStream, status: u16, message: &str) -> Result<()> {
    write_json(
        stream,
        status,
        &json!({"error": {"message": message, "type": "herdr_pro_relay"}}),
    )
}

fn write_models(stream: &mut TcpStream) -> Result<()> {
    write_json(
        stream,
        200,
        &json!({
            "object": "list",
            "data": [{
                "id": MODEL_ID,
                "object": "model",
                "created": 0,
                "owned_by": "herdr-pro",
            }]
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    #[test]
    fn the_session_key_prefers_conversation_then_the_header() {
        let body = json!({"conversation": "topic-1", "instructions": "x"});
        assert_eq!(
            session_key(&headers(&[("session_id", "s1")]), &body),
            "conversation:topic-1"
        );
        let body = json!({"instructions": "x"});
        assert_eq!(
            session_key(&headers(&[("session_id", "s1")]), &body),
            "session:s1"
        );
        assert_eq!(
            session_key(&headers(&[("x-client-request-id", "s2")]), &body),
            "session:s2"
        );
        // No conversation and no session header: instructions + lane hash.
        let a = session_key(&headers(&[("x-herdr-lane", "pro")]), &body);
        let b = session_key(&headers(&[("x-herdr-lane", "pro")]), &body);
        let c = session_key(&headers(&[("x-herdr-lane", "other")]), &body);
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert!(a.starts_with("fallback:"));
    }

    #[test]
    fn the_prompt_is_the_last_user_message_and_instructions_only_on_the_first_turn() {
        let body = json!({
            "instructions": "You are Pro.",
            "input": [
                {"role": "user", "content": [{"type": "input_text", "text": "one"}]},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "first"}]},
                {"role": "user", "content": [{"type": "input_text", "text": "two"}]}
            ]
        });
        assert_eq!(build_prompt(&body, true), "You are Pro.\n\ntwo");
        assert_eq!(build_prompt(&body, false), "two");
    }

    #[test]
    fn agent_message_snapshots_become_added_deltas_and_completed() {
        let mut state = StreamState::new("resp_1".into());
        let mut events: Vec<(String, Value)> = Vec::new();
        let mut sink = |kind: &str, data: &Value| {
            events.push((kind.to_string(), data.clone()));
            Ok(())
        };
        state
            .on_event(
                &json!({"type":"thread.started","thread_id":"tid-1"}),
                &mut sink,
            )
            .unwrap();
        // An accumulated snapshot, then a fuller one, then the completed item.
        state
            .on_event(
                &json!({"type":"item.updated","item":{"id":"m1","type":"agent_message","text":"Hel"}}),
                &mut sink,
            )
            .unwrap();
        state
            .on_event(
                &json!({"type":"item.updated","item":{"id":"m1","type":"agent_message","text":"Hello"}}),
                &mut sink,
            )
            .unwrap();
        state
            .on_event(
                &json!({"type":"item.completed","item":{"id":"m1","type":"agent_message","text":"Hello world"}}),
                &mut sink,
            )
            .unwrap();
        state
            .on_event(
                &json!({"type":"turn.completed","usage":{"input_tokens":3,"output_tokens":2}}),
                &mut sink,
            )
            .unwrap();
        state.finish(&mut sink).unwrap();
        let kinds: Vec<&str> = events.iter().map(|(k, _)| k.as_str()).collect();
        assert_eq!(
            kinds,
            vec![
                "response.output_item.added",
                "response.output_text.delta",
                "response.output_text.delta",
                "response.output_text.delta",
                "response.output_item.done",
                "response.completed",
            ]
        );
        let deltas: Vec<&str> = events
            .iter()
            .filter(|(k, _)| k == "response.output_text.delta")
            .map(|(_, d)| d["delta"].as_str().unwrap())
            .collect();
        assert_eq!(deltas, vec!["Hel", "lo", " world"]);
        assert_eq!(state.answer, "Hello world");
        assert_eq!(state.codex_id.as_deref(), Some("tid-1"));
        assert!(state.completed);
        let completed = events.last().unwrap();
        assert_eq!(completed.1["response"]["usage"]["input_tokens"], 3);
    }

    #[test]
    fn a_failed_turn_is_reported() {
        let mut state = StreamState::new("resp_2".into());
        let mut noop = |_: &str, _: &Value| Ok(());
        state
            .on_event(
                &json!({"type":"turn.failed","error":{"message":"rate limit"}}),
                &mut noop,
            )
            .unwrap();
        assert_eq!(state.failure.as_deref(), Some("rate limit"));
        assert!(!state.completed);
    }
}
