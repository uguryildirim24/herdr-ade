use std::process::Command;

#[test]
fn coordinator_cannot_select_a_role_recipe_or_model() {
    for flag in ["--role", "--recipe", "--model"] {
        let output = Command::new(env!("CARGO_BIN_EXE_herdr-ade"))
            .args([
                "thread",
                "start",
                "demo",
                "--title",
                "work",
                "--task-file",
                "brief.md",
                flag,
                "chosen",
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(
            error.contains("unexpected argument") && error.contains(flag),
            "{error}"
        );
    }
}

#[test]
fn shipped_cases_run_offline_and_report_both_error_directions() {
    let home = tempfile::tempdir().unwrap();
    let config = home.path().join("config/herdr-ade");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::write(
        config.join("routing.json"),
        include_str!("../config/routing.json"),
    )
    .unwrap();
    let cases = home.path().join("cases.json");
    std::fs::write(&cases, include_str!("../config/routing-cases.json")).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-ade"))
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env_remove("TYPESAFE_API_KEY")
        .args(["routing-eval", cases.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["correct"], 1);
    assert_eq!(result["over_routed"], 1);
    assert_eq!(result["under_routed"], 1);
    assert_eq!(result["errors"], 0);
}
