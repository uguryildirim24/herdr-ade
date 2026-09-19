//! `herdr-pro`: the Pro bridge plugin (SPEC-pro-bridge v2).
//!
//! Thin on purpose: every step is a function of the `pro` library, which also
//! compiles into `herdr-ade` (so the plugin and the binary share one source).
//! It never starts, restarts or stops the bridge, never writes `~/.codex`, and
//! never drives a login.

#[allow(dead_code)]
#[path = "../pro/mod.rs"]
mod pro;

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};

use pro::sh::RealRunner;
use pro::{Env, Layout, bridge, doctor, home, image, lane, serve, state, turn};

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("HP_BUILD_ID"));

#[derive(Parser)]
#[command(name = "herdr-pro", version = VERSION, about = "Pro as an ordinary Codex lane on the codex-chatgpt-web bridge")]
struct Cli {
    /// State directory (default: $HERDR_PRO_STATE_DIR, then <ADE root>/pro-bridge)
    #[arg(long, global = true, value_name = "DIR")]
    state_dir: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Link `~/.local/bin/herdr-pro` and create the state directories
    Init,
    /// Check the bridge, the Codex route, the login and the breaker
    Doctor {
        /// Print the checks as JSON
        #[arg(long)]
        json: bool,
    },
    /// Print the bridge and Codex login steps; never drives a login
    Login,
    /// Start the local relay that makes Pro a plain pi lane
    Serve,
    /// The foreground relay (started detached by `serve`)
    #[command(hide = true)]
    ServeRun,
    /// Stop the relay and remove its `serve.json`
    StopServe,
    /// Write the `pro` provider into the shared pi folder's `models.json`
    PiProvider,
    /// Start a Pro lane: a Codex agent pointed at the bridge
    Start {
        /// Lane name (the herdr agent name)
        #[arg(long)]
        name: String,
        /// A trusted directory; default: the current directory
        #[arg(long)]
        cwd: Option<String>,
        /// The lane recipe's ready timeout in ms; default: 180000
        #[arg(long, value_name = "MS")]
        ready_timeout_ms: Option<u64>,
    },
    /// Run one Pro turn: packet in, detached collector out
    Turn {
        /// Lane name
        name: String,
        /// The brief file
        #[arg(long, value_name = "FILE")]
        brief: PathBuf,
        /// Where the answer is written (must not exist yet)
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// The coordinator agent to notify
        #[arg(long)]
        notify: String,
        /// A file to attach; repeatable
        #[arg(long = "attach", value_name = "FILE")]
        attach: Vec<PathBuf>,
        /// The turn id (default: <lane>-<NN>)
        #[arg(long)]
        id: Option<String>,
    },
    /// The detached worker (started by `turn`)
    #[command(hide = true)]
    Collector {
        #[arg(long)]
        turn: String,
    },
    /// Start a gone lane again and resume its Codex thread
    Resume { name: String },
    /// Mark lanes whose pane no longer runs Codex as gone; print resume lines
    Reconcile,
    /// Make one picture with Codex's image tool on the `gpt-image-gen` profile
    Image {
        /// A file holding the prompt
        #[arg(long, value_name = "FILE")]
        prompt_file: PathBuf,
        /// The picture size, `WxH`
        #[arg(long, value_name = "WxH")]
        size: String,
        /// Where the PNG is saved (must not exist yet)
        #[arg(long, value_name = "FILE")]
        out: PathBuf,
        /// A reference picture the model sees before it draws; repeatable, up to four
        #[arg(long = "with", value_name = "FILE")]
        with: Vec<PathBuf>,
        /// Keep the picture lane running for the next call
        #[arg(long)]
        keep: bool,
    },
    /// The stop switch: stop one lane so a restart never brings it back
    Stop { name: String },
    /// Clear the breaker and resume the bridge
    ResumeBridge,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    if let Some(dir) = &cli.state_dir {
        // SAFETY: first thing in `main`, before any thread exists.
        unsafe { std::env::set_var("HERDR_PRO_STATE_DIR", dir) };
    }
    match run(&cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("herdr-pro: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<bool> {
    let env = Env::from_process()?;
    let layout = Layout::from_env(&env)?;
    let runner = RealRunner;
    match &cli.command {
        Command::Init => init(&env, &layout),
        Command::Serve => {
            serve::start(&layout)?;
            Ok(true)
        }
        Command::ServeRun => {
            serve::run(&layout, &env)?;
            Ok(true)
        }
        Command::StopServe => {
            serve::stop(&layout)?;
            Ok(true)
        }
        Command::PiProvider => {
            let state = serve::ServeState::read(&layout)
                .context("no serve.json; run `herdr-pro serve` first")?;
            serve::write_provider(&layout, state.port, &state.token)?;
            println!(
                "wrote the `pro` provider (http://127.0.0.1:{}/v1) into {}",
                state.port,
                layout.pi_models().display()
            );
            Ok(true)
        }
        Command::Doctor { json } => {
            let rows = doctor::doctor_rows(&env, &layout, &runner);
            if *json {
                println!("{}", serde_json::to_string_pretty(&doctor::json(&rows))?);
            } else {
                for row in &rows {
                    println!("{}", row.line());
                }
            }
            Ok(doctor::healthy(&rows))
        }
        Command::Login => {
            login(&layout);
            Ok(true)
        }
        Command::Start {
            name,
            cwd,
            ready_timeout_ms,
        } => {
            let lane = lane::start(
                &env,
                &layout,
                &runner,
                &lane::StartOptions {
                    name: name.clone(),
                    cwd: cwd.clone(),
                    profile: None,
                    images: Vec::new(),
                    ready_timeout_ms: *ready_timeout_ms,
                },
            )?;
            println!(
                "{} is ready in pane {} (rollout {})",
                lane.name,
                lane.pane_id,
                lane.rollout.as_deref().unwrap_or("not resolved yet")
            );
            Ok(true)
        }
        Command::Turn {
            name,
            brief,
            out,
            notify,
            attach,
            id,
        } => {
            let turn = turn::start(
                &env,
                &layout,
                &runner,
                &turn::TurnOptions {
                    lane: name.clone(),
                    brief: brief.clone(),
                    out: out.clone(),
                    notify: notify.clone(),
                    attachments: attach.clone(),
                    id: id.clone(),
                },
            )?;
            println!(
                "turn {} started; the answer lands at {} and DONE goes to {}",
                turn.tag,
                out.display(),
                notify
            );
            Ok(true)
        }
        Command::Collector { turn } => {
            turn::collect(&env, &layout, &runner, turn)?;
            Ok(true)
        }
        Command::Resume { name } => {
            let lane = lane::resume(&env, &layout, &runner, name)?;
            println!("{} resumed in pane {}", lane.name, lane.pane_id);
            Ok(true)
        }
        Command::Reconcile => {
            for line in lane::reconcile(&env, &layout, &runner)? {
                println!("{line}");
            }
            Ok(true)
        }
        Command::Image {
            prompt_file,
            size,
            out,
            with,
            keep,
        } => {
            let path = image::run(
                &env,
                &layout,
                &runner,
                &image::ImageOptions {
                    prompt_file: prompt_file.clone(),
                    size: size.clone(),
                    out: out.clone(),
                    with: with.clone(),
                    keep: *keep,
                },
            )?;
            println!("{}", path.display());
            Ok(true)
        }
        Command::Stop { name } => {
            let lane = lane::stop(&env, &layout, &runner, name)?;
            println!("{} stopped; reconcile will not bring it back", lane.name);
            Ok(true)
        }
        Command::ResumeBridge => {
            let (port, _) = bridge::health_any(&runner)?;
            let body = bridge::resume(&runner, &env, port)?;
            state::clear_cooldown(&layout)?;
            println!("bridge resumed on port {port}: {body}");
            Ok(true)
        }
    }
}

/// `init`: the `~/.local/bin/herdr-pro` link and the v2 shared Pro home.
fn init(env: &Env, layout: &Layout) -> Result<bool> {
    layout.ensure()?;
    home::init(layout)?;
    // `current_exe` can be the invoking symlink (`~/.local/bin/herdr-pro`),
    // so canonicalize before linking or the link points at itself.
    let exe = std::env::current_exe().context("could not find this binary's own path")?;
    let exe = std::fs::canonicalize(&exe)
        .with_context(|| format!("could not resolve {}", exe.display()))?;
    let bin_dir = env.home.join(".local/bin");
    std::fs::create_dir_all(&bin_dir)
        .with_context(|| format!("could not create {}", bin_dir.display()))?;
    let link = bin_dir.join("herdr-pro");
    if link == exe {
        println!("{} is already this binary", link.display());
    } else {
        if let Ok(meta) = link.symlink_metadata() {
            if meta.file_type().is_symlink() {
                std::fs::remove_file(&link)
                    .with_context(|| format!("could not replace {}", link.display()))?;
            } else {
                bail!(
                    "{} exists and is not a symlink; move it aside by hand",
                    link.display()
                );
            }
        }
        std::os::unix::fs::symlink(&exe, &link)
            .with_context(|| format!("could not link {}", link.display()))?;
        println!("linked {} -> {}", link.display(), exe.display());
    }
    println!("state lives in {}", layout.root.display());
    println!("Pro home: {}", layout.codex_home().display());
    println!("next: `herdr-pro login`, then `herdr-pro doctor`");
    Ok(true)
}

/// The two logins, printed, never driven (spec §4, "Never automated").
fn login(layout: &Layout) {
    println!("Two logins are needed for a Pro lane. The plugin only prints them.");
    println!();
    println!("1. The bridge's own ChatGPT login (the desktop app):");
    println!("   Install and open Codex Web GPT, sign in, accept the unofficial-use notice,");
    println!("   and choose browser-only. Then run `codex-chatgpt-web route disconnect` so");
    println!("   your daily Codex does not go through the bridge.");
    println!();
    println!("2. Codex's own ChatGPT login in the Pro home:");
    println!(
        "   Run `CODEX_HOME={} codex login` and finish in the browser or with the device code.",
        layout.codex_home().display()
    );
    println!("   The plugin never copies auth.json from ~/.codex.");
    println!();
    println!("Then `herdr-pro doctor` must be green before `herdr-pro start`.");
}
