//! One short-lived installed-binary endpoint; SSH transports only selected inputs.
use crate::{paths::Ctx, remote::MachineDeclaration, runner::Runner};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Request {
    Doctor(crate::doctor::ProbePlan),
    Courier {
        session: String,
        taken: Vec<(String, String)>,
    },
    Linked {
        root: PathBuf,
        relative: PathBuf,
        offset: Option<u64>,
    },
    Missing {
        root: PathBuf,
        relative: PathBuf,
    },
    RepoLink {
        root: PathBuf,
        relative: PathBuf,
    },
    ForgetWorktree {
        repo: String,
        path: String,
    },
    RemoveWorktree {
        repo: String,
        path: String,
    },
    Inspect {
        path: String,
        repair: Option<(String, String, String)>,
        disposable: Vec<String>,
        report_stored: bool,
    },
}
#[derive(Serialize, Deserialize)]
struct Input {
    build: String,
    request: Request,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "status")]
enum Reply<T> {
    Ready { result: T },
    Skew { build: String },
    Failed { detail: String },
}

pub(crate) fn script(machine: &MachineDeclaration) -> String {
    let bin = crate::remote::quote(&machine.ade_bin);
    let identity = crate::remote::quote(&format!(
        "herdr-ade {}.",
        crate::build::commit_version(crate::VERSION).unwrap()
    ));
    // The launch handshake also identifies binaries predating this endpoint,
    // without speaking their retired wire protocol. Same SSH trip, no fallback.
    crate::remote::with_path(
        &machine.path,
        &format!(
            "version=$({bin} --version) || exit $?; case \"$version\" in {identity}*) HERDR_ADE_BOX_INPUT=1 {bin} --root {} doctor ;; *) printf '{{\"status\":\"Skew\",\"build\":\"%s\"}}\\n' \"$version\" ;; esac",
            crate::remote::quote(&machine.root)
        ),
    )
}

pub(crate) fn call<T: DeserializeOwned>(
    runner: &dyn Runner,
    target: &str,
    machine: &MachineDeclaration,
    request: Request,
    timeout: Duration,
    control: Option<&Path>,
) -> Result<T> {
    let input = serde_json::to_string(&Input {
        build: crate::VERSION.into(),
        request,
    })?;
    let script = script(machine);
    let runner = crate::remote::BoxRunner(runner);
    let out = match control {
        Some(control) => {
            crate::remote::ssh_courier(&runner, target, control, &script, &input, timeout)?
        }
        None => crate::remote::ssh(&runner, target, &script, Some(&input), timeout)?,
    };
    decode(&out, &machine.label)
}

fn decode<T: DeserializeOwned>(out: &crate::runner::Output, target: &str) -> Result<T> {
    if let Some(reply) = out
        .stdout
        .lines()
        .rev()
        .find_map(|line| serde_json::from_str::<Reply<T>>(line).ok())
    {
        return match reply {
            Reply::Ready { result } => {
                anyhow::ensure!(out.success(), "unreachable: {}", out.error_text());
                Ok(result)
            }
            Reply::Skew { build } => bail!(
                "version_skew: {target} runs an older harness (build {build}); waiting for the box install (home build {})",
                crate::VERSION
            ),
            Reply::Failed { detail } => bail!("{detail}"),
        };
    }
    if out.success() {
        bail!(
            "protocol_unavailable: {target} returned no complete typed box report; check its installed harness build"
        );
    }
    let phase = out
        .stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter_map(|v| v["active"].as_str().map(str::to_string))
        .next_back();
    if out.timed_out {
        bail!(
            "slow: snapshot timed out while running {}; partial observations: {}",
            phase.as_deref().unwrap_or("unknown phase"),
            out.stdout.trim()
        );
    }
    bail!("unreachable: {}", out.error_text())
}

pub(crate) fn run(ctx: &Ctx) -> Result<()> {
    let input: Input = serde_json::from_reader(std::io::stdin().lock())?;
    crate::output::write_stdout(format_args!("{}\n", reply(ctx, input)?));
    Ok(())
}

fn reply(ctx: &Ctx, input: Input) -> Result<String> {
    let refusal = if !crate::build::same_commit(&input.build, crate::VERSION) {
        Reply::<()>::Skew {
            build: crate::VERSION.into(),
        }
    } else {
        match execute(ctx, input.request) {
            Ok(reply) => return Ok(reply),
            Err(error) => Reply::Failed {
                detail: format!("{error:#}"),
            },
        }
    };
    Ok(serde_json::to_string(&refusal)?)
}

