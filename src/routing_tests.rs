//! Offline dispatch, policy replay and durable escalation gates. Never calls Jev.
use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::contracts::{
    AdmissionManifest, DonePayload, Event, EventPayload, Launch, ManifestMember, MergeIntent,
    MergePhase, RoundPhase, RoundRecord, WaitingPayload,
};
use crate::launch::{self, ResolveInput};
use crate::paths::Ctx;
use crate::runner::fake::{FakeRunner, fail, ok, timeout};
use crate::scenarios::World;
use crate::{escalation, events, jev, routing, thread};

fn response(level: f64, confidence: f64) -> Value {
    let mut answers = BTreeMap::new();
    let lower = level.floor() as usize;
    let upper = level.ceil() as usize;
    let mut probabilities: BTreeMap<String, f64> = (0..4).map(|i| (i.to_string(), 0.0)).collect();
    probabilities.insert(lower.to_string(), 1.0 - level.fract());
    if upper != lower {
        probabilities.insert(upper.to_string(), level.fract());
    }
    answers.insert("required_index", json!({"type":"score", "score":level, "confidence":confidence, "probabilities":probabilities}));
    json!({"model":"jev-test", "answers":answers, "usage":{"input_tokens":70,"output_tokens":30}})
}

fn setup() -> (World, crate::project::Project) {
    let world = World::new();
    let cfg = world.home.path().join("cfg");
    std::fs::write(cfg.join("config.toml"), "[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n[recipes.test_strong]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the careful helper\"\n").unwrap();
    let mut policy: Value = serde_json::from_str(include_str!("../config/routing.json")).unwrap();
    policy["models"] = json!({
        "test_claude":{"tier":1,"coding_index":76.2,"price_per_million":1.0,"description":"cheap"},
        "test_strong":{"tier":2,"coding_index":77.2,"price_per_million":2.0,"description":"strong"}
    });
    policy["role_floors"] = json!({});
    policy["answer_floors"] = json!([]);
    std::fs::write(cfg.join("routing.json"), policy.to_string()).unwrap();
    let project = world.project("demo", "a.sock");
    (world, project)
}

fn runner(level: f64, confidence: f64) -> FakeRunner {
    let runner = FakeRunner::new();
    runner.on(
        "agent start --help",
        ok("[possible values: pi, claude, agy]"),
    );
    runner.on(
        "/usr/bin/curl",
        ok(&jev::http_response(
            &response(level, confidence).to_string(),
            200,
        )),
    );
    runner
}
fn policy(world: &World) -> routing::Policy {
    routing::Policy::read(&world.home.path().join("cfg/routing.json")).unwrap()
}
fn edit_policy(world: &World, edit: impl FnOnce(&mut Value)) {
    let path = world.home.path().join("cfg/routing.json");
    let mut value = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    edit(&mut value);
    std::fs::write(path, value.to_string()).unwrap();
}
fn request(runner: &FakeRunner) -> Value {
    let calls = runner.calls.borrow();
    let call = calls.iter().find(|c| c.program == "/usr/bin/curl").unwrap();
    let line = call
        .stdin
        .as_ref()
        .unwrap()
        .lines()
        .find_map(|s| s.strip_prefix("data = "))
        .unwrap();
    let body: String = serde_json::from_str(line).unwrap();
    serde_json::from_str(&body).unwrap()
}

