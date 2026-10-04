use super::*;
use crate::runner::fake::ok;

fn attachment_fixture(remote: bool) -> Fx {
    let fx = configured();
    fx.world.runner.on("tab rename", ok(r#"{"result":{}}"#));
    fx.world.runner.on(
        "tab create",
        ok(r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t3","pane_id":"w1:p3"}}}"#),
    );
    if remote {
        let remote = fx.world.home.path().join("published.git");
        git(
            &fx.repo,
            &["init", "--bare", "-q", remote.to_str().unwrap()],
        );
        git(
            &fx.repo,
            &["remote", "add", "box", remote.to_str().unwrap()],
        );
        let (mut settings, body) = fx.project.read_project_md().unwrap();
        settings.repos[0].box_path = Some("/box/repo".into());
        settings.repos[0].publish_url = Some(remote.to_string_lossy().into_owned());
        std::fs::write(
            fx.project.project_md(),
            format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
        )
        .unwrap();
        let config = fx.world.ctx().config_dir.join("config.toml");
        let mut text = std::fs::read_to_string(&config).unwrap();
        text.push_str("\n[dispatch]\nmachine = 'box'\n");
        std::fs::write(config, text).unwrap();
        fx.world.runner.on(
            "machine list --json",
            ok(r#"[{"id":"box-id","label":"box","target":"box","session":"default","enabled":true}]"#),
        );
        fx.world.runner.on(
            "workspace list",
            ok(r#"{"result":{"workspaces":[{"workspace_id":"w1","label":"Demo"}]}}"#),
        );
        fx.world.runner.on_fn(
            |cmd| cmd.program == "ssh",
            |cmd| {
                if crate::box_helper::tests::is_doctor(cmd) {
                    return Ok(crate::testkit::diagnostic_output(cmd, 99_999_999, None));
                }
                let script = cmd.args.last().unwrap();
                let base = script
                    .split("FETCH_HEAD)\" = ")
                    .nth(1)
                    .and_then(|rest| rest.split_whitespace().next())
                    .unwrap_or("");
                Ok(ok(&format!("{base}\n")))
            },
        );
    }
    fx
}

/// Start through the ordinary --attach path, then seal a real Git change.
fn attached_member(fx: &Fx, n: u32, bytes: &[u8]) -> (Thread, String) {
    let dir = fx.world.home.path().join(format!("input-{n}"));
    std::fs::create_dir_all(&dir).unwrap();
    let source = dir.join("same name.txt");
    std::fs::write(&source, bytes).unwrap();
    let lane = crate::threads::start(
        &fx.world.ctx(),
        "demo",
        crate::threads::StartArgs {
            title: format!("Attached member {n}"),
            repo: Some(fx.repo.to_string_lossy().into_owned()),
            machine: Some("local".into()),
            base: Some("main".into()),
            task: "Use the attached requirements.".into(),
            attach: vec![source.to_string_lossy().into_owned()],
            paths: vec![],
            workflow: None,
            recipe: None,
            task_id: String::new(),
            review_id: String::new(),
        },
    )
    .unwrap();
    let hash = lane.attachments["same name.txt"].clone();
    crate::threads::place_recovery(&fx.world.ctx(), &fx.project, &lane).unwrap();
    let lane = thread::load(&fx.project, &lane.id).unwrap();
    let sha = commit_file(
        Path::new(&lane.worktree_path),
        &format!("member-{n}.txt"),
        "Member result\n",
        "member result",
    );
    thread::update(&fx.project, &lane.id, |t| {
        t.partial = None;
        t.recovery_pending = false;
        t.status = Status::Open;
    })
    .unwrap();
    fx.seal_done(&lane.id, 1, 1, &sha, "Attached requirements used.\n");
    std::fs::remove_file(source).unwrap(); // The original host path is gone.
    (lane, hash)
}

#[test]
fn pile_attachment_launch_stages_same_names_by_hash_and_deduplicates() {
    for remote in [false, true] {
        let fx = attachment_fixture(remote);
        let (a, ha) = attached_member(&fx, 1, b"first member bytes\n");
        let (b, hb) = attached_member(&fx, 2, b"second member bytes\n");
        let (c, hc) = attached_member(&fx, 3, b"first member bytes\n");
        assert_ne!(ha, hb);
        assert_eq!(ha, hc);
        let review = start(&fx.world.ctx(), "demo", None).unwrap().unwrap();
        let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
        assert_eq!(reviewer.is_remote(), remote);
        assert_eq!(
            reviewer.attachments,
            BTreeMap::from([(ha.clone(), ha.clone()), (hb.clone(), hb.clone())])
        );
        let brief =
            String::from_utf8(thread::artifact(&fx.project, &reviewer.launch.brief_hash).unwrap())
                .unwrap();
        for (member, hash) in [(&a, &ha), (&b, &hb), (&c, &hc)] {
            let packet = member_packet(
                &fx.project,
                review
                    .members
                    .iter()
                    .find(|m| m.thread == member.id)
                    .unwrap(),
            );
            assert!(
                packet.contains(&format!(
                    "name `same name.txt`, hash `{hash}`, reviewer-side path `attachments/{hash}`"
                )),
                "{packet}"
            );
            assert!(brief.contains(&format!("{}/attachments/{hash}", reviewer.thread_dir)));
        }
        let before = fx.world.runner.calls.borrow().len();
        crate::threads::place_recovery(&fx.world.ctx(), &fx.project, &reviewer).unwrap();
        for (hash, bytes) in [
            (&ha, b"first member bytes\n".as_slice()),
            (&hb, b"second member bytes\n".as_slice()),
        ] {
            let path = format!("{}/attachments/{hash}", reviewer.thread_dir);
            if remote {
                let encoded: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
                let calls = fx.world.runner.calls.borrow();
                let writes: Vec<_> = calls[before..]
                    .iter()
                    .filter(|call| {
                        call.program == "ssh"
                            && call.stdin.as_deref() == Some(&encoded)
                            && call.args.last().unwrap().contains(&path)
                    })
                    .collect();
                assert_eq!(writes.len(), 1);
                assert!(writes[0].args.last().unwrap().contains(hash));
                assert!(writes[0].args.last().unwrap().contains("sha256"));
            } else {
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
                assert_eq!(thread::sha256_hex(&std::fs::read(&path).unwrap()), *hash);
            }
        }
    }
}

#[test]
fn missing_or_corrupt_member_attachment_is_named_not_silently_omitted() {
    let fx = attachment_fixture(false);
    let (a, ha) = attached_member(&fx, 1, b"missing member bytes\n");
    let (b, hb) = attached_member(&fx, 2, b"corrupt member bytes\n");
    std::fs::remove_file(crate::events::artifact_path(&fx.project, &ha)).unwrap();
    std::fs::write(
        crate::events::artifact_path(&fx.project, &hb),
        b"wrong bytes",
    )
    .unwrap();
    let review = start(&fx.world.ctx(), "demo", None).unwrap().unwrap();
    let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
    assert!(reviewer.attachments.is_empty());
    let brief =
        String::from_utf8(thread::artifact(&fx.project, &reviewer.launch.brief_hash).unwrap())
            .unwrap();
    for (lane, hash, error) in [
        (&a, &ha, "brief_artifact_missing"),
        (&b, &hb, "brief_artifact_mismatch"),
    ] {
        let packet = member_packet(
            &fx.project,
            review.members.iter().find(|m| m.thread == lane.id).unwrap(),
        );
        assert!(
            packet.contains(&format!(
                "name `same name.txt`, hash `{hash}` — unavailable"
            )),
            "{packet}"
        );
        assert!(packet.contains(error));
        assert!(brief.contains(&packet));
    }
}

/// Opt-in live transport proof on oci. The scratch SSH endpoint uses freshly
/// generated keys; no production credentials or projects are touched. Lifecycle
/// observations stay scripted (no model turn); Git, sealing, SSH provisioning,
/// hash verification and the reviewer-side reader are real.
#[test]
#[ignore = "requires ADE_D35_PROOF scratch SSH endpoint; see lane live-proof.sh"]
fn oci_live_attachment_transport() {
    use crate::runner::{Cmd, RealRunner, Runner};
    use std::time::Duration;

    let proof = PathBuf::from(std::env::var("ADE_D35_PROOF").unwrap());
    let fx = attachment_fixture(true);
    let (member, hash) = attached_member(&fx, 1, b"D35 scratch requirements on oci\n");
    let published = fx.world.home.path().join("published.git");
    git(&fx.repo, &["push", "box", "main"]);
    let box_repo = proof.join("box-repo");
    git(
        &fx.repo,
        &[
            "clone",
            "-q",
            published.to_str().unwrap(),
            box_repo.to_str().unwrap(),
        ],
    );
    let (mut settings, body) = fx.project.read_project_md().unwrap();
    settings.repos[0].box_path = Some(box_repo.to_string_lossy().into_owned());
    std::fs::write(
        fx.project.project_md(),
        format!("+++\n{}+++\n{body}", toml::to_string(&settings).unwrap()),
    )
    .unwrap();
    let config = fx.world.ctx().config_dir.join("config.toml");
    let text = std::fs::read_to_string(&config)
        .unwrap()
        .replace(
            "/home/agent/.herdr-ade",
            &proof.join("box-root").to_string_lossy(),
        )
        .replace("target = \"box\"", "target = \"ubuntu@127.0.0.1\"");
    std::fs::write(config, text).unwrap();
    struct LiveTransport<'a> {
        fake: &'a crate::runner::fake::FakeRunner,
        endpoint: PathBuf,
    }
    impl Runner for LiveTransport<'_> {
        fn run(&self, cmd: &Cmd) -> Result<crate::runner::Output> {
            if cmd.display().contains("machine list --json") {
                return Ok(ok(
                    r#"[{"id":"box-id","label":"box","target":"ubuntu@127.0.0.1","session":"default","enabled":true}]"#,
                ));
            }
            if cmd.program != "ssh" || crate::box_helper::tests::is_doctor(cmd) {
                return self.fake.run(cmd);
            }
            let mut live = cmd.clone();
            live.args.splice(
                0..0,
                [
                    "-p".into(),
                    "22798".into(),
                    "-i".into(),
                    self.endpoint.join("client").to_string_lossy().into_owned(),
                    "-o".into(),
                    format!(
                        "UserKnownHostsFile={}",
                        self.endpoint.join("known_hosts").display()
                    ),
                    "-o".into(),
                    "StrictHostKeyChecking=accept-new".into(),
                ],
            );
            let result = RealRunner.run(&live)?;
            assert!(result.success(), "{}", result.error_text());
            Ok(result)
        }
    }
    let live = LiveTransport {
        fake: &fx.world.runner,
        endpoint: proof.clone(),
    };
    let mut ctx = fx.world.ctx();
    ctx.runner = &live;
    let review = start(&ctx, "demo", None).unwrap().unwrap();
    let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
    assert!(reviewer.is_remote());
    crate::threads::place_recovery(&ctx, &fx.project, &reviewer).unwrap();
    let path = format!("{}/attachments/{hash}", reviewer.thread_dir);
    // An independent process on the SSH target opens the staged reviewer path.
    let script = format!(
        "python3 -c {} {} {}",
        crate::remote::quote(
            "import hashlib,pathlib,sys; p=pathlib.Path(sys.argv[1]); b=p.read_bytes(); assert hashlib.sha256(b).hexdigest()==sys.argv[2]; print(p); print(b.decode(),end=''); print('sha256='+hashlib.sha256(b).hexdigest())"
        ),
        crate::remote::quote(&path),
        hash
    );
    let out = live
        .run(&Cmd::new("ssh", Duration::from_secs(20)).args(["ubuntu@127.0.0.1", &script]))
        .unwrap();
    assert!(out.success());
    assert!(out.stdout.contains("D35 scratch requirements on oci"));
    let receipt = format!(
        "scratch home: {}\nmember: {}\nseal: {}-1-1\nreview: {}\nreviewer: {}\n{}",
        fx.world.home.path().display(),
        member.id,
        member.id,
        review.id,
        reviewer.id,
        out.stdout
    );
    std::fs::write(proof.join("receipt.txt"), &receipt).unwrap();
    println!("{receipt}");
    // Retain only this opt-in scratch root for inspection of the seal and launch.
    drop(live);
    std::mem::forget(fx);
}

#[test]
fn retry_adds_member_attachments_to_a_historical_reviewer() {
    let fx = attachment_fixture(false);
    let (_member, hash) = attached_member(&fx, 1, b"retry member bytes\n");
    let review = prepared(&fx); // Recover an old allocation without attachments.
    let id = review.reviewer.as_deref().unwrap();
    assert!(
        thread::load(&fx.project, id)
            .unwrap()
            .attachments
            .is_empty()
    );
    let checkout = thread::load(&fx.project, id).unwrap().worktree_path;
    thread::update(&fx.project, id, |t| {
        t.status = Status::Failed;
        t.error = "interrupted reviewer".into();
    })
    .unwrap();
    retry(&fx.world.ctx(), "demo", None).unwrap();
    let reviewer = thread::load(&fx.project, id).unwrap();
    assert_eq!(reviewer.worktree_path, checkout);
    assert_eq!(reviewer.attachments[&hash], hash);
    let brief =
        String::from_utf8(thread::artifact(&fx.project, &reviewer.launch.brief_hash).unwrap())
            .unwrap();
    assert!(brief.contains(&format!("attachments/{hash}")));
    crate::threads::place_recovery(&fx.world.ctx(), &fx.project, &reviewer).unwrap();
    assert_eq!(
        std::fs::read(format!("{}/attachments/{hash}", reviewer.thread_dir)).unwrap(),
        b"retry member bytes\n"
    );
}
