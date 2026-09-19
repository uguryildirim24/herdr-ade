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

/// A picture request: `herdr-pro image`.
#[derive(Debug, Clone)]
pub struct ImageOptions {
    pub prompt_file: PathBuf,
    pub size: String,
    pub out: PathBuf,
    pub keep: bool,
}

/// `herdr-pro image`: one lane, one request, one PNG.
pub fn run(
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

    let name = lane_name(layout, &cwd)?;
    let lane = ensure_lane(env, layout, runner, &name, &cwd)?;

    let request = request(&prompt, width, height);
    let started = SystemTime::now();
    herdr_cli::agent_prompt(runner, &env.herdr_bin(), &lane.name, &request)
        .context("could not send the picture request")?;
    let saved = wait_for_image(env, runner, &lane, started)?;
    save_into(&saved, &out)?;
    if !opts.keep {
        lane::stop(env, layout, runner, &lane.name)?;
    }
    Ok(out)
}

/// The one-line request typed into the lane.
fn request(prompt: &str, width: u32, height: u32) -> String {
    format!(
        "Make one picture. Prompt: {}. Size {width}x{height}. Call the image tool exactly once and reply with one line naming the picture.",
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
fn lane_name(layout: &Layout, cwd: &Path) -> Result<String> {
    let base = home::IMAGE_PROFILE;
    let cwd_text = cwd.display().to_string();
    for lane in Lane::list(layout)? {
        if lane.stopped || lane.state == "gone" {
            continue;
        }
        if lane.cwd == cwd_text && (lane.name == base || lane.name.starts_with(&format!("{base}-")))
        {
            return Ok(lane.name);
        }
    }
    if !state::name_taken(layout, base) {
        return Ok(base.to_string());
    }
    for n in 2..1000u32 {
        let candidate = format!("{base}-{n}");
        if !state::name_taken(layout, &candidate) {
            return Ok(candidate);
        }
    }
    bail!("no free picture lane name near `{base}`")
}

/// Reuse the ready lane for this cwd, resume a gone one, else start a picture
/// lane on the `gpt-image-gen` profile.
fn ensure_lane(
    env: &Env,
    layout: &Layout,
    runner: &dyn Runner,
    name: &str,
    cwd: &Path,
) -> Result<Lane> {
    let cwd_text = cwd.display().to_string();
    if let Ok(lane) = Lane::read(layout, name)
        && !lane.stopped
        && lane.cwd == cwd_text
    {
        if lane.state == "ready" {
            return Ok(lane);
        }
        if lane.state == "gone" {
            return lane::resume(env, layout, runner, name);
        }
    }
    lane::start(
        env,
        layout,
        runner,
        &lane::StartOptions {
            name: name.to_string(),
            parent: None,
            cwd: Some(cwd_text),
            profile: Some(home::IMAGE_PROFILE.to_string()),
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
    std::fs::copy(source, out)
        .with_context(|| format!("could not save {} to {}", source.display(), out.display()))?;
    Ok(())
}
