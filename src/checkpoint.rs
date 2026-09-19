//! `ha checkpoint` and `ha pickup` (SPEC-ADE D9): the port of the save-state
//! skill's `state.py` `snapshot`, `check` and `restore`.
//!
//! - `snapshot` (here `compose`): the generated `## Herdr` section of
//!   `HANDOFF.md` plus the machine-readable `HANDOFF.json`, read-only against
//!   herdr and git.
//! - `check`: every pane id, tab id, agent name, branch, repo path and
//!   session id a handoff mentions must exist now; required sections present;
//!   `## Next` is one action.
//! - `pickup` (not `resume`, which already means unpause): re-applies parents
//!   through the pane metadata path and prints start lines for gone workers
//!   from the launch recipe on their records. It never starts or prompts.
//!
//! `ha checkpoint` writes both HANDOFF files as one commit `H` through D9's
//! commit mechanics under the repository lock.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::contracts::RoundRecord;
use crate::herdr::Herdr;
use crate::paths::Ctx;
use crate::project::Project;
use crate::round::Git;
use crate::round::repo::{commit_files_on_branch, repo_lock};
use crate::thread::{self, sha256_hex};

const CALL: Duration = Duration::from_secs(15);

/// The hash the checkpoint intent binds: both files' exact bytes (item 34).
pub fn payload_hash(md: &str, json: &str) -> String {
    let mut bytes = b"HANDOFF.md\0".to_vec();
    bytes.extend_from_slice(md.as_bytes());
    bytes.extend_from_slice(b"\0HANDOFF.json\0");
    bytes.extend_from_slice(json.as_bytes());
    sha256_hex(&bytes)
}

fn by_key(items: &[Value], key: &str) -> BTreeMap<String, Value> {
    items
        .iter()
        .filter_map(|i| Some((i.get(key)?.as_str()?.to_string(), i.clone())))
        .collect()
}

