use std::process::Command;

#[test]
fn coordinator_cannot_select_a_role_or_model() {
    for flag in ["--role", "--model"] {
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
fn an_exact_recipe_needs_no_quote_and_basis_is_not_a_lane_flag() {
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-ade"))
        .args([
            "thread",
            "start",
            "demo",
            "--title",
            "work",
            "--task-file",
            "brief.md",
            "--recipe",
            "chosen",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("could not read brief.md"), "{error}");
    assert!(!error.contains("--basis"), "{error}");
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-ade"))
        .args([
            "thread",
            "start",
            "demo",
            "--task-file",
            "brief.md",
            "--recipe",
            "chosen",
            "--basis",
            "old quote",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains("unexpected argument '--basis'"), "{error}");
}

#[test]
fn workflow_help_describes_routing_match() {
    let output = Command::new(env!("CARGO_BIN_EXE_herdr-ade"))
        .args(["thread", "start", "--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("routing table may match it"), "{help}");
    assert!(!help.contains("never selects its model"));
}
