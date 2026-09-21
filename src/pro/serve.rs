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
//!
//! The relay also gives the model one text-protocol tool. The bridge's
//! browser-only mode forwards no local tools ("Local tools unavailable"), so
//! the first message of every session carries [`RELAY_PROTOCOL`]: the model may
//! end an answer with `READ <path>` or `LIST <path>` lines. The relay answers
//! those on the same Codex thread under a `=== <path> ===` header and keeps the
//! request answer to itself; only a final answer reaches pi.

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
pub(crate) const MODEL_ID: &str = super::provider::MODEL_ID;

/// Read timeout while waiting for a request header block.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The folders the model may read even without `--read-root`.
const DEFAULT_READ_ROOT: &str = "/Users/rolfie/projects";
/// One refused path's answer line, whatever the reason.
const REFUSED: &str = "refused: outside the readable folders";
/// Request rounds served per pi request before the last answer is returned.
const MAX_REQUEST_ROUNDS: usize = 8;
/// One file's text cap; a larger file is cut with a note.
const FILE_MAX_BYTES: usize = 200 * 1024;
/// One round's combined text cap.
const ROUND_MAX_BYTES: usize = 1024 * 1024;
/// One `LIST`'s entry cap.
const LIST_MAX_ENTRIES: usize = 500;
/// The note the model sees when it keeps asking past the round budget.
const REQUEST_BUDGET_NOTE: &str = "[relay: the 8-request budget is spent; this answer is final]";
/// The relay-owned preamble added to the first message of every session (the
/// relay already builds that message; the shared Pro home's instruction file
/// stays the packet lane's, so this text never reaches a packet turn).
const RELAY_PROTOCOL: &str = "\
Relay access: this session can read files for you. At the very end of an
answer you may ask for files, one request per line and nothing after them:
READ <absolute path>
LIST <absolute directory>
The relay answers on this same thread with the file's text or the folder's
entries, each under a `=== <path> ===` header. Use absolute paths. Do not emit
a request line when the answer is final.";

/// `<state dir>/relay`: the relay's own files, so a failed request leaves a trace.
fn relay_dir(layout: &Layout) -> PathBuf {
    layout.root.join("relay")
}

/// `<state dir>/relay/serve.log`: one line per request and per refusal.
fn serve_log(layout: &Layout) -> PathBuf {
    relay_dir(layout).join("serve.log")
}

unsafe extern "C" {
    fn setsid() -> i32;
}

/// `serve.json`: how pi and the doctor find the relay.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct ServeState {
    pub(crate) port: u16,
    pub(crate) pid: u32,
    pub(crate) started: String,
    pub(crate) token: String,
    /// The folders the model may read, default root plus `--read-root`.
    #[serde(default)]
    pub(crate) read_roots: Vec<PathBuf>,
}

impl ServeState {
    pub(crate) fn read(layout: &Layout) -> Option<ServeState> {
        let text = std::fs::read_to_string(layout.serve_state()).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub(crate) fn write(&self, layout: &Layout) -> Result<()> {
        let path = layout.serve_state();
        let text = serde_json::to_string_pretty(self).context("could not serialize serve.json")?;
        state::write_atomic(&path, &format!("{text}\n"))?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("could not protect {}", path.display()))
    }

    pub(crate) fn remove(layout: &Layout) {
        let _ = std::fs::remove_file(layout.serve_state());
    }
}

