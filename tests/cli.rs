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
        .stdout(predicate::str::contains(
            "Working summary: interactive session",
        ))
        .stdout(predicate::str::contains("PM: pm-model"))
        .stdout(predicate::str::contains("Reader: pm-model (inherited)"))
        .stdout(predicate::str::contains("Editor: editor-model"))
        .stdout(predicate::str::contains("Permission mode: workspace-write"))
        .stdout(predicate::str::contains("Network: local-only"));
}

#[test]
fn prompt_conflicts_with_handoff_command() {
    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .args(["-p", "inspect", "handoff", "export", "session-1"])
        .assert()
        .failure()
        .stderr(predicate::str::contains(
            "--prompt cannot be used together with handoff commands",
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
fn resume_can_run_follow_up_prompt() {
    let temp = tempdir().expect("tempdir");

    let start = Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", temp.path())
        .args(["-p", "inspect the repository layout", "--model", "pm-model"])
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
        .args(["resume", session_id, "-p", "summarize the current session"])
        .assert()
        .success()
        .stdout(predicate::str::contains("resuming session"))
        .stdout(predicate::str::contains(
            "prompt: summarize the current session",
        ))
        .stdout(predicate::str::contains("response: Reader inspected "))
        .stdout(predicate::str::contains(
            "Working summary: Reader inspected ",
        ))
        .stdout(predicate::str::contains(
            "Last completed step: summarize the current session",
        ));
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
        .stdout(predicate::str::contains(
            "Working summary: Reader inspected ",
        ))
        .stdout(predicate::str::contains(
            "Pending steps: Use the matched files",
        ))
        .stdout(predicate::str::contains(
            "Last completed step: inspect the repository layout",
        ))
        .stdout(predicate::str::contains(
            "Next recommendation: Use the matched files",
        ))
        .stdout(predicate::str::contains("response: Reader inspected "));
}

#[test]
fn prompt_mode_shows_tester_result_details() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", temp.path())
        .args([
            "-p",
            "run a build",
            "--model",
            "pm-model",
            "--permission-mode",
            "workspace-write",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "Last result: tester via pm-model - Tester ran `cargo build`",
        ))
        .stdout(predicate::str::contains("Commands run: cargo build"))
        .stdout(predicate::str::contains("Evidence: tool-output:"))
        .stdout(predicate::str::contains(
            "Next recommendation: Inspect the validation output",
        ));
}
