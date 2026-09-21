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