fn ready<T: Serialize>(result: T) -> Result<String> {
    Ok(serde_json::to_string(&Reply::Ready { result })?)
}

pub(crate) fn local<T: DeserializeOwned>(ctx: &Ctx, request: Request) -> Result<T> {
    decode(
        &crate::runner::Output {
            code: Some(0),
            stdout: execute(ctx, request)?,
            ..Default::default()
        },
        "local",
    )
}

fn execute(ctx: &Ctx, request: Request) -> Result<String> {
    match request {
        Request::Doctor(plan) => ready(crate::doctor::execute_plan(ctx, &plan)?),
        Request::Courier { session, taken } => {
            ready(crate::steps::box_manifest(ctx, &session, &taken)?)
        }
        Request::Linked {
            root,
            relative,
            offset,
        } => {
            let linked = crate::threads::read_linked(&root, &relative, offset)?;
            if offset.is_some() {
                ready(linked.files.into_values().flatten().collect::<Vec<u8>>())
            } else {
                ready(Manifest {
                    directory: linked.directory,
                    files: linked
                        .files
                        .iter()
                        .map(|(path, bytes)| File {
                            path: path.clone(),
                            size: bytes.len() as u64,
                            hash: crate::thread::sha256_hex(bytes),
                        })
                        .collect(),
                })
            }
        }
        Request::Missing { root, relative } => {
            // Missing root is not evidence of a missing link in an accessible lane.
            std::fs::read_dir(&root)
                .with_context(|| format!("could not inspect {}", root.display()))?;
            let missing = match std::fs::symlink_metadata(root.join(relative)) {
                Ok(_) => false,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => true,
                Err(e) => return Err(e.into()),
            };
            ready(missing)
        }
        Request::RepoLink { root, relative } => ready(crate::threads::repo_link_hash(
            ctx.runner, &root, &relative,
        )?),
        Request::ForgetWorktree { repo, path } => {
            crate::git::forget_worktree(ctx.runner, &repo, &path)?;
            ready(())
        }
        Request::RemoveWorktree { repo, path } => {
            if Path::new(&path).exists() {
                crate::git::worktree_remove(ctx.runner, &repo, &path)?;
            }
            crate::git::forget_worktree(ctx.runner, &repo, &path)?;
            ready(())
        }
        Request::Inspect {
            path,
            repair,
            disposable,
            report_stored,
        } => {
            if let Some((repo, branch, sealed)) = repair {
                crate::git::repair_worktree(
                    ctx.runner,
                    &repo,
                    &path,
                    &branch,
                    if report_stored { &sealed } else { "" },
                )?;
            }
            ready(crate::worktrees::inspect_local(
                ctx.runner,
                &path,
                &path,
                &disposable,
                report_stored,
            )?)
        }
    }
}
pub(crate) const CHUNK: usize = 8 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) directory: bool,
    pub(crate) files: Vec<File>,
}
#[derive(Serialize, Deserialize)]
pub(crate) struct File {
    pub(crate) path: PathBuf,
    pub(crate) size: u64,
    pub(crate) hash: String,
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    #[test]
    fn skew_is_explicit_and_not_unreachable_or_a_machine_specific_stamp() {
        let out = crate::runner::fake::ok(
            &serde_json::to_string(&Reply::<()>::Skew {
                build: "0.1.0+old.1".into(),
            })
            .unwrap(),
        );
        let error = decode::<bool>(&out, "oci").unwrap_err().to_string();
        assert!(error.contains("oci runs an older harness (build 0.1.0+old.1)"));
        assert!(!crate::remote::is_unreachable(&error));
    }
    #[test]
    fn request_skew_answers_before_touching_files_and_equal_commit_ignores_stamp() {
        let world = crate::scenarios::World::new();
        let request = || Request::Missing {
            root: world.home.path().join("absent"),
            relative: "report.md".into(),
        };
        let answer = reply(
            &world.ctx(),
            Input {
                build: "0.1.0+old.1".into(),
                request: request(),
            },
        )
        .unwrap();
        assert!(matches!(
            serde_json::from_str::<Reply<()>>(&answer).unwrap(),
            Reply::Skew { .. }
        ));
        let build = format!(
            "{}.another-host-stamp",
            crate::build::commit_version(crate::VERSION).unwrap()
        );
        let answer = reply(
            &world.ctx(),
            Input {
                build,
                request: request(),
            },
        )
        .unwrap();
        assert!(matches!(
            serde_json::from_str::<Reply<()>>(&answer).unwrap(),
            Reply::Failed { .. }
        ));
    }

