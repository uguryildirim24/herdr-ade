//! `herdr-pro image`: one picture from Codex's own image tool.
//!
//! The picture lane runs on Codex's own backend with `--profile gpt-image-gen`
//! (no bridge route). `image` starts or reuses the lane for the caller's cwd,
//! submits exactly one request, waits for the PNG Codex writes under
//! `<home>/generated_images/`, saves it to `--out`, and stops the lane unless
//! `--keep`.
//!
//! Codex 0.155 keeps the paginated thread store, so a profile lane may write no
//! JSONL rollout at startup: the generated image file is the completion signal.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{Context, Result, bail};

use super::herdr_cli;
use super::home;
use super::lane;
use super::sh::Runner;
use super::state::{self, Lane};
use super::{Env, Layout};

/// A picture can take a few minutes; this is the ceiling for one call.
const IMAGE_TIMEOUT: Duration = Duration::from_secs(20 * 60);
/// How often the generated-image folder and the lane status are checked.
const POLL: Duration = Duration::from_secs(2);
/// The most reference pictures one call may attach (`--with`).
const MAX_WITH: usize = 4;

/// A picture request: `herdr-pro image`.
#[derive(Debug, Clone)]
pub(crate) struct ImageOptions {
    pub(crate) prompt_file: PathBuf,
    pub(crate) size: String,
    pub(crate) out: PathBuf,
    /// Reference pictures Codex sees before it draws.
    pub(crate) with: Vec<PathBuf>,
    pub(crate) keep: bool,
}

/// `herdr-pro image`: one lane, one request, one PNG.
pub(crate) fn run(
    env: &Env,
    layout: &Layout,
    runner: &dyn Runner,
    opts: &ImageOptions,
) -> Result<PathBuf> {
    layout.ensure()?;
    // One picture call at a time, so one lane serves one caller.
    let _lock = state::FileLock::acquire(&layout.image_lock())?;
    let (width, height) = parse_size(&opts.size)?;
    let prompt = std::fs::read_to_string(&opts.prompt_file)
        .with_context(|| format!("could not read the prompt {}", opts.prompt_file.display()))?;
    let out = std::path::absolute(&opts.out)
        .with_context(|| format!("bad output path {}", opts.out.display()))?;
    if out.exists() {
        bail!("refused: {} already exists", out.display());
    }
    let cwd = std::env::current_dir().context("could not read the current directory")?;
    let cwd = std::path::absolute(&cwd).with_context(|| format!("bad cwd {}", cwd.display()))?;
    let pictures = reference_pictures(&opts.with)?;

    // Codex accepts pictures only on its process start line. A call with
    // references therefore needs a fresh lane even when `--keep` left an
    // earlier picture lane ready.
    let name = lane_name(layout, &cwd, !pictures.is_empty())?;
    let lane = ensure_lane(env, layout, runner, &name, &cwd, &pictures)?;

    let result: Result<PathBuf> = (|| {
        let request = request(&prompt, width, height, !pictures.is_empty());
        let started = SystemTime::now();
        herdr_cli::agent_prompt(runner, &env.herdr_bin(), &lane.name, &request)
            .context("could not send the picture request")?;
        let saved = wait_for_image(env, runner, &lane, started)?;
        save_into(&saved, &out)?;
        Ok(out)
    })();
    if !opts.keep
        && let Err(stop_error) = lane::stop(env, layout, runner, &lane.name)
    {
        return match result {
            Ok(_) => Err(stop_error.context("the picture was saved, but its lane did not stop")),
            Err(error) => Err(error.context(format!(
                "the picture lane also did not stop: {stop_error:#}"
            ))),
        };
    }
    result
}

/// The `--with` reference pictures: at most [`MAX_WITH`], each an existing,
/// readable file. A bad one is refused before the lane starts.
fn reference_pictures(files: &[PathBuf]) -> Result<Vec<PathBuf>> {
    if files.len() > MAX_WITH {
        bail!(
            "refused: --with takes at most {MAX_WITH} pictures, got {}",
            files.len()
        );
    }
    let mut pictures = Vec::new();
    for file in files {
        let path = std::path::absolute(file)
            .with_context(|| format!("bad --with path {}", file.display()))?;
        if !path.is_file() {
            bail!("refused: --with {} is not a readable file", path.display());
        }
        std::fs::File::open(&path)
            .with_context(|| format!("refused: --with {} is not readable", path.display()))?;
        pictures.push(path);
    }
    Ok(pictures)
}

/// The one-line request typed into the lane.
fn request(prompt: &str, width: u32, height: u32, attached: bool) -> String {
    let pictures = if attached {
        " The attached pictures show the current screen; keep its palette and layout."
    } else {
        ""
    };
    format!(
        "Make one picture. Prompt: {}. Size {width}x{height}.{pictures} Call the image tool exactly once and reply with one line naming the picture.",
        prompt.split_whitespace().collect::<Vec<_>>().join(" ")
    )
}

