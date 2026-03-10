use assert_cmd::Command;
use predicates::prelude::*;
use tempfile::tempdir;

#[test]
fn startup_summary_shows_role_models() {
    let temp = tempdir().expect("tempdir");
    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", temp.path())
        .args([
            "--model",
            "pm-model",
            "--editor-model",
            "editor-model",
            "--permission-mode",
            "workspace-write",
            "--network",
            "local-only",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("interactive mode"))
        .stdout(predicate::str::contains("PM: pm-model"))
        .stdout(predicate::str::contains("Reader: pm-model (inherited)"))
        .stdout(predicate::str::contains("Editor: editor-model"))
        .stdout(predicate::str::contains("Permission mode: workspace-write"))
        .stdout(predicate::str::contains("Network: local-only"));
}

#[test]
fn prompt_conflicts_with_subcommand() {
    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .args(["-p", "inspect", "resume", "session-1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--prompt cannot be used together with a subcommand",
        ));
}

#[test]
fn resume_uses_stored_models_until_overridden() {
    let temp = tempdir().expect("tempdir");

    let start = Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", temp.path())
        .args([
            "-p",
            "inspect repo",
            "--pm-model",
            "pm-stored",
            "--reviewer-model",
            "reviewer-stored",
            "--permission-mode",
            "workspace-write",
            "--network",
            "local-only",
        ])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let stdout = String::from_utf8(start).expect("utf8");
    let session_line = stdout
        .lines()
        .find(|line| line.starts_with("session: "))
        .expect("session line");
    let session_id = session_line.trim_start_matches("session: ");

    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", temp.path())
        .args(["resume", session_id, "--editor-model", "editor-override"])
        .assert()
        .success()
        .stdout(predicate::str::contains("PM: pm-stored"))
        .stdout(predicate::str::contains("Reviewer: reviewer-stored"))
        .stdout(predicate::str::contains("Editor: editor-override"))
        .stdout(predicate::str::contains("Permission mode: workspace-write"))
        .stdout(predicate::str::contains("Network: local-only"));
}

#[test]
fn prompt_mode_returns_pm_or_subagent_result() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", temp.path())
        .args(["-p", "inspect the repository layout", "--model", "pm-model"])
        .assert()
        .success()
        .stdout(predicate::str::contains("prompt mode"))
        .stdout(predicate::str::contains("response: Reader inspected "));
}
