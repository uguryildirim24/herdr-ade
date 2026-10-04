use super::*;
use crate::runner::fake::ok;
use crate::testkit::{fixture, git};

#[test]
fn d67_missing_admin_cleanup_removes_merged_work_and_keeps_unique_work() {
    for merged in [false, true] {
        let fx = fixture();
        let (id, sha) = fx.lane(1);
        fx.seal_done(&id, 1, 1, &sha, "sealed report\n");
        let record = thread::load(&fx.project, &id).unwrap();
        let pointer =
            std::fs::read_to_string(Path::new(&record.worktree_path).join(".git")).unwrap();
        let admin = pointer.trim().strip_prefix("gitdir: ").unwrap();
        std::fs::remove_dir_all(admin).unwrap();
        if merged {
            git(&fx.repo, &["merge", "--ff-only", &record.branch]);
        }
        let outcome = resolve(
            &fx.world.ctx(),
            "demo",
            &id,
            &ResolveArgs {
                skip_copy: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert!(!thread::load(&fx.project, &id).unwrap().cleanup_pending);
        if merged {
            assert_eq!(outcome.worktree, "removed");
            assert!(!Path::new(&record.worktree_path).exists());
        } else {
            assert_eq!(outcome.worktree, "kept");
            assert!(outcome.worktree_reason.unwrap().contains("work_not_done"));
            assert_eq!(git(&fx.repo, &["rev-parse", &record.branch]), sha);
            assert!(
                Path::new(&record.worktree_path)
                    .join("src/lane1.rs")
                    .exists()
            );
        }
    }
}

#[test]
fn d67_missing_admin_keeps_uncommitted_changes_and_requires_retained_seal() {
    for sealed in [false, true] {
        let fx = fixture();
        let (id, sha) = fx.lane(1);
        if sealed {
            fx.seal_done(&id, 1, 1, &sha, "report");
        }
        let record = thread::load(&fx.project, &id).unwrap();
        git(&fx.repo, &["merge", "--ff-only", &record.branch]);
        let pointer =
            std::fs::read_to_string(Path::new(&record.worktree_path).join(".git")).unwrap();
        std::fs::remove_dir_all(pointer.trim().strip_prefix("gitdir: ").unwrap()).unwrap();
        let file = Path::new(&record.worktree_path).join("README.md");
        std::fs::write(&file, "unique edit").unwrap();
        let result = resolve(
            &fx.world.ctx(),
            "demo",
            &id,
            &ResolveArgs {
                skip_copy: true,
                ..Default::default()
            },
        );
        assert!(result.is_err());
        let error = result.unwrap_err().to_string();
        assert!(
            error.contains(if sealed {
                "worktree_dirty"
            } else {
                "no sealed branch"
            }),
            "{error}"
        );
        assert_eq!(std::fs::read_to_string(file).unwrap(), "unique edit");
        assert_eq!(git(&fx.repo, &["rev-parse", &record.branch]), sha);
    }
}

#[test]
fn d67_resolved_cache_is_removed_after_worktree_inspection_errors() {
    let fx = fixture();
    let (mut settings, body) = fx.project.read_project_md().unwrap();
    settings.repos[0].box_path = Some("/box/repo".into());
    settings.repos[0].publish_url = Some("https://example.invalid/repo.git".into());
    std::fs::write(
        fx.project.project_md(),
        format!("+++\n{}+++\n\n{body}", toml::to_string(&settings).unwrap()),
    )
    .unwrap();
    fx.world.runner.on(
        "machine list --json",
        ok(r#"[{"id":"box-id","label":"box","target":"box","session":"default","enabled":true}]"#),
    );
    fx.world.runner.on_fn(
        |cmd| cmd.program == "ssh",
        |cmd| {
            Ok(ok(if cmd.stdin.is_some() {
                r#"{"status":"Failed","detail":"worktree metadata is missing"}"#
            } else if cmd.display().contains("__HERDR_WORKTREE_PRESENT__") {
                "__HERDR_WORKTREE_PRESENT__\n"
            } else {
                ""
            }))
        },
    );
    let record = thread::allocate(&fx.project, |t| {
        t.kind = Kind::Worktree;
        t.status = Status::Resolved;
        t.repo = fx.repo.to_string_lossy().into_owned();
        t.machine = "box".into();
        t.machine_id = "box-id".into();
        t.worktree_path = "/box/repo/.worktrees/t-0001".into();
        t.branch = "lane/box".into();
        t.merged_sha = "landed".into();
        t.retirement = Some(RetirementRequest {
            skip_copy: true,
            ..Default::default()
        });
        t.cleanup_pending = true;
    })
    .unwrap();
    let outcome = resolve_automatically(&fx.world.ctx(), &fx.project, &record.id, "merged");
    assert_eq!(outcome.state, "cleanup_pending");
    let saved = thread::load(&fx.project, &record.id).unwrap();
    assert_eq!(saved.worktree_path, record.worktree_path);
    assert!(saved.cleanup_reason.contains("metadata is missing"));
    assert_eq!(
        fx.world
            .runner
            .count("rm -rf -- /home/agent/build/lanes/demo-t-0001"),
        1
    );
    assert_eq!(fx.world.runner.count("worktree remove"), 0);
}