/// `WxH`, both positive.
fn parse_size(size: &str) -> Result<(u32, u32)> {
    let lower = size.to_ascii_lowercase();
    let (width, height) = lower
        .split_once('x')
        .with_context(|| format!("size `{size}` must look like 1536x1024"))?;
    let width: u32 = width
        .trim()
        .parse()
        .with_context(|| format!("size `{size}` has no width"))?;
    let height: u32 = height
        .trim()
        .parse()
        .with_context(|| format!("size `{size}` has no height"))?;
    if width == 0 || height == 0 {
        bail!("size `{size}` must be positive");
    }
    Ok((width, height))
}

/// The picture lane for `cwd`: an existing one, else the first free name near
/// `gpt-image-gen`.
fn lane_name(layout: &Layout, cwd: &Path, fresh: bool) -> Result<String> {
    let base = home::IMAGE_PROFILE;
    let cwd_text = cwd.display().to_string();
    if !fresh {
        for lane in Lane::list(layout)? {
            if !lane.stopped
                && lane.cwd == cwd_text
                && lane.profile.as_deref() == Some(home::IMAGE_PROFILE)
                && (lane.name == base || lane.name.starts_with(&format!("{base}-")))
            {
                return Ok(lane.name);
            }
        }
    }
    if name_available(layout, base) {
        return Ok(base.to_string());
    }
    for n in 2..1000u32 {
        let candidate = format!("{base}-{n}");
        if name_available(layout, &candidate) {
            return Ok(candidate);
        }
    }
    bail!("no free picture lane name near `{base}`")
}

/// A gone lane record can be replaced, but a stopped record is permanent: the
/// stop switch deliberately makes `lane::start` refuse that name.
fn name_available(layout: &Layout, name: &str) -> bool {
    Lane::read(layout, name)
        .map(|lane| !lane.stopped && lane.state == "gone")
        .unwrap_or(true)
}

/// Reuse the ready lane for this cwd, resume a gone one, else start a picture
/// lane on the `gpt-image-gen` profile.
fn ensure_lane(
    env: &Env,
    layout: &Layout,
    runner: &dyn Runner,
    name: &str,
    cwd: &Path,
    pictures: &[PathBuf],
) -> Result<Lane> {
    let cwd_text = cwd.display().to_string();
    if let Ok(lane) = Lane::read(layout, name)
        && !lane.stopped
        && lane.cwd == cwd_text
        && lane.profile.as_deref() == Some(home::IMAGE_PROFILE)
    {
        if lane.state == "ready" {
            return Ok(lane);
        }
        if lane.state == "gone" {
            return lane::resume(env, layout, runner, name);
        }
        bail!("picture lane `{name}` is {}, not ready", lane.state);
    }
    lane::start(
        env,
        layout,
        runner,
        &lane::StartOptions {
            name: name.to_string(),
            cwd: Some(cwd_text),
            profile: Some(home::IMAGE_PROFILE.to_string()),
            images: pictures.to_vec(),
        },
    )
}

/// Wait for the PNG Codex writes for this request. A blocked lane is reported
/// instead of waiting the whole timeout.
fn wait_for_image(
    env: &Env,
    runner: &dyn Runner,
    lane: &Lane,
    after: SystemTime,
) -> Result<PathBuf> {
    let dir = env.lane_codex_home().join("generated_images");
    let deadline = Instant::now() + IMAGE_TIMEOUT;
    let mut candidate: Option<(PathBuf, u64)> = None;
    loop {
        if let Some(path) = newest_png(&dir, after) {
            let size = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            if size > 0 {
                match &candidate {
                    Some((seen, seen_size)) if *seen == path && *seen_size == size => {
                        return Ok(path);
                    }
                    _ => candidate = Some((path, size)),
                }
            }
        }
        if lane_blocked(env, runner, lane)? {
            let screen = herdr_cli::pane_read(runner, &env.herdr_bin(), &lane.pane_id)
                .unwrap_or_else(|_| "no readable screen".into());
            bail!("WAITING pro-bridge the picture lane is blocked: {screen}");
        }
        if Instant::now() >= deadline {
            bail!(
                "no picture under {} after {}s; check the lane in pane {}",
                dir.display(),
                IMAGE_TIMEOUT.as_secs(),
                lane.pane_id
            );
        }
        std::thread::sleep(POLL);
    }
}

/// True when herdr reports the lane blocked.
fn lane_blocked(env: &Env, runner: &dyn Runner, lane: &Lane) -> Result<bool> {
    let bin = env.herdr_bin();
    Ok(herdr_cli::agent_find(runner, &bin, &lane.name)?
        .map(|agent| agent.blocked())
        .unwrap_or(false))
}

