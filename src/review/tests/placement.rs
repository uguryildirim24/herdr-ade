use super::*;
use crate::runner::fake::ok;

fn placement_fixture(machine: Option<&str>, kinds: &str) -> Fx {
    let fx = configured();
    let config = fx.world.ctx().config_dir.join("config.toml");
    let mut text = std::fs::read_to_string(&config).unwrap();
    text.push_str("\n[dispatch]\nmachine = 'buildbox'\n");
    text.push_str(
        &crate::remote::TEST_MACHINE.replace("kinds = [\"pi\"]", &format!("kinds = {kinds}")),
    );
    std::fs::write(config, text).unwrap();
    let (mut settings, body) = fx.project.read_project_md().unwrap();
    settings.repos[0].review_machine = machine.map(str::to_string);
    settings.repos[0].box_path = Some("/home/agent/projects/repo".into());
    settings.repos[0].publish_url = Some("https://example.test/repo.git".into());
    git(
        &fx.repo,
        &["remote", "add", "origin", "https://example.test/repo.git"],
    );
    std::fs::write(
        fx.project.project_md(),
        format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
    )
    .unwrap();
    fx.world.runner.on("machine list --json", ok(r#"[{"id":"buildbox-id","label":"buildbox","target":"buildbox-pi","session":"default","enabled":true}]"#));
    fx.world.runner.on_fn(
        |cmd| cmd.program == "ssh",
        |cmd| {
            Ok(crate::doctor::boundary_diagnostic_output(
                cmd, 99_999_999, None,
            ))
        },
    );
    lane(&fx, 1);
    fx
}

#[test]
fn review_machine_local_overrides_ready_dispatch_and_packet_names_machine() {
    let fx = placement_fixture(Some("local"), "[\"pi\", \"claude\"]");
    let before = crate::plan::counts(&fx.project).unwrap();
    let review = start(&fx.world.ctx(), "demo", None).unwrap().unwrap();
    let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
    assert!(reviewer.machine.is_empty());
    assert_eq!(reviewer.launch.machine, "local");
    thread::update(&fx.project, &reviewer.id, |t| {
        t.worktree_path = fx.repo.to_string_lossy().into_owned();
    })
    .unwrap();
    let gates = gate_context(&fx.world.ctx(), &fx.project, &review).unwrap();
    assert_eq!(gates.machine, "local");
    assert!(gates.target.is_none());
    assert_eq!(review.review_machine.as_deref(), Some("local"));
    assert_eq!(
        load(&fx.project, &review.id)
            .unwrap()
            .review_machine
            .as_deref(),
        Some("local")
    );
    let packet =
        String::from_utf8(thread::artifact(&fx.project, &reviewer.launch.brief_hash).unwrap())
            .unwrap();
    assert!(packet.contains("Reviewer machine: `local`"), "{packet}");
    assert!(packet.contains("- Machine: local."), "{packet}");
    assert!(packet.contains("Gates run here too"));
    assert_eq!(before, crate::plan::counts(&fx.project).unwrap());
    assert_eq!(fx.world.runner.count("ssh"), 0);
    let context = crate::project_view::View::load(&fx.world.ctx(), &fx.project, None)
        .unwrap()
        .render(&[]);
    assert!(
        context.contains("review machine: local (explicit; no fallback)"),
        "{context}"
    );
}

#[test]
fn review_machine_unset_keeps_box_first_and_records_effective_machine() {
    let fx = placement_fixture(None, "[\"pi\", \"claude\"]");
    let review = start(&fx.world.ctx(), "demo", None).unwrap().unwrap();
    let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
    assert_eq!(reviewer.machine, "buildbox");
    thread::update(&fx.project, &reviewer.id, |t| {
        t.worktree_path = "/home/agent/projects/repo/.worktrees/reviewer".into();
    })
    .unwrap();
    let gates = gate_context(&fx.world.ctx(), &fx.project, &review).unwrap();
    assert_eq!(gates.machine, "buildbox-id");
    assert_eq!(gates.target.as_deref(), Some("buildbox-pi"));
    assert_eq!(review.review_machine.as_deref(), Some("buildbox"));
    let packet =
        String::from_utf8(thread::artifact(&fx.project, &reviewer.launch.brief_hash).unwrap())
            .unwrap();
    assert!(packet.contains("- Machine: buildbox."), "{packet}");
    let context = crate::project_view::View::load(&fx.world.ctx(), &fx.project, None)
        .unwrap()
        .render(&[]);
    assert!(
        context.contains("review machine: buildbox first, local if unavailable (default)"),
        "{context}"
    );
}

#[test]
fn review_machine_unready_named_machine_never_falls_back() {
    let fx = placement_fixture(Some("buildbox"), "[]");
    let error = start(&fx.world.ctx(), "demo", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("recipe_unavailable"), "{error}");
    assert!(error.contains("buildbox"), "{error}");
    assert!(!error.contains("local:"), "{error}");
    assert!(
        thread::list(&fx.project)
            .iter()
            .all(|t| t.role != "reviewer")
    );
}

#[test]
fn review_machine_unknown_label_refuses_start() {
    let fx = placement_fixture(Some("missing-box"), "[\"pi\", \"claude\"]");
    let error = start(&fx.world.ctx(), "demo", None)
        .unwrap_err()
        .to_string();
    assert!(error.contains("missing-box"), "{error}");
    assert!(error.contains("recipe_unavailable"), "{error}");
    assert!(
        thread::list(&fx.project)
            .iter()
            .all(|t| t.role != "reviewer")
    );
}
