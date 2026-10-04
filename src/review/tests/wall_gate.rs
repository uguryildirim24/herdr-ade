use super::*;

#[test]
fn wall_selection_cannot_be_narrowed_away_for_harness_paths() {
    let wall = project::Gate {
        command: "tools/wall/gate".into(),
        paths: Some(vec!["docs/**".into()]),
        ..Default::default()
    };
    for file in [
        "src/review.rs",
        "assets/hooks.js",
        "mods/x.rs",
        "tools/wall/gate",
    ] {
        assert_eq!(
            selected(std::slice::from_ref(&wall), &[file.into()]),
            vec![wall.clone()]
        );
    }
    assert!(selected(std::slice::from_ref(&wall), &["skill/LANE.md".into()]).is_empty());
    // It is not implicitly installed into repositories without a declaration.
    assert!(selected(&[], &["src/main.rs".into()]).is_empty());
}

#[test]
fn wall_reuses_the_reviewer_target_without_changing_other_gate_environments() {
    let mut gate = project::Gate {
        command: "tools/wall/gate".into(),
        ..Default::default()
    };
    let env = gate_environment(BTreeMap::new(), Some("/build/demo-t-0003"), &gate);
    assert_eq!(env.get("CARGO_TARGET_DIR").unwrap(), "/build/demo-t-0003");
    gate.command = "cargo test".into();
    assert!(gate_environment(BTreeMap::new(), Some("/build/demo-t-0003"), &gate).is_empty());
}

#[test]
fn wall_result_is_preserved_in_review_notice_and_busy_is_typed_retryable() {
    for (exit, line) in [
        (0, "WALL GATE PASS"),
        (1, "WALL GATE FAIL: regression D30: lane omitted"),
        (
            75,
            "WALL GATE INCOMPLETE: gate instance busy (retryable; exit 75)",
        ),
    ] {
        let fx = configured();
        lane(&fx, 1);
        let mut review = prepared(&fx);
        review.gates = vec![project::Gate {
            command: "tools/wall/gate".into(),
            ..Default::default()
        }];
        let candidate = candidate(&fx, &review);
        fx.world.runner.on_fn(
            |cmd| cmd.program == "sh",
            move |_| {
                Ok(crate::runner::Output {
                    code: Some(exit),
                    stdout: format!("Evidence: /tmp/proof\n{line}\n"),
                    stderr: String::new(),
                    timed_out: false,
                })
            },
        );
        let git = Git::new(&fx.world.runner, &review.repo);
        let mut note = String::new();
        let result = observed_gates(
            &fx.world.ctx(),
            &fx.project,
            &review,
            &candidate,
            &git,
            &mut note,
        );
        match exit {
            0 => {
                result.unwrap();
                let mut verdict = verdict_record(&review, &candidate);
                verdict.gates_note = note;
                review.verdict = Some(verdict);
                assert!(review.gates_summary().contains(line));
            }
            75 => assert!(result.unwrap_err().downcast_ref::<WallGateBusy>().is_some()),
            _ => assert!(result.unwrap_err().to_string().contains(line)),
        }
    }
}

#[test]
fn busy_retries_the_same_seal_without_coordinator_or_new_verdict() {
    let fx = configured();
    lane(&fx, 1);
    let mut review = prepared(&fx);
    review.gates = vec![project::Gate {
        command: "tools/wall/gate".into(),
        ..Default::default()
    }];
    let candidate = candidate(&fx, &review);
    seal_verdict(
        &fx,
        &review,
        &candidate,
        "MERGE",
        BTreeMap::new(),
        vec![],
        1,
    );
    let busy = std::rc::Rc::new(std::cell::Cell::new(true));
    let observed = busy.clone();
    fx.world.runner.on_fn(
        |cmd| cmd.program == "sh",
        move |_| {
            Ok(crate::runner::Output {
                code: Some(if observed.get() { 75 } else { 0 }),
                stdout: if observed.get() {
                    "WALL GATE INCOMPLETE: gate instance busy (retryable; exit 75)\n".into()
                } else {
                    "WALL GATE PASS\n".into()
                },
                stderr: String::new(),
                timed_out: false,
            })
        },
    );
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.phase, Phase::Reviewing);
    assert!(review.checked_event.is_empty() && review.attention.is_empty());
    assert!(review.verdict.is_none() && !review.fast_forward);
    assert!(
        review
            .notices
            .iter()
            .any(|n| n.line.contains("gate instance busy"))
    );
    busy.set(false);
    advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
    assert_eq!(review.phase, Phase::Complete);
    assert!(
        review
            .notices
            .iter()
            .any(|n| n.line.contains("WALL GATE PASS"))
    );
}

fn verdict_record(review: &Review, candidate: &str) -> Verdict {
    toml::from_str(&format!(
        "review = {:?}\nverdict = 'MERGE'\ncandidate = {:?}\n",
        review.id, candidate
    ))
    .unwrap()
}