    #[test]
    fn launch_handshake_names_a_pre_helper_binary_instead_of_calling_old_protocol() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("old ' harness");
        std::fs::write(&bin, "#!/bin/sh\nif [ \"$1\" = --version ]; then echo herdr-ade 0.1.0+old.1; else exit 99; fi\n").unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        let machine = MachineDeclaration {
            path: "/bin:/usr/bin".into(),
            root: "/it's a $(box)".into(),
            ade_bin: bin.to_string_lossy().into_owned(),
            ..Default::default()
        };
        let out = crate::runner::RealRunner
            .run(
                &crate::runner::Cmd::new("sh", Duration::from_secs(10))
                    .args(["-c", &script(&machine)]),
            )
            .unwrap();
        assert!(out.success());
        let error = decode::<bool>(&out, "oci").unwrap_err().to_string();
        assert!(error.contains("oci runs an older harness"));
        assert!(!crate::remote::is_unreachable(&error));
    }

    #[test]
    fn batched_binary_chunks_survive_real_runner_retention_limit() {
        let dir = tempfile::tempdir().unwrap();
        let bytes: Vec<u8> = (0..2 * 1024 * 1024).map(|i| (i % 256) as u8).collect();
        let path = dir.path().join("reply.json");
        std::fs::write(&path, ready(&bytes)).unwrap();
        let out = crate::remote::BoxRunner(&crate::runner::RealRunner)
            .run(
                &crate::runner::Cmd::new("dd", Duration::from_secs(10))
                    .args([format!("if={}", path.display()), "status=none".into()]),
            )
            .unwrap();
        assert_eq!(decode::<Vec<u8>>(&out, "oci").unwrap(), bytes);
    }

    #[test]
    fn chunk_inventory_is_batched_sorted_and_missing_root_is_not_absent() {
        let world = crate::scenarios::World::new();
        let root = world.home.path().join("lane");
        std::fs::create_dir_all(root.join("library")).unwrap();
        let a = vec![0xff; CHUNK - 1];
        std::fs::write(root.join("library/a.bin"), &a).unwrap();
        std::fs::write(root.join("library/b.bin"), b"\0last").unwrap();
        let request = |offset| Request::Linked {
            root: root.clone(),
            relative: "library".into(),
            offset,
        };
        let manifest: Manifest = local(&world.ctx(), request(None)).unwrap();
        assert!(manifest.directory);
        assert_eq!(manifest.files.len(), 2);
        let first: Vec<u8> = local(&world.ctx(), request(Some(0))).unwrap();
        let last: Vec<u8> = local(&world.ctx(), request(Some(CHUNK as u64))).unwrap();
        assert_eq!(first.len(), CHUNK);
        assert_eq!(first.last(), Some(&0));
        assert_eq!(last, b"last");
        assert_eq!(manifest.files[0].hash, crate::thread::sha256_hex(&a));
        assert!(
            execute(
                &world.ctx(),
                Request::Missing {
                    root: root.join("gone"),
                    relative: "file".into()
                }
            )
            .is_err()
        );
    }

    pub(crate) fn ready<T: Serialize>(value: T) -> String {
        super::ready(value).unwrap()
    }
    pub(crate) fn doctor_input(input: &str) -> crate::doctor::ProbePlan {
        match serde_json::from_str::<Input>(input).unwrap().request {
            Request::Doctor(plan) => plan,
            _ => panic!("not a doctor request"),
        }
    }
    pub(crate) fn is_doctor(cmd: &crate::runner::Cmd) -> bool {
        cmd.stdin.as_deref().is_some_and(|input| {
            serde_json::from_str::<Input>(input)
                .is_ok_and(|input| matches!(input.request, Request::Doctor(_)))
        })
    }
    pub(crate) fn respond(ctx: &Ctx, input: &str) -> Result<crate::runner::Output> {
        let input: Input = serde_json::from_str(input)?;
        Ok(crate::runner::fake::ok(&reply(ctx, input)?))
    }
}