/// The newest PNG under `dir` written after `after`.
fn newest_png(dir: &Path, after: SystemTime) -> Option<PathBuf> {
    let mut best: Option<(SystemTime, PathBuf)> = None;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("png") {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|meta| meta.modified()) else {
                continue;
            };
            if modified < after {
                continue;
            }
            if best.as_ref().is_none_or(|(stamp, _)| modified > *stamp) {
                best = Some((modified, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

/// Copy the generated PNG to the requested path.
fn save_into(source: &Path, out: &Path) -> Result<()> {
    let dir = out.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).with_context(|| format!("could not create {}", dir.display()))?;
    let mut input = std::fs::File::open(source)
        .with_context(|| format!("could not open {}", source.display()))?;
    let mut output = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(out)
        .with_context(|| format!("refused to replace {}", out.display()))?;
    if let Err(error) = std::io::copy(&mut input, &mut output).and_then(|_| output.sync_all()) {
        drop(output);
        let _ = std::fs::remove_file(out);
        return Err(error)
            .with_context(|| format!("could not save {} to {}", source.display(), out.display()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pro::sh::fake::{FakeRunner, ok};

    #[test]
    fn reference_pictures_refuses_a_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let error = reference_pictures(&[dir.path().join("nope.png")]).unwrap_err();
        assert!(error.to_string().contains("--with"), "{error}");
        std::fs::write(dir.path().join("a-dir"), b"x").unwrap();
        // A file that exists but cannot be read as a picture is still a file
        // for this check; a directory is not.
        let error = reference_pictures(&[dir.path().to_path_buf()]).unwrap_err();
        assert!(error.to_string().contains("--with"), "{error}");
    }

    #[test]
    fn reference_pictures_refuses_more_than_four() {
        let dir = tempfile::tempdir().unwrap();
        let files: Vec<PathBuf> = (0..MAX_WITH + 1)
            .map(|n| dir.path().join(format!("{n}.png")))
            .collect();
        let error = reference_pictures(&files).unwrap_err();
        assert!(error.to_string().contains("at most 4"), "{error}");
    }

    #[test]
    fn the_request_names_the_attached_pictures() {
        let attached = request("a dark terminal", 1536, 1024, true);
        assert!(
            attached
                .contains("attached pictures show the current screen; keep its palette and layout"),
            "{attached}"
        );
        let plain = request("a dark terminal", 1536, 1024, false);
        assert!(!plain.contains("attached pictures"), "{plain}");
    }

    #[test]
    fn run_attaches_with_pictures_and_nests_under_the_caller() {
        let dir = tempfile::tempdir().unwrap();
        let state = dir.path().join("pro");
        let env = Env::for_test(
            dir.path(),
            &[
                ("HERDR_BIN_PATH", "/h/herdr"),
                ("HERDR_PRO_STATE_DIR", state.to_str().unwrap()),
                ("HERDR_PANE_ID", "wC:p1"),
                ("HERDR_WORKSPACE_ID", "wC"),
            ],
        );
        let layout = Layout::for_test(&state);
        layout.ensure().unwrap();
        let image = dir.path().join("ref.png");
        std::fs::write(&image, b"png").unwrap();
        let prompt = dir.path().join("prompt.txt");
        std::fs::write(&prompt, "a dark terminal window").unwrap();
        let out = dir.path().join("out.png");
        let runner = FakeRunner::new();
        runner.on(
            "tab create",
            ok(r#"{"result":{"root_pane":{"pane_id":"wC:p2","tab_id":"wC:t2","workspace_id":"wC","cwd":"/w"}}}"#),
        );
        runner.on(
            "agent start",
            ok(r#"{"result":{"agent":{"pane_id":"wC:p2","name":"gpt-image-gen","agent":"codex","agent_status":"idle"}}}"#),
        );
        runner.on("agent prompt", ok(r#"{"result":{}}"#));
        runner.on(
            "agent list",
            ok(r#"{"result":{"agents":[{"name":"gpt-image-gen","pane_id":"wC:p2","agent":"codex","agent_status":"blocked"}]}}"#),
        );
        runner.on("pane read", ok("waiting on the model\n"));
        runner.on("tab close", ok(r#"{"result":{}}"#));
        let error = run(
            &env,
            &layout,
            &runner,
            &ImageOptions {
                prompt_file: prompt,
                size: "1536x1024".into(),
                out,
                with: vec![image.clone()],
                keep: false,
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains("blocked"), "{error}");
        let calls = runner.calls.borrow();
        let start = calls
            .iter()
            .map(|cmd| cmd.display())
            .find(|line| line.contains("agent start"))
            .unwrap();
        assert!(
            start.contains(&format!("--image {}", image.display())),
            "{start}"
        );
        assert!(start.contains("--parent wC:p1"), "{start}");
        let sent = calls
            .iter()
            .map(|cmd| cmd.display())
            .find(|line| line.contains("agent prompt"))
            .unwrap();
        assert!(
            sent.contains("attached pictures show the current screen"),
            "{sent}"
        );
    }
}