fn ledger_rows(project: &crate::project::Project) -> Vec<Value> {
    std::fs::read_to_string(project.state_dir().join("dispatch.jsonl"))
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn three_tier_floors(world: &World) {
    edit_policy(world, |p| {
        p["models"]["pi_codex_sol_high"] =
            json!({"tier":3,"coding_index":78.0,"price_per_million":3.0,"description":"highest"});
        p["role_floors"] = json!({"reviewer":"test_strong"});
        p["answer_floors"] =
            json!([{ "question":"required_index", "min_score":3, "recipe":"pi_codex_sol_high" }]);
    });
}

#[test]
fn floors_raise_by_tier_without_lowering_or_pinning_and_escalate_above_the_floor() {
    let (world, project) = setup();
    three_tier_floors(&world);
    let runner = runner(0.0, 1.0);
    let ctx = Ctx {
        runner: &runner,
        ..world.ctx()
    };
    let input = ResolveInput {
        task: "Review persistence.",
        workflow: "reviewer",
        ..Default::default()
    };
    let first = launch::resolve_launch(&ctx, &project, &input).unwrap();
    assert_eq!(first.recipe_id, "test_strong");
    assert_eq!(first.strength, 2);
    let rows = ledger_rows(&project);
    assert_eq!(rows[0]["rule"], "jev-scores-floor");
    assert_eq!(
        rows[0]["floors"][0]["cause"],
        json!({"kind":"role","role":"reviewer"})
    );
    assert_eq!(rows[0]["floors"][0]["recipe"], "test_strong");
    assert!(rows[0]["assessment"].is_object());
    let next = launch::resolve_launch(
        &ctx,
        &project,
        &ResolveInput {
            previous: Some(&first),
            failure: Some("missed a durable record"),
            state: input.state.clone(),
            ..input
        },
    )
    .unwrap();
    assert_eq!(next.recipe_id, "pi_codex_sol_high");
    let error = launch::resolve_launch(
        &ctx,
        &project,
        &ResolveInput {
            previous: Some(&next),
            failure: Some("still wrong"),
            ..input
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("escalation_exhausted"));
    assert_eq!(runner.count("/usr/bin/curl"), 2);

    let p = policy(&world);
    for (level, flow, expected) in [
        (0.0, "lane", "test_claude"),
        (2.0, "reviewer", "test_strong"),
        (3.0, "reviewer", "pi_codex_sol_high"),
    ] {
        let a = jev::parse(&response(level, 1.0).to_string(), &p.questions).unwrap();
        let d = p.select(&a, None, flow).unwrap();
        assert_eq!(d.recipe, expected);
    }
}

#[test]
fn manual_thread_start_reviewer_uses_the_shared_role_floor() {
    let (world, project) = setup();
    three_tier_floors(&world);
    *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
    world.runner.on(
        "/usr/bin/curl",
        ok(&jev::http_response(&response(0.0, 1.0).to_string(), 200)),
    );
    world.runner.on("tab create", ok(r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t3","pane_id":"w1:p3","cwd":"/wt"}}}"#));
    let started = crate::threads::start(
        &world.ctx(),
        "demo",
        crate::threads::StartArgs {
            title: "Review a change".into(),
            repo: None,
            machine: None,
            base: None,
            task: "Check the saved records.".into(),
            plain: "This check reads the work.".into(),
            workflow: Some("reviewer".into()),
        },
    )
    .unwrap();
    assert_eq!(started.role, "reviewer");
    assert_eq!(started.launch.recipe_id, "test_strong");
    assert_eq!(world.runner.count("/usr/bin/curl"), 1);
    assert_eq!(ledger_rows(&project)[0]["rule"], "jev-scores-floor");
}

#[test]
fn answer_veto_uses_raw_score_and_logs_each_raising_cause() {
    let (world, project) = setup();
    three_tier_floors(&world);
    for (score, flow, expected, causes) in [
        (2.99, "lane", "test_strong", 0),
        (3.0, "lane", "pi_codex_sol_high", 1),
        (3.0, "reviewer", "pi_codex_sol_high", 1),
    ] {
        let mut answer = response(0.0, 1.0);
        answer["answers"]["required_index"] =
            response(score, 1.0)["answers"]["required_index"].clone();
        let runner = FakeRunner::new();
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        runner.on(
            "/usr/bin/curl",
            ok(&jev::http_response(&answer.to_string(), 200)),
        );
        let ctx = Ctx {
            runner: &runner,
            ..world.ctx()
        };
        let selected = launch::resolve_launch(
            &ctx,
            &project,
            &ResolveInput {
                task: "Write validation records.",
                workflow: flow,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(selected.recipe_id, expected);
        let rows = ledger_rows(&project);
        let row = rows.last().unwrap();
        assert_eq!(row["floors"].as_array().unwrap().len(), causes);
        if causes > 0 {
            assert_eq!(row["rule"], "jev-scores-floor");
            assert_eq!(
                row["floors"].as_array().unwrap().last().unwrap()["cause"],
                json!({"kind":"answer","question":"required_index","min_score":3.0,"score":3.0})
            );
        } else {
            assert_eq!(row["rule"], "jev-scores");
        }
    }
}

#[test]
fn strongest_floor_wins() {
    let (world, _) = setup();
    three_tier_floors(&world);
    edit_policy(&world, |p| {
        p["answer_floors"][0]["recipe"] = json!("pi_codex_sol_high");
        p["answer_floors"][0]["min_score"] = json!(0.0);
    });
    let p = policy(&world);
    let a = jev::parse(&response(0.0, 1.0).to_string(), &p.questions).unwrap();
    let d = p.select(&a, None, "reviewer").unwrap();
    assert_eq!(d.recipe, "pi_codex_sol_high");
    assert_eq!(d.floors.len(), 2);
}

#[test]
fn invalid_floor_targets_and_conditions_are_refused() {
    let (world, _) = setup();
    let original: Value = serde_json::from_str(
        &std::fs::read_to_string(world.ctx().config_dir.join("routing.json")).unwrap(),
    )
    .unwrap();
    for floors in [
        json!({"role_floors":{"typo":"test_strong"}}),
        json!({"role_floors":{"reviewer":"missing"}}),
        json!({"answer_floors":[{"question":"typo","min_score":3,"recipe":"test_strong"}]}),
        json!({"answer_floors":[{"question":"required_index","min_score":4,"recipe":"test_strong"}]}),
        json!({"answer_floors":[{"question":"required_index","min_score":-1,"recipe":"test_strong"}]}),
    ] {
        let mut p = original.clone();
        p.as_object_mut()
            .unwrap()
            .extend(floors.as_object().unwrap().clone());
        assert!(routing::Policy::parse(p.to_string().as_bytes()).is_err());
    }
    three_tier_floors(&world);
    let p = policy(&world);
    let mut recipes = launch::parse_launch_config(&world.ctx().config_dir)
        .unwrap()
        .recipes;
    let mut p = p;
    p.answer_floors[0].recipe = "test_strong".into();
    recipes.get_mut("test_strong").unwrap().enabled = false;
    assert!(
        p.validate_recipes(&recipes)
            .unwrap_err()
            .to_string()
            .contains("routing_recipe_disabled: test_strong")
    );
    p.role_floors.clear(); // Answer-only targets must be validated as well.
    assert!(
        p.validate_recipes(&recipes)
            .unwrap_err()
            .to_string()
            .contains("routing_recipe_disabled: test_strong")
    );
    recipes.remove("test_strong");
    assert!(
        p.validate_recipes(&recipes)
            .unwrap_err()
            .to_string()
            .contains("routing_recipe_missing: test_strong")
    );
}

#[test]
fn explicit_picker_size_refusals_fall_back_to_top_and_remain_auditable() {
    for (status, body) in [
        (
            400,
            r#"{"error":{"code":"max_tokens_exceeded"},"detail":"fake-key"}"#,
        ),
        (413, "too large"),
    ] {
        let (world, project) = setup();
        three_tier_floors(&world);
        let runner = FakeRunner::new();
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        runner.on("/usr/bin/curl", ok(&jev::http_response(body, status)));
        let ctx = Ctx {
            runner: &runner,
            ..world.ctx()
        };
        let input = ResolveInput {
            task: "Review many reports.",
            workflow: "reviewer",
            ..Default::default()
        };
        let selected = launch::resolve_launch(&ctx, &project, &input).unwrap();
        assert_eq!(selected.recipe_id, "pi_codex_sol_high");
        let rows = ledger_rows(&project);
        assert_eq!(rows[0]["rule"], "jev-size-fallback");
        assert_eq!(rows[0]["fallback"]["cause"], "picker-input-too-large");
        assert!(rows[0]["assessment"].is_null());
        assert!(rows[0]["decision"].is_null());
        assert!(!rows[0].to_string().contains("fake-key"));
        assert!(
            launch::resolve_launch(
                &ctx,
                &project,
                &ResolveInput {
                    previous: Some(&selected),
                    failure: Some("needs more"),
                    ..input
                }
            )
            .unwrap_err()
            .to_string()
            .contains("escalation_exhausted")
        );
        assert_eq!(runner.count("/usr/bin/curl"), 1);
    }
}

#[test]
fn picker_auth_and_service_errors_never_use_size_fallback() {
    for status in [401, 403, 429, 500, 503] {
        let (world, project) = setup();
        let runner = FakeRunner::new();
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        runner.on(
            "/usr/bin/curl",
            ok(&jev::http_response(
                r#"{"code":"max_tokens_exceeded"}"#,
                status,
            )),
        );
        let ctx = Ctx {
            runner: &runner,
            ..world.ctx()
        };
        let error = launch::resolve_launch(
            &ctx,
            &project,
            &ResolveInput {
                task: "Review changes.",
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(error.to_string().contains(&format!("HTTP {status}")));
        assert_eq!(ledger_rows(&project)[0]["kind"], "dispatch-refused");
    }
}

#[test]
fn full_brief_and_repository_not_title_reach_the_index_score() {
    let (world, project) = setup();
    let runner = runner(0.0, 0.99);
    let ctx = Ctx {
        runner: &runner,
        ..world.ctx()
    };
    let task = format!(
        "Implement a state machine. Read https://docs.typesafe.ai/api.md. {} FINAL-ACCEPTANCE",
        "coupled requirements ".repeat(300)
    );
    let selected = launch::resolve_launch(
        &ctx,
        &project,
        &ResolveInput {
            task: &task,
            state: json!({"head":"abc", "files":"src/state.rs", "diff":"critical invariant"}),
            workflow: "lane",
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(selected.recipe_id, "test_claude");
    let body = request(&runner);
    assert_eq!(body["state"]["brief"], task);
    assert_eq!(body["state"]["repository"]["head"], "abc");
    assert_eq!(body["questions"].as_object().unwrap().len(), 1);
    for q in body["questions"].as_object().unwrap().values() {
        assert_eq!(q["type"], "score");
        assert!(q["criteria"].is_array());
    }
    assert!(!body.to_string().contains("test_strong"));
    let calls = runner.calls.borrow();
    let curl = calls.iter().find(|c| c.program == "/usr/bin/curl").unwrap();
    assert!(!curl.args.join(" ").contains("fake-key"));
    assert!(curl.env.is_empty());
    assert!(curl.env_remove.contains(&"TYPESAFE_API_KEY".into()));
}

#[test]
fn all_four_exclusions_bypass_inference_including_coordinator() {
    let (world, project) = setup();
    three_tier_floors(&world);
    let runner = runner(3.0, 1.0);
    let ctx = Ctx {
        runner: &runner,
        ..world.ctx()
    };
    for (task, flow, expected) in [
        (
            "+++\nproduct = \"web-research\"\n+++\nCompare published API documentation.",
            "lane",
            "agy_gemini_flash",
        ),
        (
            "+++\nrequires_claude = true\n+++\nUse the Claude-only tool integration.",
            "lane",
            "claude_fable_xhigh",
        ),
        (
            "+++\nproduct = \"spec\"\n+++\nWrite a design specification.",
            "reviewer",
            "claude_fable_xhigh",
        ),
        (
            "Coordinate this project.",
            "coordinator",
            "claude_coordinator_opus",
        ),
    ] {
        let selected = launch::resolve_launch(
            &ctx,
            &project,
            &ResolveInput {
                task,
                workflow: flow,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(selected.recipe_id, expected);
    }
    let brief = "Rolf pinned this exact brief.";
    edit_policy(&world, |p| {
        p["pins"][thread::sha256_hex(brief.as_bytes())] = json!("test_strong")
    });
    assert_eq!(
        launch::resolve_launch(
            &ctx,
            &project,
            &ResolveInput {
                task: brief,
                ..Default::default()
            }
        )
        .unwrap()
        .recipe_id,
        "test_strong"
    );
    assert_eq!(runner.count("/usr/bin/curl"), 0);
}

#[test]
fn low_confidence_upgrade_never_blocks_and_is_logged() {
    let (world, project) = setup();
    let runner = runner(0.0, 0.2);
    let ctx = Ctx {
        runner: &runner,
        ..world.ctx()
    };
    assert_eq!(
        launch::resolve_launch(
            &ctx,
            &project,
            &ResolveInput {
                task: "Make the change.",
                ..Default::default()
            }
        )
        .unwrap()
        .recipe_id,
        "test_strong"
    );
    let rows = std::fs::read_to_string(project.state_dir().join("dispatch.jsonl")).unwrap();
    assert!(rows.contains("low-confidence"));
}

#[test]
fn failure_asks_again_with_evidence_and_strictly_upgrades_then_stops() {
    let (world, project) = setup();
    let runner = runner(0.0, 1.0);
    let ctx = Ctx {
        runner: &runner,
        ..world.ctx()
    };
    let previous = Launch {
        recipe_id: "test_claude".into(),
        strength: 1,
        ..Default::default()
    };
    let input = ResolveInput {
        task: "Repair persistence.",
        previous: Some(&previous),
        failure: Some("Lost an event after a crash"),
        ..Default::default()
    };
    let selected = launch::resolve_launch(&ctx, &project, &input).unwrap();
    assert_eq!(selected.recipe_id, "test_strong");
    assert_eq!(selected.escalations, 1);
    assert_eq!(
        request(&runner)["state"]["failure"],
        "Lost an event after a crash"
    );
    let error = launch::resolve_launch(
        &ctx,
        &project,
        &ResolveInput {
            previous: Some(&selected),
            state: input.state.clone(),
            ..input
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("escalation_exhausted"));
    let bounded = Launch {
        escalations: launch::MAX_ESCALATIONS,
        ..previous.clone()
    };
    let error = launch::resolve_launch(
        &ctx,
        &project,
        &ResolveInput {
            previous: Some(&bounded),
            ..input
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("escalation_bound"));
    assert_eq!(runner.count("/usr/bin/curl"), 1);
}

#[test]
fn policy_changes_reroute_saved_scores_without_changing_prompt_or_calling_jev() {
    let (world, _) = setup();
    let old = policy(&world);
    let assessment = jev::parse(&response(1.0, 1.0).to_string(), &old.questions).unwrap();
    assert_eq!(
        old.select(&assessment, None, "lane").unwrap().recipe,
        "test_claude"
    );
    edit_policy(&world, |p| {
        p["models"]["test_claude"]["coding_index"] = json!(1.0)
    });
    let new = policy(&world);
    assert_eq!(new.questions, old.questions);
    assert_eq!(
        new.select(&assessment, None, "lane").unwrap().recipe,
        "test_strong"
    );
    assert_eq!(world.runner.count("/usr/bin/curl"), 0);
}

#[test]
fn evaluator_uses_finished_lane_and_round_outcomes_without_labels() {
    let (world, project) = setup();
    let cwd = world.home.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let answer = response(0.0, 0.9);
    let assessment = jev::parse(&answer.to_string(), &policy(&world).questions).unwrap();
    let task = "Make the bounded change.";
    let hash = thread::sha256_hex(task.as_bytes());
    let lane = world.thread(&project, &cwd, |t| {
        t.role = "lane".into();
        t.attempt = 1;
        t.launch = Launch {
            recipe_id: "test_claude".into(),
            strength: 1,
            brief_hash: hash.clone(),
            ..Launch::default()
        };
    });
    std::fs::write(thread::task_path(&project, &lane.id), task).unwrap();
    let escalated_task = "Make the change that needed a stronger helper.";
    let escalated_hash = thread::sha256_hex(escalated_task.as_bytes());
    let escalated_lane = world.thread(&project, &cwd, |t| {
        t.role = "lane".into();
        t.attempt = 2;
        t.launch = Launch {
            recipe_id: "test_strong".into(),
            strength: 2,
            brief_hash: escalated_hash.clone(),
            escalations: 1,
            ..Launch::default()
        };
    });
    std::fs::write(
        thread::task_path(&project, &escalated_lane.id),
        escalated_task,
    )
    .unwrap();
    std::fs::write(
        project.state_dir().join("dispatch.jsonl"),
        format!(
            "{}\n{}\n{}\n",
            json!({"kind":"pick","brief_hash":hash,"recipe":"test_claude","escalations":0,"failure":null,"assessment":assessment}),
            json!({"kind":"pick","brief_hash":escalated_hash,"recipe":"test_claude","escalations":0,"failure":null,"assessment":assessment}),
            json!({"kind":"escalation","brief_hash":escalated_hash,"recipe":"test_strong","escalations":1,"failure":"the first run failed","previous":{"recipe":"test_claude"},"assessment":assessment}),
        ),
    )
    .unwrap();
    let event = Event {
        id: "done-1".into(),
        op: "done-1".into(),
        thread: lane.id.clone(),
        attempt: 1,
        round: Some("r1".into()),
        recipient: Default::default(),
        created: crate::project::now(),
        payload: EventPayload {
            done: Some(DonePayload::default()),
            ..Default::default()
        },
    };
    std::fs::create_dir_all(crate::round::events_dir(&project)).unwrap();
    std::fs::write(
        crate::round::events_dir(&project).join("done-1.toml"),
        toml::to_string(&event).unwrap(),
    )
    .unwrap();
    let escalated_event = Event {
        id: "done-2".into(),
        op: "done-2".into(),
        thread: escalated_lane.id.clone(),
        attempt: 2,
        round: Some("r1".into()),
        recipient: Default::default(),
        created: crate::project::now(),
        payload: EventPayload {
            done: Some(DonePayload::default()),
            ..Default::default()
        },
    };
    std::fs::write(
        crate::round::events_dir(&project).join("done-2.toml"),
        toml::to_string(&escalated_event).unwrap(),
    )
    .unwrap();
    let round = RoundRecord {
        phase: RoundPhase::Merged,
        round: "r1".into(),
        manifest: AdmissionManifest {
            revision: 1,
            members: vec![
                ManifestMember {
                    thread: lane.id.clone(),
                    pin: None,
                },
                ManifestMember {
                    thread: escalated_lane.id.clone(),
                    pin: None,
                },
            ],
        },
        rejections: Some(0),
        merge: Some(MergeIntent {
            op: "merge-1".into(),
            expected_old: "old".into(),
            candidate: "candidate".into(),
            verdict: "verdict".into(),
            phase: MergePhase::Checkpointed,
            merged: Some("merged".into()),
            checkpoint: None,
            head: Some("head".into()),
        }),
        ..RoundRecord::default()
    };
    std::fs::create_dir_all(crate::round::rounds_dir(&project)).unwrap();
    std::fs::write(
        crate::round::round_path(&project, "r1"),
        toml::to_string(&round).unwrap(),
    )
    .unwrap();

    let result = routing::evaluate(&world.ctx(), &project).unwrap();
    assert_eq!(result.finished_lanes, 2);
    assert_eq!(result.observed_good_enough, 1);
    assert_eq!(result.observed_not_good_enough, 1);
    assert_eq!(result.policy_confirmed_good, 1);
    assert_eq!(result.policy_confirmed_bad, 1);
    assert_eq!(result.confidence_clear, 2);
    let escalated = result
        .outcomes
        .iter()
        .find(|outcome| outcome["thread"] == escalated_lane.id)
        .unwrap();
    assert_eq!(escalated["model_run"], "test_claude");
    assert_eq!(escalated["final_model"], "test_strong");
    assert_eq!(escalated["failure"], "the first run failed");
    assert_eq!(escalated["policy_result"], "confirmed-bad");

    let mut pending = round;
    pending.phase = RoundPhase::UnderReview;
    pending.merge = None;
    std::fs::write(
        crate::round::round_path(&project, "r1"),
        toml::to_string(&pending).unwrap(),
    )
    .unwrap();
    let pending_result = routing::evaluate(&world.ctx(), &project).unwrap();
    assert_eq!(pending_result.no_round_outcome, 2);
    assert_eq!(pending_result.observed_not_good_enough, 0);
    assert_eq!(pending_result.policy_confirmed_bad, 0);
    assert_eq!(world.runner.count("/usr/bin/curl"), 0);
}

#[test]
fn oversized_repository_state_is_cut_disclosed_and_logged() {
    let (world, project) = setup();
    let runner = runner(0.0, 1.0);
    let ctx = Ctx {
        runner: &runner,
        ..world.ctx()
    };
    let brief = "Classify this complete bounded task brief.";
    launch::resolve_launch(
        &ctx,
        &project,
        &ResolveInput {
            task: brief,
            state: json!({"head":"abc", "files":"generated.json\n".repeat(30_000),
                "recent_changes":"small useful summary"}),
            ..Default::default()
        },
    )
    .unwrap();
    let body = request(&runner);
    assert!(body.to_string().len() <= jev::REQUEST_BYTE_CAP);
    assert_eq!(body["state"]["brief"], brief);
    assert_eq!(body["state"]["repository"]["head"], "abc");
    assert_eq!(body["state"]["input_truncation"]["cut"], true);
    assert!(
        body["state"]["input_truncation"]["omitted_fields"]
            .as_array()
            .unwrap()
            .iter()
            .any(|field| field == "repository.files")
    );
    let ledger = std::fs::read_to_string(project.state_dir().join("dispatch.jsonl")).unwrap();
    assert!(ledger.contains("dispatch-input-truncated"));
    assert!(ledger.contains("input_truncation"));
}

#[test]
fn brief_that_cannot_fit_is_refused_without_a_call() {
    let (world, project) = setup();
    let runner = runner(0.0, 1.0);
    let ctx = Ctx {
        runner: &runner,
        ..world.ctx()
    };
    let brief = "b".repeat(jev::REQUEST_BYTE_CAP + 1);
    let error = launch::resolve_launch(
        &ctx,
        &project,
        &ResolveInput {
            task: &brief,
            state: json!({"files":"cheap evidence"}),
            ..Default::default()
        },
    )
    .unwrap_err();
    assert!(error.to_string().contains("dispatch_brief_too_large"));
    assert_eq!(runner.count("/usr/bin/curl"), 0);
}

#[test]
fn server_refusal_includes_status_and_body_but_never_the_key() {
    let (world, project) = setup();
    let runner = FakeRunner::new();
    runner.on(
        "agent start --help",
        ok("[possible values: pi, claude, agy]"),
    );
    runner.on(
        "/usr/bin/curl",
        ok(&jev::http_response(
            r#"{"detail":"request is too large; fake-key must not leak"}"#,
            400,
        )),
    );
    let ctx = Ctx {
        runner: &runner,
        ..world.ctx()
    };
    let error = launch::resolve_launch(
        &ctx,
        &project,
        &ResolveInput {
            task: "Implement the parser.",
            ..Default::default()
        },
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("jev_server_refused: HTTP 400"), "{error}");
    assert!(error.contains("request is too large"), "{error}");
    assert!(!error.contains("fake-key"), "{error}");
}

#[test]
fn timeout_is_reported_as_a_timeout_not_a_server_or_transport_failure() {
    let (world, project) = setup();
    // curl's 30-second deadline normally fires before the runner's 35-second
    // watchdog. Both paths must have the same classification.
    for output in [timeout(), fail(28, "curl: (28) Operation timed out")] {
        let runner = FakeRunner::new();
        runner.on(
            "agent start --help",
            ok("[possible values: pi, claude, agy]"),
        );
        runner.on("/usr/bin/curl", output);
        let ctx = Ctx {
            runner: &runner,
            ..world.ctx()
        };
        let error = launch::resolve_launch(
            &ctx,
            &project,
            &ResolveInput {
                task: "Implement the parser.",
                ..Default::default()
            },
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("jev_timeout"), "{error}");
        assert!(!error.contains("jev_server_refused"), "{error}");
        assert!(!error.contains("jev_transport"), "{error}");
    }
}

#[test]
fn a_policy_cannot_route_to_a_model_below_the_required_index() {
    let (world, _) = setup();
    edit_policy(&world, |policy| {
        policy["models"]["test_strong"]["coding_index"] = json!(77.1);
    });
    let error = routing::Policy::read(&world.home.path().join("cfg/routing.json"))
        .unwrap_err()
        .to_string();
    assert!(error.contains("cover every index value"), "{error}");
}

#[test]
fn malformed_score_http_failure_missing_policy_and_leftover_roles_fail_closed() {
    let (world, project) = setup();
    let p = policy(&world);
    let mut bad = response(0.0, 1.0);
    bad["answers"]["required_index"]["probabilities"]["0"] = json!(0.1);
    assert!(jev::parse(&bad.to_string(), &p.questions).is_err());
    bad["answers"]["required_index"]["confidence"] = json!(2.0);
    assert!(jev::parse(&bad.to_string(), &p.questions).is_err());
    let runner = FakeRunner::new();
    runner.on(
        "agent start --help",
        ok("[possible values: pi, claude, agy]"),
    );
    runner.on("/usr/bin/curl", fail(22, "private server error"));
    let ctx = Ctx {
        runner: &runner,
        ..world.ctx()
    };
    assert!(
        launch::resolve_launch(
            &ctx,
            &project,
            &ResolveInput {
                task: "Implement the parser.",
                ..Default::default()
            }
        )
        .unwrap_err()
        .to_string()
        .contains("jev_transport")
    );
    std::fs::remove_file(ctx.config_dir.join("routing.json")).unwrap();
    assert!(
        launch::doctor_rows(&ctx)
            .unwrap()
            .iter()
            .any(|r| r.ok == Some(false))
    );
    std::fs::write(
        ctx.config_dir.join("config.toml"),
        "[roles.reviewer]\ndefault = \"old\"\n",
    )
    .unwrap();
    let err = launch::doctor_rows(&ctx).unwrap_err().to_string();
    assert!(err.contains("roles_removed") && err.contains("remove [roles]"));
}

#[test]
fn model_override_in_task_contract_is_refused_but_urls_are_not_research() {
    assert!(launch::work_contract("+++\nrecipe = \"test_strong\"\n+++\nDo work.", "lane").is_err());
    let (world, _) = setup();
    let brief = "Build an HTTP client using https://docs.typesafe.ai/api.md";
    assert!(
        policy(&world)
            .exclusion(brief, &launch::work_contract(brief, "lane").unwrap())
            .is_none()
    );
}

#[test]
fn scrubbing_keeps_scope_and_ordinary_high_effort_words() {
    let (world, _) = setup();
    let config = launch::parse_launch_config(&world.ctx().config_dir).unwrap();
    let state = routing::scrub(
        json!({"brief":"Use gpt-5.6-sol for high risk code; keep invariants."}),
        &config.recipes,
    );
    assert_eq!(
        state["brief"],
        "Use [model] for high risk code; keep invariants."
    );
}

#[test]
fn failure_event_is_replay_safe_and_replacement_preserves_dirty_work() {
    let (world, project) = setup();
    let cwd = world.home.path().join("work");
    std::fs::create_dir_all(&cwd).unwrap();
    let dirty = cwd.join("uncommitted.txt");
    std::fs::write(&dirty, "keep this work").unwrap();
    let old = world.thread(&project, &cwd, |t| {
        t.kind = thread::Kind::Tab;
        t.repo.clear();
        t.attempt = 1;
        t.launch = Launch {
            kind: "claude".into(),
            recipe_id: "test_claude".into(),
            strength: 1,
            attempt: 1,
            brief_hash: "brief-hash".into(),
            ..Default::default()
        };
    });
    std::fs::write(
        thread::task_path(&project, &old.id),
        "Fix the parser and test it.",
    )
    .unwrap();
    let event = Event {
        id: "t-0001-1-1".into(),
        op: "t-0001-1-1".into(),
        thread: old.id.clone(),
        attempt: 1,
        round: None,
        recipient: Default::default(),
        created: crate::project::now(),
        payload: EventPayload {
            failed: Some(WaitingPayload {
                text: "The parser still drops input".into(),
            }),
            ..Default::default()
        },
    };
    // A box failure uses the same immutable event, without a DONE artifact.
    let bytes = toml::to_string(&event).unwrap().into_bytes();
    assert_eq!(
        events::import_box_event(&project, "oci", &bytes, None).unwrap(),
        events::ImportOutcome::New
    );
    assert_eq!(
        events::import_box_event(&project, "oci", &bytes, None).unwrap(),
        events::ImportOutcome::Replay
    );
    world.runner.on("tab create",ok(r#"{"result":{"root_pane":{"workspace_id":"w1","tab_id":"w1:t3","pane_id":"w1:p3","cwd":"/wt"}}}"#));
    *world.panes.borrow_mut() = format!("[{}]", world.coordinator_pane(&project));
    escalation::tick(&world.ctx(), &project).unwrap();
    let next = thread::load(&project, &old.id).unwrap();
    assert_eq!(next.attempt, 2);
    assert_eq!(next.launch.recipe_id, "test_strong");
    assert!(!next.escalation_pending);
    assert!(next.prompt_pending);
    assert!(thread::launch_prompt("ha", "demo", &next).contains("parser still drops input"));
    escalation::tick(&world.ctx(), &project).unwrap();
    assert_eq!(world.runner.count("/usr/bin/curl"), 1);
    assert_eq!(std::fs::read_to_string(dirty).unwrap(), "keep this work");
}
