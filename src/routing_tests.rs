//! Offline dispatch, policy replay and durable escalation gates. Never calls Jev.
use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::contracts::{Event, EventPayload, Launch, WaitingPayload};
use crate::launch::{self, ResolveInput};
use crate::paths::Ctx;
use crate::runner::fake::{FakeRunner, fail, ok};
use crate::scenarios::World;
use crate::{escalation, events, jev, routing, thread};

fn response(level: f64, confidence: f64) -> Value {
    let mut answers = BTreeMap::new();
    for id in ["difficulty", "ambiguity", "blast_radius"] {
        let lower = level.floor() as usize;
        let upper = level.ceil() as usize;
        let mut probabilities: BTreeMap<String, f64> =
            (0..4).map(|i| (i.to_string(), 0.0)).collect();
        probabilities.insert(lower.to_string(), 1.0 - level.fract());
        if upper != lower {
            probabilities.insert(upper.to_string(), level.fract());
        }
        answers.insert(id, json!({"type":"score", "score":level, "confidence":confidence, "probabilities":probabilities}));
    }
    json!({"model":"jev-test", "answers":answers, "usage":{"input_tokens":70,"output_tokens":30}})
}

fn setup() -> (World, crate::project::Project) {
    let world = World::new();
    let cfg = world.home.path().join("cfg");
    std::fs::write(cfg.join("config.toml"), "[recipes.test_claude]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the quick helper\"\n[recipes.test_strong]\nkind = \"claude\"\nargs = [\"--dangerously-skip-permissions\"]\nplain = \"the careful helper\"\n").unwrap();
    let mut policy: Value = serde_json::from_str(include_str!("../config/routing.json")).unwrap();
    policy["models"] = json!({"test_claude":{"tier":1,"description":"cheap"},"test_strong":{"tier":2,"description":"strong"}});
    policy["routes"] =
        json!([{"up_to":0.5,"recipe":"test_claude"},{"up_to":1.0,"recipe":"test_strong"}]);
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
        ok(&response(level, confidence).to_string()),
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

#[test]
fn full_brief_and_repository_not_title_reach_three_parallel_scores() {
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
    assert_eq!(body["questions"].as_object().unwrap().len(), 3);
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
            "lane",
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
fn confidence_and_borderline_upgrade_never_block_and_are_logged() {
    let (world, project) = setup();
    for (level, confidence, reason) in [(0.0, 0.2, "low-confidence"), (1.48, 0.99, "borderline")] {
        let runner = runner(level, confidence);
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
        assert!(rows.contains(reason));
    }
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
    assert_eq!(old.select(&assessment, None).unwrap().recipe, "test_claude");
    edit_policy(&world, |p| p["routes"][0]["up_to"] = json!(0.2));
    let new = policy(&world);
    assert_eq!(new.questions, old.questions);
    assert_eq!(new.select(&assessment, None).unwrap().recipe, "test_strong");
    assert_eq!(world.runner.count("/usr/bin/curl"), 0);
}

#[test]
fn evaluator_counts_both_costly_mistakes_and_preserves_scores() {
    let (world, _) = setup();
    let cases = world.home.path().join("cases.json");
    let case = |id, expected, level| json!({"id":id,"brief":"A full labelled task with scope and gates.","state":{"files":"lib.rs"},"expected":expected,"response":response(level,1.0)});
    std::fs::write(
        &cases,
        json!([
            case("correct", "test_claude", 0.0),
            case("wasted-money", "test_claude", 3.0),
            case("wasted-lane", "test_strong", 0.0)
        ])
        .to_string(),
    )
    .unwrap();
    let result = routing::evaluate(&world.ctx(), &cases).unwrap();
    assert_eq!(
        (
            result.correct,
            result.over_routed,
            result.under_routed,
            result.errors
        ),
        (1, 1, 1, 0)
    );
    assert!(result.cases[0]["assessment"]["scores"].is_object());
    assert_eq!(world.runner.count("/usr/bin/curl"), 0);
}

#[test]
fn malformed_score_http_failure_missing_policy_and_leftover_roles_fail_closed() {
    let (world, project) = setup();
    let p = policy(&world);
    let mut bad = response(0.0, 1.0);
    bad["answers"]["difficulty"]["probabilities"]["0"] = json!(0.1);
    assert!(jev::parse(&bad.to_string(), &p.questions).is_err());
    bad["answers"]["difficulty"]["confidence"] = json!(2.0);
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
