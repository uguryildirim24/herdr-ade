use super::*;

fn four_criteria(fx: &Fx, id: &str) -> crate::task::Task {
    let mut record = job(fx, id);
    record.acceptance = (1..=4).map(|n| format!("Original condition {n}")).collect();
    std::fs::write(
        fx.project
            .record_dir_for_write("tasks")
            .unwrap()
            .join(format!("{}.toml", record.id)),
        toml::to_string(&record).unwrap(),
    )
    .unwrap();
    let original = format!("# Frozen brief\n{}\n", record.acceptance.join("\n"));
    let hash = thread::store_artifact(&fx.project, original.as_bytes()).unwrap();
    thread::update(&fx.project, id, |t| t.launch.brief_hash = hash).unwrap();
    record
}

fn withdraw_second(fx: &Fx, record: &crate::task::Task) -> crate::task::Task {
    crate::task::withdraw_acceptance(
        &fx.project,
        &record.id,
        vec![2],
        "No longer part of the requested outcome",
    )
    .unwrap()
}

fn rows(review: &Review, record: &crate::task::Task, failed: &[usize]) -> String {
    (1..=4)
        .map(|n| {
            criterion_reason(
                &review.members[0].thread,
                &review.members[0].event,
                &record.acceptance[n - 1],
                !failed.contains(&n),
            )
            .replace("criterion = 1\n", &format!("criterion = {n}\n"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn seal_rows(fx: &Fx, review: &Review, word: &str, rows: &str) {
    let candidate = candidate(fx, review);
    fx.seal_done(
        review.reviewer.as_deref().unwrap(),
        1,
        1,
        &candidate,
        &format!(
            "+++\nreview = {:?}\nverdict = {word:?}\ncandidate = {candidate:?}\n{rows}\n+++\nCriterion judgments\n",
            review.id
        ),
    );
}

#[test]
fn packet_lists_withdrawals_without_rewriting_frozen_brief_or_numbers() {
    let fx = configured();
    let (id, _) = lane(&fx, 1);
    let record = four_criteria(&fx, &id);
    let original_hash = thread::load(&fx.project, &id).unwrap().launch.brief_hash;
    let original = thread::artifact(&fx.project, &original_hash).unwrap();
    let record = withdraw_second(&fx, &record);
    let review = prepared(&fx);
    let packet = task(&fx.project, &review);
    assert!(packet.contains(std::str::from_utf8(&original).unwrap()));
    assert!(packet.contains(
        "Withdrawn criterion 2: Original condition 2 — NOT REQUIRED; must not be judged."
    ));
    assert!(packet.contains(&record.withdrawn[0].reason));
    assert!(packet.contains(&record.withdrawn[0].at));
    assert!(packet.contains("via coordinator `ha task drop`"));
    assert!(packet.contains("individual identity not recorded"));
    assert!(packet.contains("Required acceptance rows for job-0001: 1, 3, 4"));
    assert!(packet.contains("# Repeat for each included task's required criterion not withdrawn:"));
    assert!(packet.contains(
        "any acceptance row for a withdrawn criterion is ignored, never a reason to reject"
    ));
    assert!(packet.find("### End original").unwrap() < packet.find("Withdrawn criterion").unwrap());
    assert_eq!(
        thread::artifact(&fx.project, &original_hash).unwrap(),
        original
    );
    assert_eq!(
        crate::task::withdraw_acceptance(&fx.project, &record.id, vec![1, 3, 4], "all removed")
            .unwrap_err()
            .to_string(),
        "task_drop_acceptance_all: this would withdraw every acceptance condition; drop the task instead"
    );
}

#[test]
fn withdrawn_only_reject_starts_fresh_review_of_same_seals() {
    for explicit_retry in [false, true] {
        let fx = configured();
        let (id, _) = lane(&fx, 1);
        let record = four_criteria(&fx, &id);
        let mut review = prepared(&fx);
        // The withdrawal arrives after the old packet was frozen, matching
        // the production failure. The unchanged member seal remains valid.
        assert!(!task(&fx.project, &review).contains("Withdrawn criterion"));
        let record = withdraw_second(&fx, &record);
        seal_rows(&fx, &review, "REJECT", &rows(&review, &record, &[2]));
        advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
        assert!(review.verdict.as_ref().unwrap().withdrawn_only);
        assert!(!review.verdict.as_ref().unwrap().evidence_only);
        assert!(review.notices[0].line.contains("not a failed review; starting a fresh review of the same members with the corrected packet"));
        let member = thread::load(&fx.project, &id).unwrap();
        assert!(member.review_after.is_empty());
        assert!(member.review_reason.is_empty());
        assert_eq!(git(&fx.repo, &["rev-parse", "main"]), review.base);
        assert!(
            load(&fx.project, &review.id)
                .unwrap()
                .verdict
                .unwrap()
                .withdrawn_only
        );
        let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
        assert_eq!(reviewer.status, Status::Resolved);
        let next_reviewer = fx.thread("fresh reviewer");
        thread::update(&fx.project, &next_reviewer, |t| {
            t.role = "reviewer".into();
            t.review_id = "review-2".into();
            t.pane_id.clear();
        })
        .unwrap();
        let fresh = if explicit_retry {
            retry(&fx.world.ctx(), "demo", None).unwrap().unwrap()
        } else {
            tick(&fx.world.ctx(), &fx.project).unwrap();
            load(&fx.project, "review-2").unwrap()
        };
        assert_eq!(fresh.phase, Phase::Reviewing);
        assert_ne!(fresh.reviewer, review.reviewer);
        assert_eq!(fresh.members[0].event, review.members[0].event);
        assert_eq!(fresh.members[0].sha, review.members[0].sha);
        let packet = task(&fx.project, &fresh);
        assert!(packet.contains("Withdrawn criterion 2"));
        assert!(packet.contains("Required acceptance rows for job-0001: 1, 3, 4"));
    }
}

#[test]
fn required_or_mixed_failures_still_reject_members() {
    for failed in [vec![3], vec![2, 3], vec![]] {
        let fx = configured();
        let (id, _) = lane(&fx, 1);
        let record = withdraw_second(&fx, &four_criteria(&fx, &id));
        let mut review = prepared(&fx);
        // Even a report asserting withdrawn_only cannot bypass classification.
        let rows = format!("withdrawn_only = true\n{}", rows(&review, &record, &failed));
        seal_rows(&fx, &review, "REJECT", &rows);
        advance(&fx.world.ctx(), &fx.project, &mut review).unwrap();
        assert_eq!(review.phase, Phase::Rejected);
        assert!(!review.verdict.as_ref().unwrap().withdrawn_only);
        assert_eq!(
            thread::load(&fx.project, &id).unwrap().review_after,
            review.members[0].event
        );
        assert!(retry(&fx.world.ctx(), "demo", None).is_err());
        assert!(start(&fx.world.ctx(), "demo", None).unwrap().is_none());
    }
}

#[test]
fn withdrawal_classification_requires_exact_member_seal_and_condition() {
    let fx = configured();
    let (id, _) = lane(&fx, 1);
    let record = withdraw_second(&fx, &four_criteria(&fx, &id));
    let review = prepared(&fx);
    let report = format!("+++\n{}\n+++\n", rows(&review, &record, &[2]));
    let criteria = crate::task::report_criteria(&report).unwrap();
    assert!(rejection_only_withdrawn(&fx.project, &review, &criteria));
    for field in ["thread", "event", "criterion", "condition"] {
        let mut criteria = criteria.clone();
        let failed = &mut criteria[1];
        match field {
            "thread" => failed.thread = "t-unknown".into(),
            "event" => failed.event = "old-seal".into(),
            "criterion" => failed.criterion = 0,
            "condition" => failed.condition = "rewritten intent".into(),
            _ => unreachable!(),
        }
        assert!(!rejection_only_withdrawn(&fx.project, &review, &criteria));
    }
}

#[test]
fn merge_ignores_withdrawn_rows_but_requires_all_remaining_evidence() {
    for omit_withdrawn in [false, true] {
        let fx = configured();
        let (id, _) = lane(&fx, 1);
        let record = withdraw_second(&fx, &four_criteria(&fx, &id));
        let review = prepared(&fx);
        let acceptance = if omit_withdrawn {
            [1, 3, 4]
                .map(|n| {
                    criterion_reason(
                        &id,
                        &review.members[0].event,
                        &record.acceptance[n - 1],
                        true,
                    )
                    .replace("criterion = 1\n", &format!("criterion = {n}\n"))
                })
                .join("\n")
        } else {
            rows(&review, &record, &[2])
        };
        seal_rows(&fx, &review, "MERGE", &acceptance);
        let events = crate::events::checked(&fx.project).unwrap();
        let reviewer = thread::load(&fx.project, review.reviewer.as_deref().unwrap()).unwrap();
        assert!(
            verdict(
                &fx.world.ctx(),
                &fx.project,
                &review,
                sealed(&events, &reviewer).unwrap(),
                &Git::new(&fx.world.runner, fx.repo.to_str().unwrap()),
            )
            .is_ok()
        );
        let criteria = crate::task::report_criteria(&format!("+++\n{acceptance}\n+++\n")).unwrap();
        assert!(crate::task::criteria_established(
            &record,
            &id,
            &review.members[0].event,
            &criteria
        ));
        for missing in [1, 3, 4] {
            let mut incomplete = criteria.clone();
            incomplete.retain(|row| row.criterion != missing);
            assert!(!crate::task::criteria_established(
                &record,
                &id,
                &review.members[0].event,
                &incomplete
            ));
        }
    }
}