fn s(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

fn session_value(v: &Value) -> Option<String> {
    v.get("agent_session")
        .and_then(|s| s.get("value"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Agent binary names per kind, to find an agent's own argv in its pane's
/// foreground group (state.py `KIND_BINARIES`).
fn kind_binaries(kind: &str) -> Vec<&str> {
    match kind {
        "cursor" => vec!["cursor-agent", "cursor"],
        "qodercli" => vec!["qodercli", "qoder"],
        other => vec![other],
    }
}

const INTERPRETERS: [&str; 8] = [
    "node", "python3", "python", "bash", "zsh", "sh", "bun", "deno",
];

fn clean_args(argv: &[String]) -> Vec<String> {
    argv.iter()
        .filter(|a| {
            *a != "--use-system-ca"
                && !(a.starts_with('/')
                    && (a.ends_with(".js") || a.ends_with(".py") || a.ends_with(".mjs")))
        })
        .cloned()
        .collect()
}

/// The flags the agent in `pane` was started with, from `pane process-info`.
fn start_args(h: &Herdr, pane: &str, kind: &str) -> Vec<String> {
    let Ok(info) = h.call(&["pane", "process-info", "--pane", pane], CALL) else {
        return Vec::new();
    };
    let procs = info
        .get("process_info")
        .and_then(|p| p.get("foreground_processes"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let argv_of = |p: &Value| -> Vec<String> {
        p.get("argv")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default()
    };
    let base = |a: &str| {
        Path::new(a)
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_default()
    };
    let names = kind_binaries(kind);
    for p in &procs {
        let argv = argv_of(p);
        if let Some(first) = argv.first()
            && names.contains(&base(first).as_str())
        {
            return clean_args(&argv[1..]);
        }
    }
    let mut sorted = procs.clone();
    sorted.sort_by_key(|p| p.get("pid").and_then(Value::as_u64).unwrap_or(0));
    for p in &sorted {
        let argv = argv_of(p);
        let Some(first) = argv.first() else { continue };
        if INTERPRETERS.contains(&base(first).as_str()) {
            return clean_args(argv.get(2..).unwrap_or(&[]));
        }
        return clean_args(&argv[1..]);
    }
    Vec::new()
}

// --------------------------------------------------------------------- git

fn git_state(git: &Git) -> Option<Value> {
    let repo = git.repo.clone();
    git.run(&["rev-parse", "--git-dir"]).ok()?;
    let common = git.common_dir().ok()?;
    let main_repo = if common.file_name().is_some_and(|n| n == ".git") {
        common
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or(repo.clone())
    } else {
        PathBuf::from(
            git.run(&["rev-parse", "--show-toplevel"])
                .unwrap_or_default(),
        )
    };
    let main = Git::new(git.runner, &main_repo);
    let listing = main
        .run(&["worktree", "list", "--porcelain"])
        .unwrap_or_default();
    let mut worktrees = Vec::new();
    let mut block: BTreeMap<String, String> = BTreeMap::new();
    for line in listing.lines().chain(std::iter::once("")) {
        if line.is_empty() {
            if !block.is_empty() {
                let path = block.get("worktree").cloned().unwrap_or_default();
                let branch = block
                    .get("branch")
                    .map(|b| b.trim_start_matches("refs/heads/").to_string())
                    .filter(|b| !b.is_empty())
                    .unwrap_or_else(|| "(detached)".into());
                let dir = PathBuf::from(&path);
                let dirty = main.dirty_paths(&dir).map(|d| d.len()).unwrap_or(0);
                worktrees.push(json!({
                    "path": path,
                    "branch": branch,
                    "head": main.run_in(&dir, &["rev-parse", "--short", "HEAD"]).unwrap_or_default(),
                    "subject": main.run_in(&dir, &["log", "-1", "--format=%s"]).unwrap_or_default(),
                    "dirty": dirty,
                }));
            }
            block.clear();
            continue;
        }
        let (k, v) = line.split_once(' ').unwrap_or((line, ""));
        block.insert(k.to_string(), v.to_string());
    }
    let integration = worktrees
        .first()
        .map(|w| s(w, "branch"))
        .unwrap_or_else(|| {
            main.run(&["rev-parse", "--abbrev-ref", "HEAD"])
                .unwrap_or_default()
        });
    let remote = main.run(&["remote"]).unwrap_or_default();
    let ahead_behind = if remote.is_empty() {
        "no remote".to_string()
    } else {
        match main.run(&[
            "rev-parse",
            "--abbrev-ref",
            &format!("{integration}@{{upstream}}"),
        ]) {
            Ok(up) if !up.is_empty() => main
                .run(&[
                    "rev-list",
                    "--left-right",
                    "--count",
                    &format!("{integration}...{up}"),
                ])
                .ok()
                .and_then(|ab| {
                    let mut it = ab.split_whitespace();
                    Some(format!("{} ahead, {} behind {up}", it.next()?, it.next()?))
                })
                .unwrap_or_default(),
            _ => "no upstream set".to_string(),
        }
    };
    let branches: Vec<String> = main
        .run(&["branch", "--format=%(refname:short)"])
        .unwrap_or_default()
        .lines()
        .map(|b| {
            b.trim_matches(|c| c == '*' || c == '+' || c == ' ')
                .to_string()
        })
        .collect();
    let log: Vec<String> = main
        .run(&["log", "--oneline", "-6", &integration])
        .unwrap_or_default()
        .lines()
        .map(str::to_string)
        .collect();
    Some(json!({
        "repo": main_repo.to_string_lossy(),
        "integration_branch": integration,
        "integration_log": log,
        "remote": ahead_behind,
        "worktrees": worktrees,
        "branches": branches,
    }))
}

fn record_files(repo: &Path) -> Value {
    let pats: [(&str, &str, &str); 4] = [
        ("briefs", "tasks", ".md"),
        ("verdicts", "tasks/reviews", ".md"),
        ("reports", ".reports", ".md"),
        ("handoff", "", "HANDOFF.md"),
    ];
    let mut found = serde_json::Map::new();
    for (k, dir, suffix) in pats {
        let mut files: Vec<(std::time::SystemTime, String)> = Vec::new();
        if k == "handoff" {
            if repo.join("HANDOFF.md").is_file() {
                files.push((std::time::SystemTime::UNIX_EPOCH, "HANDOFF.md".into()));
            }
        } else if let Ok(entries) = std::fs::read_dir(repo.join(dir)) {
            for e in entries.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if name.ends_with(suffix) && e.path().is_file() {
                    let t = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
                    files.push((t, format!("{dir}/{name}")));
                }
            }
        }
        files.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        found.insert(
            k.to_string(),
            Value::Array(
                files
                    .into_iter()
                    .take(12)
                    .map(|(_, f)| Value::String(f))
                    .collect(),
            ),
        );
    }
    Value::Object(found)
}

// ----------------------------------------------------------------- collect

pub struct Where<'a> {
    pub herdr: &'a Herdr<'a>,
    pub pane: String,
    pub session: String,
    pub repo: Option<PathBuf>,
}

/// `state.py collect`: the live state around a coordinator pane.
pub fn collect(ctx: &Ctx, at: &Where) -> Result<Value> {
    let snap = at
        .herdr
        .call(&["api", "snapshot"], CALL)
        .map_err(|e| anyhow::anyhow!("herdr api snapshot: {}", e.message))?;
    let snap = snap.get("snapshot").cloned().unwrap_or(snap);
    let arr = |k: &str| {
        snap.get(k)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let (agents, panes, tabs, workspaces) =
        (arr("agents"), arr("panes"), arr("tabs"), arr("workspaces"));
    let panes_by = by_key(&panes, "pane_id");
    let tabs_by = by_key(&tabs, "tab_id");
    let agents_by_pane = by_key(&agents, "pane_id");
    let me_pane = panes_by
        .get(&at.pane)
        .cloned()
        .with_context(|| format!("pane {} is not in the live snapshot", at.pane))?;
    let me_agent = agents_by_pane.get(&at.pane).cloned().unwrap_or(json!({}));
    let ws_id = s(&me_pane, "workspace_id");
    let ws = workspaces
        .iter()
        .find(|w| s(w, "workspace_id") == ws_id)
        .cloned()
        .unwrap_or(json!({}));
    let session_id = session_value(&me_pane).or_else(|| session_value(&me_agent));
    let cwd = {
        let f = s(&me_pane, "foreground_cwd");
        if f.is_empty() { s(&me_pane, "cwd") } else { f }
    };
    let repo = at.repo.clone().unwrap_or_else(|| PathBuf::from(&cwd));
    let git = Git::new(ctx.runner, &repo);
    let gstate = git_state(&git);
    let repo = gstate
        .as_ref()
        .map(|g| PathBuf::from(s(g, "repo")))
        .unwrap_or(repo);

    let agent_row = |a: &Value| -> Value {
        let pane_id = s(a, "pane_id");
        let p = panes_by.get(&pane_id).cloned().unwrap_or(json!({}));
        let t = tabs_by.get(&s(a, "tab_id")).cloned().unwrap_or(json!({}));
        let kind = s(a, "agent");
        let cwd = [s(&p, "foreground_cwd"), s(a, "foreground_cwd"), s(a, "cwd")]
            .into_iter()
            .find(|c| !c.is_empty())
            .unwrap_or_default();
        json!({
            "start_args": start_args(at.herdr, &pane_id, &kind),
            "name": a.get("name").cloned().unwrap_or(Value::Null),
            "kind": kind,
            "status": s(a, "agent_status"),
            "pane_id": pane_id,
            "tab_id": s(a, "tab_id"),
            "tab_label": t.get("label").cloned().unwrap_or(Value::Null),
            "pane_label": p.get("label").cloned().unwrap_or(Value::Null),
            "cwd": cwd,
            "session_id": session_value(a),
            "tokens": a.get("tokens").cloned().unwrap_or(json!({})),
            "title": a.get("terminal_title_stripped").cloned().unwrap_or(Value::Null),
        })
    };
    let mut children = Vec::new();
    let mut unlinked = Vec::new();
    for a in &agents {
        if s(a, "pane_id") == at.pane {
            continue;
        }
        let parent = a
            .get("tokens")
            .and_then(|t| t.get("parent"))
            .and_then(Value::as_str);
        if parent == Some(at.pane.as_str()) {
            children.push(agent_row(a));
        } else if s(a, "workspace_id") == ws_id {
            unlinked.push(agent_row(a));
        }
    }
    let other_ws: Vec<Value> = workspaces
        .iter()
        .filter(|w| s(w, "workspace_id") != ws_id)
        .map(|w| json!({"workspace_id": s(w, "workspace_id"), "label": w.get("label"), "status": w.get("agent_status")}))
        .collect();
    let tabs_in_ws = tabs
        .iter()
        .filter(|t| s(t, "workspace_id") == ws_id)
        .count();
    Ok(json!({
        "generated": jiff::Zoned::now().strftime("%Y-%m-%dT%H:%M:%S%:z").to_string(),
        "herdr_version": snap.get("version").cloned().unwrap_or(Value::Null),
        "session": if at.session.is_empty() { "default".to_string() } else { at.session.clone() },
        "workspace": {"id": ws_id, "label": ws.get("label").cloned().unwrap_or(Value::Null), "tabs": tabs_in_ws},
        "coordinator": {
            "pane_id": at.pane,
            "tab_id": s(&me_pane, "tab_id"),
            "pane_label": me_pane.get("label").cloned().unwrap_or(Value::Null),
            "agent_name": me_agent.get("name").cloned().unwrap_or(Value::Null),
            "kind": first_nonempty(&[s(&me_agent, "agent"), s(&me_pane, "agent")]),
            "status": first_nonempty(&[s(&me_agent, "agent_status"), s(&me_pane, "agent_status")]),
            "session_id": session_id,
            "cwd": cwd,
        },
        "workers": children,
        "unlinked_agents": unlinked,
        "other_workspaces": other_ws,
        "git": gstate,
        "files": record_files(&repo),
        "repo": repo.to_string_lossy(),
    }))
}

fn first_nonempty(items: &[String]) -> String {
    items
        .iter()
        .find(|k| !k.is_empty())
        .cloned()
        .unwrap_or_default()
}

// ------------------------------------------------------------------ render

fn table(headers: &[&str], rows: &[Vec<String>]) -> String {
    if rows.is_empty() {
        return "_none_\n".into();
    }
    let mut out = format!(
        "| {} |\n|{}\n",
        headers.join(" | "),
        "---|".repeat(headers.len())
    );
    for r in rows {
        out.push_str(&format!("| {} |\n", r.join(" | ")));
    }
    out
}

fn text_or(v: Option<&Value>, fallback: &str) -> String {
    match v {
        Some(Value::String(s)) if !s.is_empty() => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => fallback.to_string(),
    }
}

/// `state.py render`: the `## Herdr` section.
pub fn render(st: &Value, prefix: &str) -> String {
    let c = &st["coordinator"];
    let mut o = String::new();
    o.push_str(&format!(
        "## Herdr (generated {} by herdr-ade checkpoint, herdr {}, session `{}`)\n\n",
        text_or(st.get("generated"), ""),
        text_or(st.get("herdr_version"), "?"),
        text_or(st.get("session"), "default")
    ));
    o.push_str(&format!(
        "Workspace `{}` ({}), {} tabs. Coordinator: pane `{}` in tab `{}`, agent name `{}`, kind {}, status {}, cwd `{}`.\n\n",
        text_or(st["workspace"].get("id"), ""),
        text_or(st["workspace"].get("label"), "unnamed"),
        text_or(st["workspace"].get("tabs"), "0"),
        text_or(c.get("pane_id"), ""),
        text_or(c.get("tab_id"), ""),
        text_or(c.get("agent_name"), "UNNAMED"),
        text_or(c.get("kind"), ""),
        text_or(c.get("status"), ""),
        text_or(c.get("cwd"), ""),
    ));
    if let Some(sid) = c.get("session_id").and_then(Value::as_str) {
        o.push_str(&format!("Coordinator session id `{sid}`.\n\n"));
    }
    if c.get("agent_name").and_then(Value::as_str).is_none() {
        o.push_str("The coordinator has no agent name, so workers cannot `herdr agent prompt` it. Name it: `herdr agent rename \"$HERDR_PANE_ID\" coordinator`.\n\n");
    }
    let workers = st["workers"].as_array().cloned().unwrap_or_default();
    o.push_str("### Workers nested under the coordinator\n\n");
    let rows: Vec<Vec<String>> = workers
        .iter()
        .map(|w| {
            let tokens = w["tokens"]
                .as_object()
                .map(|m| {
                    m.iter()
                        .filter(|(k, _)| *k != "parent")
                        .map(|(k, v)| format!("{k}={}", v.as_str().unwrap_or("")))
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .filter(|t| !t.is_empty())
                .unwrap_or_else(|| "-".into());
            vec![
                text_or(w.get("name"), "-"),
                text_or(w.get("kind"), ""),
                text_or(w.get("status"), ""),
                format!("`{}`", text_or(w.get("pane_id"), "")),
                format!(
                    "`{}` ({})",
                    text_or(w.get("tab_id"), ""),
                    text_or(w.get("tab_label"), "")
                ),
                format!("`{}`", text_or(w.get("cwd"), "")),
                tokens,
                text_or(w.get("title"), ""),
            ]
        })
        .collect();
    o.push_str(&table(
        &[
            "name",
            "kind",
            "status",
            "pane",
            "tab (label)",
            "cwd",
            "tokens",
            "last title",
        ],
        &rows,
    ));
    if !workers.is_empty() {
        o.push_str("\nStart lines as they run now (from `pane process-info`), for restarting a worker that is gone:\n\n```bash\n");
        for w in &workers {
            let flags = w["start_args"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(" ")
                })
                .filter(|f| !f.is_empty())
                .map(|f| format!(" -- {f}"))
                // No flags recorded: the line starts the agent without any.
                .unwrap_or_default();
            o.push_str(&format!(
                "herdr agent start {} --kind {} --pane <new pane> --parent \"$HERDR_PANE_ID\"{flags}\n",
                text_or(w.get("name"), "-"),
                text_or(w.get("kind"), "")
            ));
        }
        o.push_str("```\n");
    }
    let unlinked = st["unlinked_agents"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !unlinked.is_empty() {
        o.push_str("\n### Agents in this workspace NOT linked to the coordinator\n\n");
        o.push_str(&format!(
            "Re-link the ones that are yours with `{prefix} pickup <slug>`.\n\n"
        ));
        let rows: Vec<Vec<String>> = unlinked
            .iter()
            .map(|w| {
                vec![
                    text_or(w.get("name"), "-"),
                    text_or(w.get("kind"), ""),
                    text_or(w.get("status"), ""),
                    format!("`{}`", text_or(w.get("pane_id"), "")),
                    format!(
                        "`{}` ({})",
                        text_or(w.get("tab_id"), ""),
                        text_or(w.get("tab_label"), "")
                    ),
                    format!("`{}`", text_or(w.get("cwd"), "")),
                    text_or(w.get("title"), ""),
                ]
            })
            .collect();
        o.push_str(&table(
            &[
                "name",
                "kind",
                "status",
                "pane",
                "tab (label)",
                "cwd",
                "last title",
            ],
            &rows,
        ));
    }
    let others = st["other_workspaces"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    if !others.is_empty() {
        let list: Vec<String> = others
            .iter()
            .map(|w| {
                format!(
                    "`{}` {} ({})",
                    text_or(w.get("workspace_id"), ""),
                    text_or(w.get("label"), ""),
                    text_or(w.get("status"), "")
                )
            })
            .collect();
        o.push_str(&format!(
            "\nOther workspaces on this server (not yours to touch): {}\n",
            list.join(", ")
        ));
    }
    o.push_str("\n### Git\n\n");
    match st.get("git").filter(|g| !g.is_null()) {
        None => o.push_str(&format!(
            "No git repository at `{}`.\n",
            text_or(st.get("repo"), "")
        )),
        Some(g) => {
            o.push_str(&format!(
                "Repo `{}`, integration branch `{}` ({}).\n\n",
                text_or(g.get("repo"), ""),
                text_or(g.get("integration_branch"), ""),
                text_or(g.get("remote"), "")
            ));
            let rows: Vec<Vec<String>> = g["worktrees"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|w| {
                    vec![
                        format!("`{}`", text_or(w.get("path"), "")),
                        format!("`{}`", text_or(w.get("branch"), "")),
                        text_or(w.get("head"), ""),
                        text_or(w.get("dirty"), "0"),
                        text_or(w.get("subject"), ""),
                    ]
                })
                .collect();
            o.push_str(&table(
                &["worktree", "branch", "head", "dirty files", "last commit"],
                &rows,
            ));
            let log: Vec<String> = g["integration_log"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .filter_map(|l| l.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            o.push_str(&format!(
                "\nLast commits on the integration branch:\n\n```\n{}\n```\n",
                log.join("\n")
            ));
        }
    }
    if let Some(files) = st.get("files").and_then(Value::as_object) {
        o.push_str("\n### Record files (newest first)\n\n");
        for k in ["handoff", "briefs", "verdicts", "reports"] {
            let list: Vec<String> = files
                .get(k)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|f| f.as_str().map(|f| format!("`{f}`")))
                        .collect()
                })
                .unwrap_or_default();
            if !list.is_empty() {
                o.push_str(&format!("- {k}: {}\n", list.join(", ")));
            }
        }
    }
    o.push_str("\n### Pickup\n\n");
    o.push_str("Run from the coordinator pane after a server restart, or from the fresh coordinator pane that takes over:\n\n```bash\n");
    o.push_str(&format!("{prefix} pickup <slug>\n"));
    if let Some(sid) = c.get("session_id").and_then(Value::as_str) {
        o.push_str(&format!(
            "# the old coordinator conversation, if herdr did not resume it in the pane:\n# cd {} && claude --resume {sid}\n",
            text_or(c.get("cwd"), "")
        ));
    }
    o.push_str("```\n");
    o
}

/// Replaces the `## Herdr` section of a handoff (through the next `## `
/// heading) or appends it.
pub fn splice_herdr(document: &str, section: &str) -> String {
    let mut out = String::new();
    let mut skipping = false;
    let mut replaced = false;
    for line in document.split_inclusive('\n') {
        if line.starts_with("## Herdr") {
            skipping = true;
            if !replaced {
                out.push_str(section);
                if !section.ends_with('\n') {
                    out.push('\n');
                }
                out.push('\n');
                replaced = true;
            }
            continue;
        }
        if skipping && line.starts_with("## ") {
            skipping = false;
        }
        if !skipping {
            out.push_str(line);
        }
    }
    if !replaced {
        if !out.is_empty() && !out.ends_with("\n\n") {
            out.push_str(if out.ends_with('\n') { "\n" } else { "\n\n" });
        }
        out.push_str(section);
    }
    out
}

// ------------------------------------------------------------------- check

/// `state.py check`, over a document and the collected live state. Returns
/// the problem classes; empty means the document checks out.
pub fn check_document(text: &str, st: &Value, repo: &Path) -> BTreeMap<String, BTreeSet<String>> {
    let mut problems: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut add = |class: &str, item: String| {
        problems.entry(class.to_string()).or_default().insert(item);
    };
    let rows = |k: &str| st[k].as_array().cloned().unwrap_or_default();
    let mut live_ids: BTreeSet<String> = BTreeSet::new();
    for list in ["workers", "unlinked_agents", "live_panes"] {
        for w in rows(list) {
            for k in ["pane_id", "tab_id"] {
                let v = s(&w, k);
                if !v.is_empty() {
                    live_ids.insert(v);
                }
            }
        }
    }
    for k in ["pane_id", "tab_id"] {
        let v = s(&st["coordinator"], k);
        if !v.is_empty() {
            live_ids.insert(v);
        }
    }
    let mut live_uuids: BTreeSet<String> = BTreeSet::new();
    if let Some(u) = st["coordinator"]["session_id"].as_str() {
        live_uuids.insert(u.to_string());
    }
    for list in ["workers", "unlinked_agents"] {
        for w in rows(list) {
            if let Some(u) = w["session_id"].as_str() {
                live_uuids.insert(u.to_string());
            }
        }
    }
    let line_of = |start: usize, end: usize| -> &str {
        let a = text[..start].rfind('\n').map_or(0, |i| i + 1);
        let b = text[end..].find('\n').map_or(text.len(), |i| end + i);
        &text[a..b]
    };
    let lower_has = |line: &str, words: &[&str]| {
        let l = line.to_ascii_lowercase();
        words.iter().any(|w| l.contains(w))
    };
    // herdr ids: w<digits>:[pt]<alnum>
    for (start, end, id) in herdr_ids(text) {
        if !live_ids.contains(&id)
            && !lower_has(line_of(start, end), &["closed", "removed", "gone", "stale"])
        {
            add("herdr ids not live and not marked closed", id);
        }
    }
    for (_, _, u) in uuids(text) {
        if !live_uuids.contains(&u) {
            add("session uuids that match no live agent", u);
        }
    }
    for (start, end, p) in backticked(text) {
        let path_like = [
            "tasks/",
            "docs/",
            ".reports/",
            ".worktrees/",
            "src/",
            "scripts/",
        ]
        .iter()
        .any(|pre| p.starts_with(pre))
            || p == "HANDOFF.md"
            || p == "SPEC.md";
        if path_like && !p.contains(char::is_whitespace) {
            if p.contains('<') {
                add("template placeholders left in the document", p.clone());
            } else if !repo.join(&p).exists()
                && !lower_has(
                    line_of(start, end),
                    &[
                        "absent",
                        "expected",
                        "not yet",
                        "will be",
                        "once it lands",
                        "after",
                    ],
                )
            {
                add(
                    "repo paths that do not exist (say 'absent' or 'expected' on the line if that is intended)",
                    p.clone(),
                );
            }
        }
        let branch_like = ["lane/", "review/", "spec/", "track/", "feature/"]
            .iter()
            .any(|pre| p.starts_with(pre));
        if branch_like
            && let Some(branches) = st["git"]["branches"].as_array()
            && !branches.iter().any(|b| b.as_str() == Some(p.as_str()))
        {
            add("branches that do not exist", p.clone());
        }
    }
    for placeholder in angle_placeholders(text) {
        if !placeholder.starts_with("<!--")
            && !placeholder.starts_with("<new pane>")
            && !placeholder.starts_with("<root_pane")
            && !placeholder.starts_with("<slug>")
        {
            add("template placeholders left in the document", placeholder);
        }
    }
    for w in rows("workers") {
        if let Some(name) = w["name"].as_str()
            && !text.contains(name)
        {
            add(
                "live nested workers the document never mentions",
                name.to_string(),
            );
        }
    }
    for sec in [
        "## Goal",
        "## Authority",
        "## Settled",
        "## In flight",
        "## Open",
        "## Next",
        "## Traps",
        "## Herdr",
    ] {
        if !text.contains(sec) {
            add("required sections missing", sec.to_string());
        }
    }
    if let Some(start) = text.find("## Next") {
        let body = &text[start + "## Next".len()..];
        let body = body.find("\n## ").map_or(body, |i| &body[..i]);
        let bullets = body
            .lines()
            .filter(|l| {
                let t = l.trim_start();
                t.starts_with('-') || t.starts_with('*') || t.starts_with('1') || t.starts_with('2')
            })
            .count();
        if bullets > 1 {
            add(
                "Next must be exactly one action",
                "more than one bullet under ## Next".into(),
            );
        }
    }
    problems
}

fn herdr_ids(text: &str) -> Vec<(usize, usize, String)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let boundary = i == 0 || !b[i - 1].is_ascii_alphanumeric() && b[i - 1] != b'_';
        if boundary && b[i] == b'w' {
            let mut j = i + 1;
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            if j > i + 1
                && j + 1 < b.len()
                && b[j] == b':'
                && (b[j + 1] == b'p' || b[j + 1] == b't')
            {
                let mut k = j + 2;
                while k < b.len() && b[k].is_ascii_alphanumeric() {
                    k += 1;
                }
                let end_ok = k == b.len() || !(b[k].is_ascii_alphanumeric() || b[k] == b'_');
                if k > j + 2 && end_ok {
                    out.push((i, k, text[i..k].to_string()));
                    i = k;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

fn uuids(text: &str) -> Vec<(usize, usize, String)> {
    let b = text.as_bytes();
    let groups = [8, 4, 4, 4, 12];
    let mut out = Vec::new();
    let mut i = 0;
    while i + 36 <= b.len() {
        let before_ok = i == 0
            || !(b[i - 1].is_ascii_alphanumeric()
                || b[i - 1] == b'/'
                || b[i - 1] == b'-'
                || b[i - 1] == b'_');
        let mut j = i;
        let mut ok = before_ok;
        for (gi, g) in groups.iter().enumerate() {
            for _ in 0..*g {
                if ok && j < b.len() && (b[j].is_ascii_digit() || (b'a'..=b'f').contains(&b[j])) {
                    j += 1;
                } else {
                    ok = false;
                }
            }
            if gi < 4 {
                if ok && j < b.len() && b[j] == b'-' {
                    j += 1;
                } else {
                    ok = false;
                }
            }
        }
        let after_ok = j == b.len()
            || !(b[j].is_ascii_alphanumeric() || b[j] == b'/' || b[j] == b'-' || b[j] == b'_');
        if ok && after_ok {
            out.push((i, j, text[i..j].to_string()));
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

fn backticked(text: &str) -> Vec<(usize, usize, String)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(a) = text[from..].find('`') {
        let start = from + a + 1;
        let Some(b) = text[start..].find('`') else {
            break;
        };
        let end = start + b;
        let inner = &text[start..end];
        if !inner.contains('\n') && !inner.is_empty() {
            out.push((start, end, inner.to_string()));
        }
        from = end + 1;
    }
    out
}

fn angle_placeholders(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(a) = text[from..].find('<') {
        let start = from + a;
        let rest = &text[start + 1..];
        let Some(close) = rest.find('>') else { break };
        let inner = &rest[..close];
        let first_ok = inner
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic());
        let len = inner.chars().count();
        if first_ok && (2..=60).contains(&len) && !inner.contains(['<', '`', '\n']) {
            out.push(format!("<{inner}>"));
        }
        from = start + 1;
    }
    out
}

// ------------------------------------------------------------ the verbs

fn coordinator_where<'a>(
    ctx: &'a Ctx,
    project: &Project,
    pane: Option<&str>,
) -> Result<(Herdr<'a>, String, String)> {
    let coord = project
        .coordinator()
        .context("the project has not been opened")?;
    let pane = pane
        .map(str::to_string)
        .or_else(|| ctx.env.var("HERDR_PANE_ID").map(str::to_string))
        .unwrap_or(coord.pane_id.clone());
    Ok((
        Herdr::new(ctx.env.herdr_bin(), &coord.socket, ctx.runner),
        pane,
        coord.session,
    ))
}

/// The integration branch and repo a checkpoint commits to: the newest
/// round's, else the project's repository and its checked-out branch.
fn integration(
    ctx: &Ctx,
    project: &Project,
    repo: Option<&str>,
    branch: Option<&str>,
) -> Result<(PathBuf, String)> {
    let newest = crate::round::list(project).pop();
    let repo = match (repo, &newest) {
        (Some(r), _) => PathBuf::from(r),
        (None, Some(r)) => PathBuf::from(&r.repo),
        (None, None) => {
            let (settings, _) = project.read_project_md()?;
            PathBuf::from(
                settings
                    .repos
                    .iter()
                    .find(|r| r.machine.is_none())
                    .map(|r| r.path.clone())
                    .context("checkpoint_needs_repo: pass --repo")?,
            )
        }
    };
    let branch = match (branch, &newest) {
        (Some(b), _) => b.to_string(),
        (None, Some(r)) => r.branch.clone(),
        (None, None) => Git::new(ctx.runner, &repo).run(&["symbolic-ref", "--short", "HEAD"])?,
    };
    Ok((repo, branch))
}

/// The HANDOFF pair for an automatic checkpoint after a round merge: the
/// handoff at `V` (or a skeleton) with a fresh `## Herdr` section.
pub fn compose_for_round(
    ctx: &Ctx,
    project: &Project,
    record: &RoundRecord,
    v: &str,
) -> Result<(String, String)> {
    let git = Git::new(ctx.runner, &record.repo);
    let base = git.show_file(v, "HANDOFF.md")?.unwrap_or_else(skeleton);
    let prefix =
        crate::coordinator::current_prefix(&ctx.root).unwrap_or_else(|_| "herdr-ade".into());
    let state = match coordinator_where(ctx, project, None) {
        Ok((h, pane, session)) => collect(
            ctx,
            &Where {
                herdr: &h,
                pane,
                session,
                repo: Some(PathBuf::from(&record.repo)),
            },
        )
        .unwrap_or_else(|e| json!({"unavailable": format!("{e:#}")})),
        Err(e) => json!({"unavailable": format!("{e:#}")}),
    };
    let mut section = if state.get("unavailable").is_some() {
        format!(
            "## Herdr\n\nThe herdr snapshot was unavailable at this checkpoint: {}\n",
            text_or(state.get("unavailable"), "")
        )
    } else {
        render(&state, &prefix)
    };
    section.push_str(&format!(
        "\nRound `{}` was merged into `{}` at verdict commit `{v}`; this checkpoint is its child.\n",
        record.round, record.branch
    ));
    let md = splice_herdr(&base, &section);
    let mut sidecar = state;
    sidecar["round"] = json!({"round": record.round, "branch": record.branch, "verdict": v});
    let json = format!("{}\n", serde_json::to_string_pretty(&sidecar)?);
    Ok((md, json))
}

fn skeleton() -> String {
    "# HANDOFF\n\n## Goal\n\n## Authority\n\n## Settled\n\n## In flight\n\n## Open\n\n## Next\n\n## Traps\n\n".into()
}

pub struct CheckpointArgs {
    pub pane: Option<String>,
    pub repo: Option<String>,
    pub branch: Option<String>,
    /// Print the generated section only; write nothing.
    pub print: bool,
    /// Check the committed handoff only; write nothing.
    pub check_only: bool,
}

/// `ha checkpoint <slug>`: snapshot, check, commit both files as `H`.
pub fn checkpoint(ctx: &Ctx, slug: &str, args: CheckpointArgs) -> Result<String> {
    let project = Project::load(&ctx.root, slug)?;
    let (repo, branch) = integration(ctx, &project, args.repo.as_deref(), args.branch.as_deref())?;
    let (h, pane, session) = coordinator_where(ctx, &project, args.pane.as_deref())?;
    let st = collect(
        ctx,
        &Where {
            herdr: &h,
            pane,
            session,
            repo: Some(repo.clone()),
        },
    )?;
    let prefix = crate::coordinator::current_prefix(&ctx.root)?;
    let section = render(&st, &prefix);
    if args.print {
        return Ok(section);
    }
    let git = Git::new(ctx.runner, &repo);
    let head = git
        .branch_head(&branch)?
        .with_context(|| format!("branch_missing: `{branch}`"))?;
    let checkout = git.checkout_of(&branch)?;
    let current = match &checkout {
        Some(dir) => std::fs::read_to_string(dir.join("HANDOFF.md")).ok(),
        None => git.show_file(&head, "HANDOFF.md")?,
    }
    .unwrap_or_else(skeleton);
    if args.check_only {
        let problems = check_document(&current, &st, &repo);
        return report(&problems, "HANDOFF.md", &st);
    }
    let md = splice_herdr(&current, &section);
    let problems = check_document(&md, &st, &repo);
    if !problems.is_empty() {
        let text = report(&problems, "HANDOFF.md", &st).unwrap_err();
        bail!("checkpoint_check_failed: nothing was committed\n{text:#}");
    }
    let json = format!("{}\n", serde_json::to_string_pretty(&st)?);
    let commit = {
        let _repo = repo_lock(&git)?;
        commit_files_on_branch(
            &git,
            &branch,
            &[("HANDOFF.md", md.as_str()), ("HANDOFF.json", json.as_str())],
            "checkpoint: HANDOFF",
            &head,
            &project.state_dir().join("tmp"),
        )?
    };
    // The shared plan refresh at the checkpoint's durable completion
    // boundary; a failure is separate and never affects the commit
    // (SPEC-talk §6.5).
    if let Err(e) = crate::plan::refresh(ctx, &project) {
        eprintln!("note: the plan refresh failed: {e:#}");
    }
    Ok(format!(
        "checkpoint H {commit} on `{branch}` (payload {})\n",
        payload_hash(&md, &json)
    ))
}

fn report(problems: &BTreeMap<String, BTreeSet<String>>, doc: &str, st: &Value) -> Result<String> {
    if problems.is_empty() {
        let n = st["workers"].as_array().map_or(0, Vec::len);
        return Ok(format!(
            "OK {doc}: {n} nested workers mentioned, all ids/paths/branches/uuids resolve, sections present, Next is one action\n"
        ));
    }
    let lines: Vec<String> = problems
        .iter()
        .map(|(k, v)| {
            format!(
                "FAIL {k}: {}",
                v.iter().cloned().collect::<Vec<_>>().join(", ")
            )
        })
        .collect();
    bail!("{}", lines.join("\n"))
}

/// `ha pickup [slug] [--all] [--start]`: re-apply parents to live threads
/// whose identity still matches. Local lanes are read from the project's own
/// session; box lanes are read from the courier's box-local lists (one SSH per
/// machine, never a second bridge). Gone lanes are printed as start lines, or
/// restarted through the existing restart path when `--start` runs under a
/// project whose `start_threads` is `auto`.
pub struct PickupArgs<'a> {
    pub slug: Option<&'a str>,
    pub pane: Option<&'a str>,
    pub dry_run: bool,
    pub all: bool,
    pub start: bool,
}

pub fn pickup(ctx: &Ctx, args: PickupArgs<'_>) -> Result<String> {
    if !args.all && args.slug.is_none() {
        bail!("pickup_needs_project: pass a project slug or --all");
    }
    let slugs: Vec<String> = if args.all {
        active_slugs(ctx)
    } else {
        vec![args.slug.unwrap_or_default().to_string()]
    };
    let projects: Vec<Project> = slugs
        .iter()
        .map(|slug| Project::load(&ctx.root, slug))
        .collect::<Result<_>>()?;
    let views = machine_views(ctx, &projects);
    let mut out = String::new();
    for project in &projects {
        out.push_str(&pickup_project(ctx, project, &views, &args)?);
    }
    Ok(out)
}

/// Every project under the root that is not archived.
fn active_slugs(ctx: &Ctx) -> Vec<String> {
    crate::project::list_slugs(&ctx.root)
        .into_iter()
        .filter(|slug| {
            Project::load(&ctx.root, slug)
                .is_ok_and(|p| p.status() != crate::project::Status::Archived)
        })
        .collect()
}

/// One courier pass per machine that has an open box lane in any project, so
/// `--all` costs one SSH per machine, not one per project. A failed pass is
/// kept as its reason: its lanes are never invented as gone.
fn machine_views(
    ctx: &Ctx,
    projects: &[Project],
) -> BTreeMap<String, Result<crate::steps::CourierOutcome, String>> {
    let mut by_machine: BTreeMap<String, Vec<&Project>> = BTreeMap::new();
    for project in projects {
        for t in thread::list(project) {
            if t.status == thread::Status::Resolved || !t.is_remote() || t.pane_id.is_empty() {
                continue;
            }
            let entry = by_machine.entry(t.machine_route().to_string()).or_default();
            if !entry.iter().any(|p| p.slug == project.slug) {
                entry.push(project);
            }
        }
    }
    let mut views = BTreeMap::new();
    for (machine, projects) in &by_machine {
        views.insert(
            machine.clone(),
            crate::steps::courier(ctx, projects, machine).map_err(|e| format!("{e:#}")),
        );
    }
    views
}

fn pickup_project(
    ctx: &Ctx,
    project: &Project,
    views: &BTreeMap<String, Result<crate::steps::CourierOutcome, String>>,
    args: &PickupArgs<'_>,
) -> Result<String> {
    let (h, coord_pane, _) = coordinator_where(ctx, project, args.pane)?;
    let coord = project.coordinator().unwrap_or_default();
    let listed = h
        .call(&["agent", "list"], CALL)
        .map_err(|e| anyhow::anyhow!("herdr agent list: {}", e.message))?;
    let agents = listed["agents"].as_array().cloned().unwrap_or_default();
    let by_pane = by_key(&agents, "pane_id");
    let now = jiff::Timestamp::now();
    let (mut relinked, mut already) = (Vec::new(), Vec::new());
    let mut gone: Vec<thread::Thread> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    for t in thread::list(project) {
        if t.status == thread::Status::Resolved || t.pane_id.is_empty() {
            continue;
        }
        if t.is_remote() {
            let view = match views.get(t.machine_route()) {
                Some(Ok(view)) => view,
                Some(Err(error)) => {
                    notes.push(format!("machine `{}` unreachable: {error}", t.machine));
                    continue;
                }
                None => {
                    notes.push(format!("machine `{}` was not read this pass", t.machine));
                    continue;
                }
            };
            let (Some(box_agents), Some(box_panes)) = (&view.agents, &view.panes) else {
                notes.push(format!(
                    "machine `{}` did not answer; {} is undecided",
                    t.machine, t.id
                ));
                continue;
            };
            if !thread::live_state(&t, box_agents, box_panes, now).pane_exists {
                gone.push(t);
                continue;
            }
            let parent = crate::threads::parent_token(&t, &coord_pane);
            let current = box_agents
                .iter()
                .find(|a| thread::agent_matches(&t, a))
                .and_then(crate::herdr::Agent::parent);
            if current == Some(parent.as_str()) {
                already.push(t.id.clone());
            } else if args.dry_run {
                relinked.push(format!("{} (dry-run)", t.id));
            } else {
                h.on_machine(t.machine_route())
                    .pane_set_parent(&t.pane_id, &parent)
                    .map_err(|e| anyhow::anyhow!("re-parenting {}: {}", t.id, e.message))?;
                relinked.push(t.id.clone());
            }
            continue;
        }
        let live = by_pane.get(&t.pane_id).filter(|a| {
            let name = s(a, "name");
            t.agent_name.is_empty() || name.is_empty() || name == t.agent_name
        });
        let Some(a) = live else {
            gone.push(t);
            continue;
        };
        let parent = a
            .get("tokens")
            .and_then(|t| t.get("parent"))
            .and_then(Value::as_str);
        if parent == Some(coord_pane.as_str()) {
            already.push(t.id.clone());
        } else if args.dry_run {
            relinked.push(format!("{} (dry-run)", t.id));
        } else {
            let token = format!("parent={coord_pane}");
            h.call(
                &[
                    "pane",
                    "report-metadata",
                    &t.pane_id,
                    "--source",
                    crate::herdr::SOURCE,
                    "--token",
                    &token,
                ],
                CALL,
            )
            .map_err(|e| anyhow::anyhow!("re-parenting {}: {}", t.id, e.message))?;
            relinked.push(t.id.clone());
        }
    }
    let mut out = format!("project {}\ncoordinator pane {coord_pane}\n", project.slug);
    out.push_str(&format!(
        "already linked: {}\n",
        if already.is_empty() {
            "-".into()
        } else {
            already.join(", ")
        }
    ));
    out.push_str(&format!(
        "re-linked:      {}\n",
        if relinked.is_empty() {
            "-".into()
        } else {
            relinked.join(", ")
        }
    ));
    for note in &notes {
        out.push_str(&format!("note: {note}\n"));
    }
    if !gone.is_empty() {
        let safety = project.safety(&ctx.config_dir)?;
        if args.start && !args.dry_run && safety.start_threads == "auto" {
            out.push_str("started (restarted through the launch record):\n");
            for t in &gone {
                match crate::threads::restart(ctx, &project.slug, &t.id) {
                    Ok(thread) => {
                        out.push_str(&format!("  {} now in pane {}\n", t.id, thread.pane_id))
                    }
                    Err(error) => out.push_str(&format!("  {} could not start: {error:#}\n", t.id)),
                }
            }
        } else {
            out.push_str(
                "gone (not live; the ticker or you start them again; nothing was started):\n",
            );
            for t in &gone {
                out.push_str(&start_lines(project, t, &coord_pane, &coord.workspace_id));
            }
        }
    }
    Ok(out)
}

/// The copy-ready `herdr` commands for one gone lane, from its launch recipe.
/// A box lane's commands run through `herdr --machine <label>` and name the
/// coordinator with the machine-qualified parent token.
fn start_lines(
    project: &Project,
    t: &thread::Thread,
    coord_pane: &str,
    coord_workspace: &str,
) -> String {
    let launch = launch_of(project, &t.id);
    let kind = launch.kind.clone().unwrap_or_else(|| t.agent.clone());
    let parent = crate::threads::parent_token(t, coord_pane);
    let (herdr, workspace, cwd) = if t.is_remote() {
        (
            format!("herdr --machine {}", t.machine),
            t.workspace_id.clone(),
            t.cwd.clone(),
        )
    } else {
        (
            "herdr".to_string(),
            coord_workspace.to_string(),
            t.cwd.clone(),
        )
    };
    let mut out = format!(
        "  {herdr} tab create --workspace {workspace} --cwd {cwd} --label {} --no-focus",
        t.id
    );
    for e in &launch.env {
        out.push_str(&format!(" --env {e}"));
    }
    out.push('\n');
    out.push_str(&format!(
        "  {herdr} agent start {} --kind {kind} --pane <root_pane.pane_id from that JSON> --parent {parent}",
        if t.agent_name.is_empty() {
            t.id.clone()
        } else {
            t.agent_name.clone()
        }
    ));
    if let Some(ms) = launch.ready_timeout_ms.filter(|ms| *ms > 0) {
        out.push_str(&format!(" --timeout {ms}"));
    }
    if !launch.args.is_empty() {
        out.push_str(&format!(" -- {}", launch.args.join(" ")));
    }
    out.push('\n');
    out
}

#[derive(Default)]
struct LaunchSeen {
    kind: Option<String>,
    args: Vec<String>,
    env: Vec<String>,
    ready_timeout_ms: Option<u64>,
}

/// The launch recipe on A1's record (`[launch]`, SPEC-ADE D2).
fn launch_of(project: &Project, id: &str) -> LaunchSeen {
    let Ok(t) = thread::load(project, id) else {
        return LaunchSeen::default();
    };
    let l = t.launch;
    LaunchSeen {
        kind: Some(l.kind).filter(|k| !k.is_empty()),
        args: l.args,
        env: l.env,
        ready_timeout_ms: Some(l.ready_timeout_ms).filter(|ms| *ms > 0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::round::testkit::{Fx, commit_file, fixture, git};
    use crate::runner::fake::{fail, ok};

    const HANDOFF: &str = "# HANDOFF\n\n## Goal\n\nShip the rounds.\n\n## Authority\n\nRolf.\n\n## Settled\n\n## In flight\n\nThe worker lane-one writes `src/lane1.rs` (expected).\n\n## Open\n\n## Next\n\n- Review round one.\n\n## Traps\n\n";

    fn snapshot(fx: &Fx) {
        let repo = fx.repo.display();
        fx.world.runner.on(
            "api snapshot",
            ok(&format!(
                r#"{{"result":{{"snapshot":{{"version":"0.9.1",
                "workspaces":[{{"workspace_id":"w1","label":"demo"}}],
                "tabs":[{{"tab_id":"w1:t1","workspace_id":"w1","label":"coord"}},{{"tab_id":"w1:t2","workspace_id":"w1","label":"lane"}}],
                "panes":[{{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","cwd":"{repo}"}},{{"pane_id":"w1:p11","tab_id":"w1:t2","workspace_id":"w1","cwd":"{repo}"}}],
                "agents":[{{"pane_id":"w1:p1","tab_id":"w1:t1","workspace_id":"w1","name":"hp-demo-coordinator","agent":"claude","agent_status":"idle"}},
                          {{"pane_id":"w1:p11","tab_id":"w1:t2","workspace_id":"w1","name":"lane-one","agent":"claude","agent_status":"working","tokens":{{"parent":"w1:p1"}}}}]}}}}}}"#
            )),
        );
        fx.world.runner.on("pane process-info", fail(1, "no"));
    }

    fn args(fx: &Fx) -> CheckpointArgs {
        CheckpointArgs {
            pane: Some("w1:p1".into()),
            repo: Some(fx.repo.to_string_lossy().into_owned()),
            branch: Some("main".into()),
            print: false,
            check_only: false,
        }
    }

    #[test]
    fn the_payload_hash_binds_both_files_and_their_order() {
        assert_ne!(payload_hash("a", "b"), payload_hash("b", "a"));
        assert_ne!(payload_hash("ab", ""), payload_hash("a", "b"));
        assert_eq!(payload_hash("a", "b"), payload_hash("a", "b"));
    }

    #[test]
    fn splice_replaces_the_herdr_section_or_appends_it() {
        let doc = "# H\n\n## Goal\n\ng\n\n## Herdr\n\nold\n\n## Next\n\n- n\n";
        let out = splice_herdr(doc, "## Herdr (new)\n\nfresh\n");
        assert_eq!(
            out,
            "# H\n\n## Goal\n\ng\n\n## Herdr (new)\n\nfresh\n\n## Next\n\n- n\n"
        );
        assert_eq!(
            splice_herdr(&out, "## Herdr (new)\n\nfresh\n"),
            out,
            "idempotent"
        );
        assert_eq!(splice_herdr("# H\n", "## Herdr\n"), "# H\n\n## Herdr\n");
    }

    #[test]
    fn check_finds_dead_ids_missing_paths_placeholders_sections_and_two_nexts() {
        let fx = fixture();
        let st = json!({
            "coordinator": {"pane_id": "w1:p1", "tab_id": "w1:t1"},
            "workers": [{"pane_id": "w1:p11", "tab_id": "w1:t2", "name": "lane-one"}],
            "git": {"branches": ["main", "lane/1"]},
        });
        let good = format!(
            "{HANDOFF}## Herdr\n\nPane w1:p1 and lane-one in w1:p11, pane w1:p5 is closed. Branch `lane/1`, `README.md`.\n"
        );
        let problems = check_document(&good, &st, &fx.repo);
        assert!(problems.is_empty(), "{problems:?}");
        let bad = "# H\n\n## Next\n\n- one\n- two\n\nPane w1:p5 and `tasks/nope.md`, `lane/9`, <fill me>.\n";
        let problems = check_document(bad, &st, &fx.repo);
        let classes: Vec<&str> = problems.keys().map(String::as_str).collect();
        assert_eq!(
            classes,
            [
                "Next must be exactly one action",
                "branches that do not exist",
                "herdr ids not live and not marked closed",
                "live nested workers the document never mentions",
                "repo paths that do not exist (say 'absent' or 'expected' on the line if that is intended)",
                "required sections missing",
                "template placeholders left in the document",
            ]
        );
        assert!(problems["herdr ids not live and not marked closed"].contains("w1:p5"));
    }

    #[test]
    fn checkpoint_commits_both_files_as_one_commit_or_nothing() {
        let fx = fixture();
        snapshot(&fx);
        let ctx = fx.world.ctx();
        commit_file(
            &fx.repo,
            "HANDOFF.md",
            "# HANDOFF\n\n## Next\n\n- a\n- b\n",
            "bad handoff",
        );
        let before = git(&fx.repo, &["rev-parse", "main"]);
        let e = format!("{:#}", checkpoint(&ctx, "demo", args(&fx)).unwrap_err());
        assert!(
            e.starts_with("checkpoint_check_failed")
                && e.contains("Next must be exactly one action"),
            "{e}"
        );
        assert_eq!(
            git(&fx.repo, &["rev-parse", "main"]),
            before,
            "nothing was committed"
        );

        commit_file(&fx.repo, "HANDOFF.md", HANDOFF, "handoff");
        let before = git(&fx.repo, &["rev-parse", "main"]);
        let printed = checkpoint(
            &ctx,
            "demo",
            CheckpointArgs {
                print: true,
                ..args(&fx)
            },
        )
        .unwrap();
        assert!(
            printed.starts_with("## Herdr (generated ") && printed.contains("lane-one"),
            "{printed}"
        );
        assert_eq!(
            git(&fx.repo, &["rev-parse", "main"]),
            before,
            "--print writes nothing"
        );
        let out = checkpoint(&ctx, "demo", args(&fx)).unwrap();
        let h = git(&fx.repo, &["rev-parse", "main"]);
        assert!(
            out.starts_with(&format!("checkpoint H {h} on `main`")),
            "{out}"
        );
        assert_eq!(git(&fx.repo, &["rev-parse", "main^"]), before);
        assert_eq!(
            git(&fx.repo, &["diff", "--name-only", &before, &h]),
            "HANDOFF.json\nHANDOFF.md"
        );
        let md = git(&fx.repo, &["show", "main:HANDOFF.md"]);
        assert!(md.contains("## Herdr (generated ") && md.contains("## Traps"));
        let sidecar: Value =
            serde_json::from_str(&git(&fx.repo, &["show", "main:HANDOFF.json"])).unwrap();
        assert_eq!(sidecar["coordinator"]["pane_id"], "w1:p1");
        assert_eq!(sidecar["workers"][0]["name"], "lane-one");
        let checked = checkpoint(
            &ctx,
            "demo",
            CheckpointArgs {
                check_only: true,
                ..args(&fx)
            },
        )
        .unwrap();
        assert!(
            checked.starts_with("OK HANDOFF.md: 1 nested workers"),
            "{checked}"
        );
    }

    #[test]
    fn pickup_relinks_live_threads_and_prints_start_lines_but_never_starts() {
        let fx = fixture();
        let (live, _) = fx.lane(1);
        let (linked, _) = fx.lane(2);
        let (gone, _) = fx.lane(3);
        thread::update(&fx.project, &gone, |t| {
            t.launch.kind = "codex".into();
            t.launch.args = vec!["--full-auto".into()];
            t.launch.env = vec!["HERDR_ADE_LAUNCH=1".into()];
            t.launch.ready_timeout_ms = 30000;
        })
        .unwrap();
        *fx.world.agents.borrow_mut() = r#"[
            {"pane_id":"w1:p11","tab_id":"w1:t2","workspace_id":"w1","name":"","agent":"claude","agent_status":"working"},
            {"pane_id":"w1:p12","tab_id":"w1:t3","workspace_id":"w1","name":"","agent":"claude","agent_status":"idle","tokens":{"parent":"w1:p1"}}]"#
            .into();
        let ctx = fx.world.ctx();
        let dry = pickup(
            &ctx,
            PickupArgs {
                slug: Some("demo"),
                pane: Some("w1:p1"),
                dry_run: true,
                all: false,
                start: false,
            },
        )
        .unwrap();
        assert!(
            dry.contains(&format!("re-linked:      {live} (dry-run)")),
            "{dry}"
        );
        assert_eq!(fx.world.runner.count("pane report-metadata"), 0);
        let out = pickup(
            &ctx,
            PickupArgs {
                slug: Some("demo"),
                pane: Some("w1:p1"),
                dry_run: false,
                all: false,
                start: false,
            },
        )
        .unwrap();
        assert!(
            out.contains(&format!("already linked: {linked}\n")),
            "{out}"
        );
        assert!(out.contains(&format!("re-linked:      {live}\n")), "{out}");
        assert_eq!(
            fx.world
                .runner
                .count("pane report-metadata w1:p11 --source herdr-ade --token parent=w1:p1"),
            1
        );
        assert!(
            out.contains(&format!(
                "--label {gone} --no-focus --env HERDR_ADE_LAUNCH=1"
            )),
            "{out}"
        );
        assert!(out.contains("--kind codex --pane <root_pane.pane_id from that JSON> --parent w1:p1 --timeout 30000 -- --full-auto"), "{out}");
        for verb in ["agent start", "agent prompt", "tab create", "pane run"] {
            assert_eq!(fx.world.runner.count(verb), 0, "pickup ran `{verb}`");
        }
    }

    /// One box lane on saved machine `box` (id `1`), live at `pane`.
    fn box_lane(fx: &Fx, pane: &str, cwd: &str) -> String {
        thread::allocate(&fx.project, |t| {
            t.title = "Box lane".into();
            t.kind = thread::Kind::Worktree;
            t.status = thread::Status::Open;
            t.agent = "claude".into();
            t.agent_name = thread::agent_name("demo", &t.id);
            t.machine = "box".into();
            t.workspace_id = "w2".into();
            t.tab_id = "w2:t1".into();
            t.pane_id = pane.into();
            t.cwd = cwd.into();
        })
        .unwrap()
        .id
    }

    /// The saved-machine profile and one courier manifest with these box-local
    /// `agent list`/`pane list` arrays.
    fn box_courier(world: &crate::scenarios::World, agents: &str, panes: &str) {
        world.runner.on(
            "machine list --json",
            ok(r#"[{"id":"1","label":"box","target":"me@box","session":"default","enabled":true}]"#),
        );
        let manifest = format!(
            "boot\tboot-1\nfree\t100\nagents\t{{\"result\":{{\"agents\":{agents}}}}}\npanes\t{{\"result\":{{\"panes\":{panes}}}}}\n"
        );
        world.runner.on("ssh", ok(&manifest));
    }

    #[test]
    fn pickup_relinks_a_live_box_lane_with_the_machine_qualified_parent() {
        let fx = fixture();
        let id = box_lane(&fx, "w2:p1", "/box/wt");
        box_courier(
            &fx.world,
            r#"[{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","name":"hp-demo-t-0001","agent":"claude","agent_status":"idle","cwd":"/box/wt"}]"#,
            r#"[{"pane_id":"w2:p1","tab_id":"w2:t1","workspace_id":"w2","cwd":"/box/wt"}]"#,
        );
        let ctx = fx.world.ctx();
        let out = pickup(
            &ctx,
            PickupArgs {
                slug: Some("demo"),
                pane: Some("w1:p1"),
                dry_run: false,
                all: false,
                start: false,
            },
        )
        .unwrap();
        assert!(out.contains(&format!("re-linked:      {id}\n")), "{out}");
        assert_eq!(
            fx.world
                .runner
                .count("pane report-metadata w2:p1 --source herdr-ade --token parent=Local:w1:p1"),
            1
        );
        assert!(
            fx.world
                .runner
                .calls
                .borrow()
                .iter()
                .any(|c| c.display().contains("--machine box pane report-metadata")),
            "the re-parent must go through the box bridge"
        );
    }

    #[test]
    fn pickup_prints_a_machine_start_line_for_a_gone_box_lane() {
        let fx = fixture();
        let id = box_lane(&fx, "w2:p1", "/box/wt");
        box_courier(&fx.world, "[]", "[]");
        let ctx = fx.world.ctx();
        let out = pickup(
            &ctx,
            PickupArgs {
                slug: Some("demo"),
                pane: Some("w1:p1"),
                dry_run: false,
                all: false,
                start: false,
            },
        )
        .unwrap();
        assert!(
            out.contains(&format!(
                "herdr --machine box tab create --workspace w2 --cwd /box/wt --label {id}"
            )),
            "{out}"
        );
        assert!(
            out.contains(
                "herdr --machine box agent start hp-demo-t-0001 --kind claude --pane <root_pane.pane_id from that JSON> --parent Local:w1:p1"
            ),
            "{out}"
        );
        assert_eq!(fx.world.runner.count("pane report-metadata"), 0);
    }

    #[test]
    fn pickup_all_covers_two_projects() {
        let fx = fixture();
        let _ = fx.lane(1);
        let beta = fx.world.project("beta", "b.sock");
        thread::allocate(&beta, |t| {
            t.title = "Beta".into();
            t.kind = thread::Kind::Tab;
            t.status = thread::Status::Open;
            t.agent = "claude".into();
            t.agent_name = "hp-beta-t-0001".into();
            t.workspace_id = "w3".into();
            t.tab_id = "w3:t1".into();
            t.pane_id = "w3:p1".into();
            t.cwd = fx.world.home.path().to_string_lossy().into_owned();
        })
        .unwrap();
        let ctx = fx.world.ctx();
        let out = pickup(
            &ctx,
            PickupArgs {
                slug: None,
                pane: Some("w1:p1"),
                dry_run: true,
                all: true,
                start: false,
            },
        )
        .unwrap();
        assert!(out.contains("project demo\n"), "{out}");
        assert!(out.contains("project beta\n"), "{out}");
    }

    #[test]
    fn pickup_start_starts_only_under_auto_and_skips_a_relinked_lane() {
        let fx = fixture();
        let (linked, _) = fx.lane(1);
        *fx.world.agents.borrow_mut() = r#"[
            {"pane_id":"w1:p11","tab_id":"w1:t2","workspace_id":"w1","name":"","agent":"claude","agent_status":"idle","tokens":{"parent":"w1:p1"}}]"#
            .into();
        let gone = thread::allocate(&fx.project, |t| {
            t.title = "Gone".into();
            t.kind = thread::Kind::Tab;
            t.status = thread::Status::Open;
            t.agent = "claude".into();
            t.agent_name = thread::agent_name("demo", &t.id);
            t.workspace_id = "w1".into();
            t.tab_id = "w1:t3".into();
            t.pane_id = "w1:p12".into();
            t.cwd = fx.world.home.path().to_string_lossy().into_owned();
        })
        .unwrap()
        .id;
        *fx.world.panes.borrow_mut() = format!("[{}]", fx.world.coordinator_pane(&fx.project));
        fx.world.runner.on(
            "tab create",
            ok(r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t9","pane_id":"w1:p99","cwd":"/tmp"}}}"#),
        );
        let ctx = fx.world.ctx();
        let propose = PickupArgs {
            slug: Some("demo"),
            pane: Some("w1:p1"),
            dry_run: false,
            all: false,
            start: true,
        };
        // The default `propose` row prints, as today: nothing is started.
        let out = pickup(&ctx, propose).unwrap();
        assert!(out.contains("gone (not live;"), "{out}");
        assert_eq!(fx.world.runner.count("tab create"), 0);

        let config = fx.world.home.path().join("cfg/config.toml");
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(
            &config,
            format!(
                "[safety.\"{}\"]\nstart_threads = \"auto\"\n",
                fx.project.canonical_dir().display()
            ),
        )
        .unwrap();
        let out = pickup(
            &ctx,
            PickupArgs {
                slug: Some("demo"),
                pane: Some("w1:p1"),
                dry_run: false,
                all: false,
                start: true,
            },
        )
        .unwrap();
        assert!(out.contains(&format!("{gone} now in pane")), "{out}");
        assert!(!out.contains(&format!("re-linked:      {linked}")), "{out}");
        assert_eq!(fx.world.runner.count("tab create"), 1);
    }
}