/// True when a process with this pid exists.
pub(crate) fn pid_alive(pid: u32) -> bool {
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
pub(crate) fn start(layout: &Layout, read_roots: &[PathBuf]) -> Result<bool> {
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
    let mut args: Vec<String> = vec!["serve-run".into()];
    for root in read_roots {
        args.push("--read-root".into());
        args.push(root.display().to_string());
    }
    let mut command = Command::new(exe);
    command
        .env("HERDR_PRO_STATE_DIR", layout.root.display().to_string())
        .args(&args)
        .current_dir(layout.root.parent().unwrap_or(&layout.root))
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

/// The effective readable roots: the default plus every `--read-root`, each
/// resolved so a symlinked root and a canonical path still match. Duplicates
/// are dropped.
fn effective_read_roots(extra: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from(DEFAULT_READ_ROOT)];
    roots.extend(extra.iter().cloned());
    let mut resolved: Vec<PathBuf> = Vec::new();
    for root in roots {
        let root = std::fs::canonicalize(&root).unwrap_or(root);
        if !resolved.contains(&root) {
            resolved.push(root);
        }
    }
    resolved
}

/// The readable roots recorded in `serve.json`, or the default when there is
/// no relay state yet.
pub(crate) fn readable_roots(layout: &Layout) -> Vec<PathBuf> {
    match ServeState::read(layout) {
        Some(state) if !state.read_roots.is_empty() => state.read_roots,
        _ => vec![PathBuf::from(DEFAULT_READ_ROOT)],
    }
}

/// The last non-empty `serve.log` line, for the doctor's relay row.
pub(crate) fn last_log_line(layout: &Layout) -> Option<String> {
    let text = std::fs::read_to_string(serve_log(layout)).ok()?;
    text.lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(str::to_string)
}

/// `herdr-pro serve-run`: the foreground server (started detached by `serve`).
pub(crate) fn run(layout: &Layout, env: &Env, extra_roots: &[PathBuf]) -> Result<()> {
    layout.ensure()?;
    let read_roots = effective_read_roots(extra_roots);
    let inflight_limit = env.inflight_limit()?;
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
        read_roots: read_roots.clone(),
    }
    .write(layout)?;
    // The relay's own cwd, trusted so Codex never prompts for it.
    super::home::trust(layout, &layout.root).context("could not trust the relay cwd")?;
    write_provider(layout, port, &token)?;
    let bridge_port = BRIDGE_PORT;
    let relay = Arc::new(Relay {
        layout: layout.clone(),
        token,
        port,
        sessions: Mutex::new(HashMap::new()),
        inflight: Mutex::new(HashMap::new()),
        inflight_limit,
        failures: Mutex::new(Vec::new()),
        read_roots,
        codex: Box::new(RealCodex {
            env: env.clone(),
            layout: layout.clone(),
            bridge_port,
            seq: AtomicU64::new(0),
        }),
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
pub(crate) fn stop(layout: &Layout) -> Result<bool> {
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
pub(crate) fn models_health(runner: &dyn Runner, layout: &Layout) -> Result<String> {
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
pub(crate) fn write_provider(layout: &Layout, port: u16, token: &str) -> Result<()> {
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

/// What one relay turn produced: the final Codex outcome plus what the model
/// asked for along the way.
#[derive(Debug, Clone)]
struct RelayTurn {
    outcome: CodexOutcome,
    rounds: usize,
    files: usize,
}

/// A failed Codex turn plus the reads already served for this pi request.
#[derive(Debug)]
struct RelayFailure {
    error: anyhow::Error,
    rounds: usize,
    files: usize,
}

/// One Codex turn. The real one spawns `codex exec`; the tests script answers.
trait Codex: Send + Sync {
    fn turn(
        &self,
        resume: Option<&str>,
        prompt: &str,
        sink: &mut dyn FnMut(&str, &Value) -> Result<()>,
    ) -> Result<CodexOutcome>;
}

struct Relay {
    layout: Layout,
    token: String,
    port: u16,
    sessions: Mutex<HashMap<String, Session>>,
    inflight: Mutex<HashMap<String, ()>>,
    inflight_limit: usize,
    failures: Mutex<Vec<Instant>>,
    read_roots: Vec<PathBuf>,
    codex: Box<dyn Codex>,
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
        let bytes_in = request.body.len();

        // Fail closed before spending a Codex turn.
        if state::cooldown_active(&self.layout, jiff::Timestamp::now()) {
            let until = state::cooldown_until(&self.layout)
                .map(|u| u.to_string())
                .unwrap_or_default();
            let _ = self.record_request(&key, bytes_in, "cooldown", 0, 0, None);
            return write_json_error(
                stream,
                503,
                &format!(
                    "the Pro breaker is active until {until}; clear it with `herdr-pro resume-bridge`"
                ),
            );
        }
        let limit = self.inflight_limit;
        {
            let mut inflight = self.inflight.lock().unwrap_or_else(|e| e.into_inner());
            if inflight.contains_key(&key) {
                let _ = self.record_request(&key, bytes_in, "busy", 0, 0, None);
                return write_json_error(
                    stream,
                    429,
                    "a turn is already running for this conversation",
                );
            }
            if inflight.len() >= limit {
                let _ = self.record_request(&key, bytes_in, "busy", 0, 0, None);
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
            self.stream_turn(stream, &key, resume.as_deref(), &prompt, bytes_in)
        } else {
            self.json_turn(stream, &key, resume.as_deref(), &prompt, bytes_in)
        };
        drop(guard);
        result
    }

    /// Run the Codex turns for one pi request, serving the model's file
    /// requests until it produces a final answer.
    fn converse(
        &self,
        key: &str,
        resume: Option<&str>,
        prompt: &str,
    ) -> std::result::Result<RelayTurn, RelayFailure> {
        let mut resume_id = resume.map(str::to_string);
        let mut prompt = prompt.to_string();
        let mut rounds = 0usize;
        let mut files = 0usize;
        loop {
            let mut noop = |_: &str, _: &Value| Ok(());
            let mut outcome = self
                .codex
                .turn(resume_id.as_deref(), &prompt, &mut noop)
                .map_err(|error| RelayFailure {
                    error,
                    rounds,
                    files,
                })?;
            if let Some(id) = &outcome.codex_id {
                resume_id = Some(id.clone());
            }
            outcome.answer = strip_bridge_note(&unescape_bridge_markdown(&outcome.answer));
            let (body, requests) = split_requests(&outcome.answer);
            if requests.is_empty() {
                outcome.answer = body;
                return Ok(RelayTurn {
                    outcome,
                    rounds,
                    files,
                });
            }
            if rounds >= MAX_REQUEST_ROUNDS {
                outcome.answer = if body.is_empty() {
                    REQUEST_BUDGET_NOTE.to_string()
                } else {
                    format!("{body}\n\n{REQUEST_BUDGET_NOTE}")
                };
                return Ok(RelayTurn {
                    outcome,
                    rounds,
                    files,
                });
            }
            let (message, served) = self.execute_requests(key, &requests);
            files += served;
            rounds += 1;
            prompt = message;
        }
    }

    /// Answer the model's requests as one follow-up message. Returns the
    /// message and the number of files or folders actually served.
    fn execute_requests(&self, key: &str, requests: &[FileRequest]) -> (String, usize) {
        let mut sections: Vec<String> = Vec::new();
        let mut budget = ROUND_MAX_BYTES;
        let mut served = 0usize;
        for request in requests {
            let label = request.path().display().to_string();
            let (text, ok) = self.one_request(request, &mut budget);
            if ok {
                served += 1;
            } else {
                let _ = state::append_line(
                    &serve_log(&self.layout),
                    &format!("{} session={key} refused {label}", super::now_rfc3339()),
                );
            }
            sections.push(format!("=== {label} ===\n{text}"));
        }
        (sections.join("\n\n"), served)
    }

    fn one_request(&self, request: &FileRequest, budget: &mut usize) -> (String, bool) {
        if *budget == 0 {
            return ("[relay: this round's read budget is spent]".into(), false);
        }
        let raw = request.path().display().to_string();
        let path = match self.resolve_allowed(&raw) {
            Ok(path) => path,
            Err(refusal) => return (refusal, false),
        };
        let (text, ok) = match request {
            FileRequest::Read(_) => self.render_read(&path, *budget),
            FileRequest::List(_) => self.render_list(&path, *budget),
        };
        *budget = budget.saturating_sub(text.len());
        (text, ok)
    }

    /// Resolve a requested path, or return the one refusal line.
    fn resolve_allowed(&self, raw: &str) -> std::result::Result<PathBuf, String> {
        let path = Path::new(raw);
        if !path.is_absolute() || forbidden_path(path) {
            return Err(REFUSED.to_string());
        }
        let Ok(canonical) = std::fs::canonicalize(path) else {
            return Err(format!("error: could not read {raw}"));
        };
        if !self
            .read_roots
            .iter()
            .any(|root| canonical.starts_with(root))
        {
            return Err(REFUSED.to_string());
        }
        if forbidden_path(&canonical) {
            return Err(REFUSED.to_string());
        }
        Ok(canonical)
    }

    fn render_read(&self, path: &Path, budget: usize) -> (String, bool) {
        let limit = FILE_MAX_BYTES.min(budget);
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(error) => return (format!("error: {error}"), false),
        };
        let mut buf = Vec::new();
        if let Err(error) = file.take((limit + 1) as u64).read_to_end(&mut buf) {
            return (format!("error: {error}"), false);
        }
        if buf.contains(&0) {
            return ("refused: binary file".into(), false);
        }
        let over_file = buf.len() > FILE_MAX_BYTES;
        if buf.len() > limit {
            buf.truncate(limit);
        }
        let text = match String::from_utf8(buf) {
            Ok(text) => text,
            Err(error) if error.utf8_error().error_len().is_none() => {
                let valid = error.utf8_error().valid_up_to();
                // The byte cap may split the last UTF-8 character. Keep the
                // complete prefix instead of calling an ordinary text file binary.
                String::from_utf8_lossy(&error.into_bytes()[..valid]).into_owned()
            }
            Err(_) => return ("refused: binary file".into(), false),
        };
        let mut text = text;
        if over_file {
            text.push_str(&format!(
                "\n[truncated: the file is larger than {} KB]",
                FILE_MAX_BYTES / 1024
            ));
        } else if limit < FILE_MAX_BYTES {
            text.push_str("\n[truncated: this round's read budget is spent]");
        }
        (text, true)
    }

    fn render_list(&self, path: &Path, budget: usize) -> (String, bool) {
        let entries = match std::fs::read_dir(path) {
            Ok(entries) => entries,
            Err(error) => return (format!("error: {error}"), false),
        };
        let mut rows: Vec<(String, String, String)> = Vec::new();
        for entry in entries.flatten().take(LIST_MAX_ENTRIES + 1) {
            let name = entry.file_name().to_string_lossy().into_owned();
            let (kind, size) = match entry.metadata() {
                Ok(meta) if meta.is_dir() => ("dir".to_string(), String::new()),
                Ok(meta) if meta.is_file() => ("file".to_string(), meta.len().to_string()),
                Ok(_) => ("other".to_string(), String::new()),
                Err(_) => ("?".to_string(), String::new()),
            };
            rows.push((name, kind, size));
        }
        rows.sort();
        let cut = rows.len() > LIST_MAX_ENTRIES;
        rows.truncate(LIST_MAX_ENTRIES);
        let mut lines: Vec<String> = rows
            .into_iter()
            .map(|(name, kind, size)| {
                if size.is_empty() {
                    format!("{name}\t{kind}")
                } else {
                    format!("{name}\t{kind}\t{size}")
                }
            })
            .collect();
        if cut {
            lines.push(format!(
                "[more than {LIST_MAX_ENTRIES} entries; the list was cut]"
            ));
        }
        let text = fit_with_note(
            &lines.join("\n"),
            budget,
            "[truncated: this round's read budget is spent]",
        );
        (text, true)
    }

    /// Append the one log line and the `usage.jsonl` line for one request.
    #[allow(clippy::too_many_arguments)]
    fn record_request(
        &self,
        key: &str,
        bytes_in: usize,
        outcome: &str,
        rounds: usize,
        files: usize,
        codex_exit: Option<i32>,
    ) -> Result<()> {
        let exit = codex_exit
            .map(|code| code.to_string())
            .unwrap_or_else(|| "-".into());
        state::append_line(
            &serve_log(&self.layout),
            &format!(
                "{} session={key} bytes_in={bytes_in} rounds={rounds} files={files} outcome={outcome} codex_exit={exit}",
                super::now_rfc3339()
            ),
        )?;
        let usage = json!({
            "ts": super::now_rfc3339(),
            "relay": true,
            "session": key,
            "rounds": rounds,
            "files": files,
            "outcome": outcome,
        });
        state::append_line(&self.layout.usage(), &usage.to_string())
    }

    fn stream_turn(
        &self,
        stream: &mut TcpStream,
        key: &str,
        resume: Option<&str>,
        prompt: &str,
        bytes_in: usize,
    ) -> Result<()> {
        write_sse_headers(stream)?;
        let response_id = new_response_id();
        let mut sse = Sse { stream };
        sse.send(
            "response.created",
            &json!({"type":"response.created","response":{"id":response_id,"status":"in_progress"}}),
        )?;
        match self.converse(key, resume, prompt) {
            Ok(turn) => {
                self.remember(key, &turn.outcome);
                let _ = self.record_request(
                    key,
                    bytes_in,
                    "ok",
                    turn.rounds,
                    turn.files,
                    turn.outcome.exit_code,
                );
                let answer = turn.outcome.answer.clone();
                let message = json!({
                    "id": turn.outcome.message_id,
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": answer, "annotations": []}]
                });
                sse.send(
                    "response.output_item.added",
                    &json!({
                        "type":"response.output_item.added",
                        "output_index": 0,
                        "item":{"id":turn.outcome.message_id,"type":"message","role":"assistant","status":"in_progress","content":[]}
                    }),
                )?;
                if !answer.is_empty() {
                    sse.send(
                        "response.output_text.delta",
                        &json!({
                            "type":"response.output_text.delta",
                            "output_index": 0,
                            "delta": answer
                        }),
                    )?;
                }
                sse.send(
                    "response.output_item.done",
                    &json!({
                        "type":"response.output_item.done",
                        "output_index": 0,
                        "item": message
                    }),
                )?;
                sse.send(
                    "response.completed",
                    &json!({
                        "type":"response.completed",
                        "response":{
                            "id": response_id,
                            "object": "response",
                            "status": "completed",
                            "output": [message],
                            "usage": turn.outcome.usage,
                        }
                    }),
                )?;
                sse.end()
            }
            Err(failure) => {
                let message = self.failure_message(&failure.error);
                let _ = self.record_request(
                    key,
                    bytes_in,
                    "failed",
                    failure.rounds,
                    failure.files,
                    None,
                );
                let _ = sse.send(
                    "response.failed",
                    &json!({
                        "type":"response.failed",
                        "response":{"status":"failed","error":{"code":"codex_error","message":message}}
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
        bytes_in: usize,
    ) -> Result<()> {
        match self.converse(key, resume, prompt) {
            Ok(turn) => {
                self.remember(key, &turn.outcome);
                let _ = self.record_request(
                    key,
                    bytes_in,
                    "ok",
                    turn.rounds,
                    turn.files,
                    turn.outcome.exit_code,
                );
                write_json(
                    stream,
                    200,
                    &json!({
                        "id": turn.outcome.response_id,
                        "object": "response",
                        "status": "completed",
                        "output": [{
                            "id": turn.outcome.message_id,
                            "type": "message",
                            "role": "assistant",
                            "status": "completed",
                            "content": [{"type": "output_text", "text": turn.outcome.answer, "annotations": []}]
                        }],
                        "usage": turn.outcome.usage,
                    }),
                )
            }
            Err(failure) => {
                let message = self.failure_message(&failure.error);
                let _ = self.record_request(
                    key,
                    bytes_in,
                    "failed",
                    failure.rounds,
                    failure.files,
                    None,
                );
                write_json(
                    stream,
                    200,
                    &json!({
                        "status": "failed",
                        "error": {"code": "codex_error", "message": message}
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

    fn record_failure(&self) -> Result<()> {
        let now = Instant::now();
        let mut failures = self.failures.lock().unwrap_or_else(|e| e.into_inner());
        failures.retain(|at| now.duration_since(*at) <= FAILURE_WINDOW);
        failures.push(now);
        if failures.len() >= 2 {
            let until = jiff::Timestamp::now()
                + jiff::SignedDuration::from_secs(super::COOLDOWN.as_secs() as i64);
            state::set_cooldown(&self.layout, until).context("could not set the Pro breaker")?;
        }
        Ok(())
    }

    fn failure_message(&self, error: &anyhow::Error) -> String {
        let message = format!("{error:#}");
        match self.record_failure() {
            Ok(()) => message,
            Err(breaker) => format!("{message}; {breaker:#}"),
        }
    }
}

/// The real Codex: one `codex exec --json` turn per call.
struct RealCodex {
    env: Env,
    layout: Layout,
    bridge_port: u16,
    seq: AtomicU64,
}

impl RealCodex {
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
        let mut outcome = state.into_outcome();
        outcome.exit_code = status.code();
        Ok(outcome)
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

impl Codex for RealCodex {
    fn turn(
        &self,
        resume: Option<&str>,
        prompt: &str,
        sink: &mut dyn FnMut(&str, &Value) -> Result<()>,
    ) -> Result<CodexOutcome> {
        self.run_codex(resume, prompt, sink)
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
pub(crate) struct CodexOutcome {
    pub(crate) response_id: String,
    pub(crate) message_id: String,
    pub(crate) answer: String,
    pub(crate) codex_id: Option<String>,
    pub(crate) usage: Value,
    /// The `codex exec` process exit code, for the relay log.
    pub(crate) exit_code: Option<i32>,
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
            exit_code: None,
        }
    }
}

/// The session key: an explicit `conversation`, else the pi session header,
/// else the `instructions` hash plus `X-Herdr-Lane`.
fn session_key(headers: &HashMap<String, String>, body: &Value) -> String {
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

/// The prompt for one turn: the first turn carries the relay protocol and the
/// pi instructions, later turns carry only the new user message (Codex already
/// holds the thread).
fn build_prompt(body: &Value, first: bool) -> String {
    let user = last_user_text(body);
    if !first {
        return user;
    }
    let instructions = body
        .get("instructions")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    let mut parts: Vec<&str> = vec![RELAY_PROTOCOL];
    if !instructions.is_empty() {
        parts.push(instructions);
    }
    if !user.trim().is_empty() {
        parts.push(user.trim());
    }
    parts.join("\n\n")
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
// The relay's one text protocol: trailing request lines in an answer.
// ---------------------------------------------------------------------------

/// One line the model may put at the end of an answer.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum FileRequest {
    Read(PathBuf),
    List(PathBuf),
}

impl FileRequest {
    pub(crate) fn path(&self) -> &Path {
        match self {
            FileRequest::Read(path) | FileRequest::List(path) => path,
        }
    }
}

/// `READ <absolute path>` or `LIST <absolute directory>`, else `None`.
fn parse_request(line: &str) -> Option<FileRequest> {
    let line = line.trim();
    for (keyword, list) in [("READ ", false), ("LIST ", true)] {
        if let Some(rest) = line.strip_prefix(keyword) {
            let path = rest.trim();
            if !path.is_empty() {
                return Some(if list {
                    FileRequest::List(PathBuf::from(path))
                } else {
                    FileRequest::Read(PathBuf::from(path))
                });
            }
        }
    }
    None
}

/// Split the trailing request lines off an answer. Returns the answer body
/// without them and the requests in the order the model wrote them.
fn split_requests(answer: &str) -> (String, Vec<FileRequest>) {
    let mut lines: Vec<&str> = answer.lines().collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let mut requests: Vec<FileRequest> = Vec::new();
    while let Some(line) = lines.last() {
        match parse_request(line) {
            Some(request) => {
                requests.push(request);
                lines.pop();
            }
            None => break,
        }
    }
    requests.reverse();
    (lines.join("\n").trim_end().to_string(), requests)
}

/// Remove the markdown escapes the web bridge adds to model text.
///
/// Only punctuation observed from the bridge is unescaped; ordinary
/// backslashes (including those in paths) stay untouched.
fn unescape_bridge_markdown(answer: &str) -> String {
    let mut chars = answer.chars().peekable();
    let mut plain = String::with_capacity(answer.len());
    while let Some(ch) = chars.next() {
        if ch == '\\'
            && chars.peek().is_some_and(|next| {
                matches!(
                    next,
                    '=' | '.'
                        | '-'
                        | '_'
                        | '*'
                        | '['
                        | ']'
                        | '#'
                        | '>'
                        | '|'
                        | '('
                        | ')'
                        | '+'
                        | '!'
                        | '`'
                )
            })
        {
            plain.push(chars.next().expect("peeked character exists"));
        } else {
            plain.push(ch);
        }
    }
    plain
}

/// Drop the bridge's "Local tools unavailable" blockquote: the leading `>`
/// lines and the blank line that closes them.
fn strip_bridge_note(answer: &str) -> String {
    let mut lines = answer.lines().peekable();
    while lines.peek().is_some_and(|line| line.starts_with('>')) {
        lines.next();
    }
    if lines.peek().is_some_and(|line| line.trim().is_empty()) {
        lines.next();
    }
    lines.collect::<Vec<&str>>().join("\n")
}

/// A path the relay never serves: the credential names and `.git/objects`.
fn forbidden_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    for needle in ["auth.json", ".env", "id_", ".pem", "token"] {
        if text.contains(needle) {
            return true;
        }
    }
    let parts: Vec<String> = path
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect();
    parts
        .windows(2)
        .any(|pair| pair[0] == ".git" && pair[1] == "objects")
}

/// Cut UTF-8 text to a byte budget and keep the truncation note inside it.
fn fit_with_note(text: &str, limit: usize, note: &str) -> String {
    if text.len() <= limit {
        return text.to_string();
    }
    if limit <= note.len() {
        let mut end = limit;
        while end > 0 && !note.is_char_boundary(end) {
            end -= 1;
        }
        return note[..end].to_string();
    }
    let content_limit = limit - note.len() - 1;
    let mut end = content_limit;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}\n{note}", &text[..end])
}

fn new_response_id() -> String {
    format!(
        "resp_{}",
        hex(&std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
            .to_be_bytes())
    )
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
        assert_eq!(
            build_prompt(&body, true),
            format!("{RELAY_PROTOCOL}\n\nYou are Pro.\n\ntwo")
        );
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

    // --- the request-line protocol, driven by a scripted Codex -------------

    use std::collections::VecDeque;

    struct FakeInner {
        answers: Mutex<VecDeque<String>>,
        prompts: Mutex<Vec<String>>,
        resumes: Mutex<Vec<Option<String>>>,
    }

    #[derive(Clone)]
    struct FakeCodex(Arc<FakeInner>);

    impl FakeCodex {
        fn new(answers: Vec<String>) -> FakeCodex {
            FakeCodex(Arc::new(FakeInner {
                answers: Mutex::new(answers.into()),
                prompts: Mutex::new(Vec::new()),
                resumes: Mutex::new(Vec::new()),
            }))
        }

        fn prompt_count(&self) -> usize {
            self.0.prompts.lock().unwrap().len()
        }

        fn prompt(&self, index: usize) -> String {
            self.0.prompts.lock().unwrap()[index].clone()
        }

        fn resume(&self, index: usize) -> Option<String> {
            self.0.resumes.lock().unwrap()[index].clone()
        }
    }

    impl Codex for FakeCodex {
        fn turn(
            &self,
            resume: Option<&str>,
            prompt: &str,
            sink: &mut dyn FnMut(&str, &Value) -> Result<()>,
        ) -> Result<CodexOutcome> {
            self.0.prompts.lock().unwrap().push(prompt.to_string());
            self.0
                .resumes
                .lock()
                .unwrap()
                .push(resume.map(str::to_string));
            let answer = self
                .0
                .answers
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default();
            sink(
                "response.created",
                &json!({"type":"response.created","response":{"id":"resp_test","status":"in_progress"}}),
            )?;
            Ok(CodexOutcome {
                response_id: "resp_test".into(),
                message_id: "msg_test".into(),
                answer,
                codex_id: Some("tid-1".into()),
                usage: json!({}),
                exit_code: Some(0),
            })
        }
    }

    fn relay_with(root: &Path, answers: Vec<String>) -> (Arc<Relay>, FakeCodex) {
        let layout = Layout::for_test(root.join("pro"));
        layout.ensure().unwrap();
        let read_roots = effective_read_roots(&[root.to_path_buf()]);
        let codex = FakeCodex::new(answers);
        let relay = Arc::new(Relay {
            layout,
            token: "test-token".into(),
            port: 0,
            sessions: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            inflight_limit: 2,
            failures: Mutex::new(Vec::new()),
            read_roots,
            codex: Box::new(codex.clone()),
        });
        (relay, codex)
    }

    #[test]
    fn trailing_requests_are_split_and_only_the_final_answer_returns() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.md"), "alpha").unwrap();
        std::fs::write(root.join("b.md"), "beta").unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/inner.md"), "inner").unwrap();
        let first = format!(
            "let me look\nREAD {}\nREAD {}\nLIST {}",
            root.join("a.md").display(),
            root.join("b.md").display(),
            root.join("sub").display()
        );
        let (relay, codex) = relay_with(root, vec![first, "final answer".into()]);
        let turn = relay.converse("session:test", None, "hello").unwrap();
        assert_eq!(turn.outcome.answer, "final answer");
        assert_eq!(turn.rounds, 1);
        assert_eq!(turn.files, 3);
        assert_eq!(codex.prompt_count(), 2);
        assert_eq!(codex.resume(1).as_deref(), Some("tid-1"));
        let follow_up = codex.prompt(1);
        let sections = follow_up
            .lines()
            .filter(|line| line.starts_with("=== "))
            .count();
        assert_eq!(sections, 3, "{follow_up}");
        assert!(follow_up.contains("alpha"), "{follow_up}");
        assert!(follow_up.contains("beta"), "{follow_up}");
        assert!(follow_up.contains("inner.md"), "{follow_up}");
    }

    #[test]
    fn a_refused_path_is_one_line_and_the_next_answer_returns() {
        let dir = tempfile::tempdir().unwrap();
        let (relay, codex) = relay_with(
            dir.path(),
            vec!["let me look\nREAD /etc/passwd".into(), "done".into()],
        );
        let turn = relay.converse("session:test", None, "hello").unwrap();
        assert_eq!(turn.outcome.answer, "done");
        assert_eq!(turn.rounds, 1);
        assert_eq!(turn.files, 0);
        let follow_up = codex.prompt(1);
        assert!(follow_up.contains("=== /etc/passwd ==="), "{follow_up}");
        assert!(follow_up.contains(REFUSED), "{follow_up}");
    }

    #[test]
    fn the_round_budget_stops_after_eight_follow_ups() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("a.md"), "alpha").unwrap();
        let answers: Vec<String> = (1..=9)
            .map(|i| format!("turn {i}\nREAD {}", root.join("a.md").display()))
            .collect();
        let (relay, codex) = relay_with(root, answers);
        let turn = relay.converse("session:test", None, "hello").unwrap();
        assert_eq!(turn.rounds, MAX_REQUEST_ROUNDS);
        assert_eq!(turn.files, MAX_REQUEST_ROUNDS);
        assert_eq!(codex.prompt_count(), MAX_REQUEST_ROUNDS + 1);
        assert!(turn.outcome.answer.ends_with(REQUEST_BUDGET_NOTE));
    }

    #[test]
    fn the_bridge_note_is_stripped() {
        assert_eq!(
            strip_bridge_note("> Local tools unavailable\n>\nactual answer"),
            "actual answer"
        );
        assert_eq!(
            strip_bridge_note("> Local tools unavailable\n\nactual answer"),
            "actual answer"
        );
        assert_eq!(strip_bridge_note("plain answer"), "plain answer");
    }

    #[test]
    fn bridge_markdown_escapes_are_removed_before_requests_and_final_text() {
        assert_eq!(
            unescape_bridge_markdown("\\=\\.\\-\\_\\*\\[\\]\\#\\>\\|\\(\\)\\+\\!\\` and \\q"),
            "=.-_*[]#>|()+!` and \\q"
        );
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = root.join("a.md");
        std::fs::write(&file, "alpha").unwrap();
        let escaped_path = file.display().to_string().replace('.', "\\.");
        let first = format!("\\> Local tools unavailable\n\\>\n\nlooking\nREAD {escaped_path}");
        let (relay, codex) = relay_with(
            root,
            vec![first, "\\#\\# 1\\. Goal\n\\=== FILE: a\\.md ===".into()],
        );

        let turn = relay.converse("session:test", None, "hello").unwrap();

        assert_eq!(turn.rounds, 1);
        assert_eq!(turn.files, 1);
        assert!(codex.prompt(1).contains("alpha"), "{}", codex.prompt(1));
        assert_eq!(turn.outcome.answer, "## 1. Goal\n=== FILE: a.md ===");
    }

    #[test]
    fn a_file_over_the_cap_is_truncated_with_a_note() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let big = root.join("big.txt");
        std::fs::write(&big, "a".repeat(FILE_MAX_BYTES + 1024)).unwrap();
        let (relay, codex) = relay_with(
            root,
            vec![format!("need it\nREAD {}", big.display()), "ok".into()],
        );
        let turn = relay.converse("session:test", None, "hello").unwrap();
        assert_eq!(turn.outcome.answer, "ok");
        assert_eq!(turn.files, 1);
        assert!(
            codex
                .prompt(1)
                .contains("[truncated: the file is larger than 200 KB]"),
            "{}",
            codex.prompt(1)
        );
    }

    #[test]
    fn a_multibyte_character_at_the_file_cap_is_text_not_binary() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = root.join("utf8.txt");
        std::fs::write(&file, "€".repeat(FILE_MAX_BYTES / 3 + 2)).unwrap();
        let (relay, _) = relay_with(root, Vec::new());

        let (text, served) = relay.render_read(&file, FILE_MAX_BYTES);

        assert!(served);
        assert!(text.ends_with("[truncated: the file is larger than 200 KB]"));
    }

    #[test]
    fn a_list_uses_only_the_round_budget_left() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for index in 0..20 {
            std::fs::write(root.join(format!("long-entry-{index:02}.txt")), "x").unwrap();
        }
        let (relay, _) = relay_with(root, Vec::new());

        let (text, served) = relay.render_list(root, 80);

        assert!(served);
        assert!(text.len() <= 80, "{} bytes: {text}", text.len());
        assert!(text.ends_with("[truncated: this round's read budget is spent]"));
    }

    #[test]
    fn credential_names_and_git_objects_are_forbidden() {
        let root = Path::new("/Users/rolfie/projects");
        for name in ["auth.json", ".env", "id_rsa", "key.pem", "token.txt"] {
            assert!(forbidden_path(&root.join(name)), "{name} was not refused");
        }
        assert!(forbidden_path(&root.join("x/.git/objects/ab/cd")));
        assert!(!forbidden_path(&root.join("readme.md")));

        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("readme.md");
        let alias = dir.path().join("token-link");
        std::fs::write(&target, "public").unwrap();
        std::os::unix::fs::symlink(&target, &alias).unwrap();
        let (relay, _) = relay_with(dir.path(), Vec::new());
        assert_eq!(
            relay.resolve_allowed(&alias.display().to_string()),
            Err(REFUSED.to_string())
        );
    }
}
