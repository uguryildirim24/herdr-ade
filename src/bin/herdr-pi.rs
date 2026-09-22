//! `herdr-pi`: setup, login instructions, doctor and check (SPEC-pi v2 §3).
//!
//! Thin on purpose: every step is a function of the pi library, which is also
//! linked into `herdr-ade`. It never runs a login itself: `login` prints the
//! one-time instructions and opens the shared pi folder in the terminal; Rolf
//! types `/login` there.
//!
//! Compiles `src/pi/` through a path attribute because the package has no
//! library target by design (SPEC-pi v2 §3: one library crate, two binaries).

#[allow(dead_code)]
#[path = "../pi/mod.rs"]
mod pi;

use std::io::IsTerminal;
use std::process::ExitCode;

use anyhow::Result;
use clap::{Parser, Subcommand};

const VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "+", env!("HP_BUILD_ID"));

#[derive(Parser)]
#[command(name = "herdr-pi", version = VERSION, about = "Pinned pi for the ADE harness")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Install the pinned pi, write the shared folder and the guard
    Setup,
    /// Print the one-time login steps; open pi on the shared folder
    Login {
        /// One provider: openai-codex, opencode-go, kimi-coding
        provider: Option<String>,
    },
    /// Check the setup; exit 1 when any check fails
    Doctor,
    /// Read-only readiness for one provider (JSON); exit 1 when not ready
    Check {
        /// The pi provider id.
        provider: String,
        /// The exact routed model to probe.
        #[arg(long)]
        model: Option<String>,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("herdr-pi: {error:#}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: &Cli) -> Result<bool> {
    let env = pi::Env::from_process()?;
    let layout = pi::Layout::from_env(&env)?;
    let runner = pi::sh::RealRunner;
    match &cli.command {
        Command::Setup => {
            let report = pi::install::setup(&runner, &env, &layout)?;
            for step in &report.steps {
                println!("{step}");
            }
            println!();
            println!("The pi folder is {}", report.pi_folder.display());
            println!("Type this one line to put the wrapper on your login PATH:");
            println!();
            println!("{}", report.link_line);
            println!();
            println!("Then run `herdr-pi doctor` and `herdr-pi login`.");
            Ok(true)
        }
        Command::Login { provider } => login(&layout, provider.as_deref()),
        Command::Doctor => {
            let (rows, healthy) = pi::doctor::doctor_rows()?;
            for row in &rows {
                println!("{}", row.line());
            }
            if healthy {
                println!("herdr-pi: all checks passed");
            } else {
                println!("herdr-pi: some checks failed");
            }
            Ok(healthy)
        }
        Command::Check { provider, model } => {
            let report =
                pi::doctor::check_report_model(&env, &layout, &runner, provider, model.as_deref());
            println!("{}", serde_json::to_string_pretty(&report.json())?);
            Ok(report.ok)
        }
    }
}

/// The one-time login per provider (SPEC-pi v2 §2). Never types `/login`,
/// never reads or writes `auth.json`.
fn login(layout: &pi::Layout, provider: Option<&str>) -> Result<bool> {
    if let Some(provider) = provider
        && !pi::launch::PROVIDERS.contains(&provider)
    {
        anyhow::bail!(
            "unknown provider `{provider}`; expected one of {}",
            pi::launch::PROVIDERS.join(", ")
        );
    }
    for (id, name, steps) in pi::login_instructions() {
        if let Some(provider) = provider
            && provider != id
        {
            continue;
        }
        println!("{name} ({id}):");
        println!("  {steps}");
        println!();
    }
    if !layout.wrapper().is_file() {
        println!(
            "{}",
            pi::doctor::Row::fail("wrapper", "missing; run `herdr-pi setup` first").line()
        );
        return Ok(false);
    }
    println!("Type `/login` inside pi, pick the provider, and finish there.");
    println!("The plugin never sees your key and never copies another tool's login.");
    println!();
    if !std::io::stdin().is_terminal() {
        println!("No terminal here, so pi was not opened. Run `herdr-pi login` from a pane.");
        return Ok(true);
    }
    let status = std::process::Command::new(layout.wrapper())
        .env("PI_CODING_AGENT_DIR", layout.agent())
        .status()?;
    Ok(status.success())
}
