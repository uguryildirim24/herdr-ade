//! The spec dialogue between two lanes (SPEC-ADE D7): a drafter thread on
//! `lane/spec-<topic>` and a critic, which is a `critic` thread or the Pro
//! pane adopted passively. `turn` pins `{ topic, n, expected_path }` before
//! it types the one-line `TURN` prompt; `commit` verifies the expected new
//! file, records its hash, commits it through D9 under the repository lock,
//! and only then advances the turn. A delayed turn-N line never completes
//! turn N+1: the commit names `n`.
//!
//! The pair filter is the hook point for lane A4 (the picker): `start` asks
//! it whether the drafter and critic roles may work as a pair.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

use crate::herdr::Herdr;
use crate::paths::Ctx;
use crate::project::{self, Project, write_atomic};
use crate::round::Git;
use crate::round::repo::{commit_files_on_branch, repo_lock};
use crate::thread::sha256_hex;

/// Lane A4 plugs its picker in here. `Err` carries the refusal to print.
pub trait PairFilter {
    fn check(&self, drafter: &str, critic: &str) -> std::result::Result<(), String>;
}

/// The default: any two roles may pair, except a role with itself.
pub struct AnyPair;

impl PairFilter for AnyPair {
    fn check(&self, drafter: &str, critic: &str) -> std::result::Result<(), String> {
        if drafter == critic {
            return Err(format!(
                "dialogue_pair: the drafter and the critic are both `{drafter}`"
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Turn {
    pub topic: String,
    pub n: u32,
    pub expected_path: String,
    pub sent: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TurnRecord {
    pub n: u32,
    pub path: String,
    pub hash: String,
    pub commit: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Dialogue {
    pub topic: String,
    pub plain: String,
    pub drafter: String,
    pub critic: String,
    pub branch: String,
    pub repo: String,
    /// The integration branch turn files are committed on (D9).
    pub integration: String,
    pub started: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub critic_pane: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<Turn>,
    #[serde(default)]
    pub turns: Vec<TurnRecord>,
}

fn dir(project: &Project) -> PathBuf {
    project.dir().join("dialogues")
}

fn path(project: &Project, topic: &str) -> PathBuf {
    dir(project).join(format!("{topic}.toml"))
}

pub fn validate_topic(topic: &str) -> Result<()> {
    if topic.is_empty()
        || topic.len() > 40
        || !topic
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        bail!("`{topic}` is not a topic (lowercase letters, digits and dashes)");
    }
    Ok(())
}

pub fn load(project: &Project, topic: &str) -> Result<Dialogue> {
    validate_topic(topic)?;
    let p = path(project, topic);
    let text = std::fs::read_to_string(&p).with_context(|| format!("no dialogue `{topic}`"))?;
    toml::from_str(&text).with_context(|| format!("{} does not parse", p.display()))
}

fn save(project: &Project, d: &Dialogue) -> Result<()> {
    std::fs::create_dir_all(dir(project))?;
    write_atomic(&path(project, &d.topic), toml::to_string(d)?.as_bytes())
}

pub fn list(project: &Project) -> Vec<Dialogue> {
    let Ok(entries) = std::fs::read_dir(dir(project)) else {
        return Vec::new();
    };
    let mut out: Vec<Dialogue> = entries
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter_map(|n| n.strip_suffix(".toml").map(str::to_string))
        .filter_map(|t| load(project, &t).ok())
        .collect();
    out.sort_by(|a, b| a.started.cmp(&b.started));
    out
}

pub struct StartArgs {
    pub topic: String,
    pub drafter: String,
    pub critic: String,
    pub plain: Option<String>,
    pub repo: Option<String>,
    pub integration: Option<String>,
}

/// `ha dialogue start`. Records the dialogue and prints the lines that start
/// its two sides; the drafter thread is started by `thread start` (A1) and
/// Pro is adopted passively (A1's `thread adopt --role pro --passive`).
pub fn start(
    ctx: &Ctx,
    slug: &str,
    args: StartArgs,
    filter: &dyn PairFilter,
) -> Result<(Dialogue, String)> {
    let project = Project::load(&ctx.root, slug)?;
    validate_topic(&args.topic)?;
    let Some(plain) = args.plain.filter(|p| !p.trim().is_empty()) else {
        bail!(
            "plain_missing: `dialogue start` needs --plain \"<one sentence that says what the spec is about>\""
        );
    };
    crate::glossary::check_birth(&project, &plain)?;
    filter
        .check(&args.drafter, &args.critic)
        .map_err(|e| anyhow::anyhow!(e))?;
    let repo = match args.repo {
        Some(r) => r,
        None => {
            let (settings, _) = project.read_project_md()?;
            settings
                .repos
                .iter()
                .find(|r| r.machine.is_none())
                .map(|r| r.path.clone())
                .context("dialogue_needs_repo: pass --repo")?
        }
    };
    let git = Git::new(ctx.runner, &repo);
    let integration = match args.integration {
        Some(b) => b,
        None => git.run(&["symbolic-ref", "--short", "HEAD"])?,
    };
    let d = Dialogue {
        topic: args.topic.clone(),
        plain: plain.trim().to_string(),
        drafter: args.drafter.clone(),
        critic: args.critic.clone(),
        branch: format!("lane/spec-{}", args.topic),
        repo: repo.clone(),
        integration,
        started: project::now(),
        ..Default::default()
    };
    {
        let _lock = project.lock()?;
        if path(&project, &d.topic).exists() {
            bail!("dialogue_exists: `{}` is already recorded", d.topic);
        }
        save(&project, &d)?;
    }
    let _ = crate::glossary::rewrite(&project);
    let prefix =
        crate::coordinator::current_prefix(&ctx.root).unwrap_or_else(|_| "herdr-ade".into());
    let coord = project.coordinator().map(|c| c.pane_id).unwrap_or_default();
    let mut next = format!(
        "drafter: {prefix} thread start {slug} --role {} --branch {} --plain \"{}\" --task-file <brief>\n",
        d.drafter, d.branch, d.plain
    );
    if d.critic == "pro" {
        next.push_str(&format!(
            "critic:  pro-mcp start --name pro --parent {coord}\n         {prefix} thread adopt {slug} --pane <pro pane> --role pro --passive\n         {prefix} dialogue critic {slug} {} --pane <pro pane>\n",
            d.topic
        ));
    } else {
        next.push_str(&format!(
            "critic:  {prefix} thread start {slug} --role {} --plain \"<sentence>\" --task-file <brief>, then\n         {prefix} dialogue critic {slug} {} --pane <its pane>\n",
            d.critic, d.topic
        ));
    }
    Ok((d, next))
}

/// Records the critic's pane after validating the recipient: for Pro the
/// agent in that pane must be kind `chatgpt` and named `pro`.
pub fn bind_critic(ctx: &Ctx, slug: &str, topic: &str, pane: &str) -> Result<Dialogue> {
    let project = Project::load(&ctx.root, slug)?;
    let mut d = load(&project, topic)?;
    let coord = project
        .coordinator()
        .context("the project has not been opened")?;
    let h = Herdr::new(ctx.env.herdr_bin(), &coord.socket, ctx.runner);
    let listed = h
        .call(&["agent", "list"], Duration::from_secs(10))
        .map_err(|e| anyhow::anyhow!("herdr agent list: {}", e.message))?;
    let agent = listed["agents"]
        .as_array()
        .and_then(|a| {
            a.iter()
                .find(|x| x["pane_id"].as_str() == Some(pane))
                .cloned()
        })
        .with_context(|| format!("critic_not_found: no agent in pane {pane}"))?;
    if d.critic == "pro" {
        let kind = agent["agent"].as_str().unwrap_or("");
        let name = agent["name"].as_str().unwrap_or("");
        if kind != "chatgpt" || name != "pro" {
            bail!(
                "critic_mismatch: pane {pane} holds `{name}` of kind `{kind}`, expected `pro` of kind `chatgpt`"
            );
        }
    }
    let _lock = project.lock()?;
    d.critic_pane = Some(pane.to_string());
    save(&project, &d)?;
    Ok(d)
}

pub fn turn_path(d: &Dialogue, n: u32) -> String {
    format!("tasks/{}/turns/{n:02}-{}.md", d.topic, d.critic)
}

/// The one-line `TURN` prompt; its `DONE` reply is an artifact event.
pub fn turn_line(d: &Dialogue, n: u32) -> String {
    let path = turn_path(d, n);
    format!(
        "TURN {topic}-{n:02}: write your turn to {repo}/{path}, then reply DONE {topic}-{n:02} {path} -",
        topic = d.topic,
        repo = d.repo.trim_end_matches('/'),
    )
}

/// `ha dialogue turn <topic>`: pin the turn first, then type the line.
pub fn turn(ctx: &Ctx, slug: &str, topic: &str, resend: bool) -> Result<Turn> {
    let project = Project::load(&ctx.root, slug)?;
    let (d, t) = {
        let _lock = project.lock()?;
        let mut d = load(&project, topic)?;
        let pane = d
            .critic_pane
            .clone()
            .context("critic_unbound: run `dialogue critic` with the critic's pane first")?;
        let _ = pane;
        let t = match (&d.turn, resend) {
            (Some(t), true) => t.clone(),
            (Some(t), false) => bail!(
                "turn_outstanding: turn {} is sent and not committed; commit it or pass --resend",
                t.n
            ),
            (None, _) => {
                let n = d.turns.last().map_or(1, |r| r.n + 1);
                Turn {
                    topic: d.topic.clone(),
                    n,
                    expected_path: turn_path(&d, n),
                    sent: project::now(),
                }
            }
        };
        d.turn = Some(t.clone());
        save(&project, &d)?;
        (d, t)
    };
    let coord = project
        .coordinator()
        .context("the project has not been opened")?;
    let h = Herdr::new(ctx.env.herdr_bin(), &coord.socket, ctx.runner);
    let pane = d.critic_pane.clone().unwrap_or_default();
    h.agent_prompt(&pane, &turn_line(&d, t.n)).map_err(|e| {
        anyhow::anyhow!(
            "the TURN line was not sent ({}); the turn stays pinned, retry with --resend",
            e.message
        )
    })?;
    Ok(t)
}

/// `ha dialogue commit <topic> <nn>`.
pub fn commit(ctx: &Ctx, slug: &str, topic: &str, n: u32) -> Result<TurnRecord> {
    let project = Project::load(&ctx.root, slug)?;
    let d = load(&project, topic)?;
    let t = d
        .turn
        .clone()
        .context("turn_none: no turn is outstanding for this dialogue")?;
    if t.n != n {
        bail!(
            "turn_mismatch: the outstanding turn is {}, not {n}; a delayed line never completes a later turn",
            t.n
        );
    }
    let file = PathBuf::from(&d.repo).join(&t.expected_path);
    let bytes = std::fs::read(&file)
        .map_err(|e| anyhow::anyhow!("turn_file_missing: {} ({e})", file.display()))?;
    if bytes.iter().all(u8::is_ascii_whitespace) {
        bail!("turn_file_empty: {} is empty", file.display());
    }
    let text = String::from_utf8(bytes.clone()).context("turn_file_not_text")?;
    let hash = sha256_hex(&bytes);
    let git = Git::new(ctx.runner, &d.repo);
    let sha = {
        let _repo = repo_lock(&git)?;
        let head = git
            .branch_head(&d.integration)?
            .with_context(|| format!("branch_missing: `{}`", d.integration))?;
        if git.show_file(&head, &t.expected_path)?.is_some() {
            bail!(
                "turn_file_exists: {} is already committed on `{}`",
                t.expected_path,
                d.integration
            );
        }
        commit_files_on_branch(
            &git,
            &d.integration,
            &[(t.expected_path.as_str(), text.as_str())],
            &format!("spec({topic}): turn {n:02} from {}", d.critic),
            &head,
            &project.state_dir().join("tmp"),
        )?
    };
    let record = TurnRecord {
        n,
        path: t.expected_path.clone(),
        hash,
        commit: sha,
    };
    {
        let _lock = project.lock()?;
        let mut d = load(&project, topic)?;
        if d.turn.as_ref().map(|x| x.n) == Some(n) {
            d.turn = None;
        }
        d.turns.push(record.clone());
        save(&project, &d)?;
    }
    Ok(record)
}
