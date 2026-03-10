use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::process::Command as ProcessCommand;
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
fn interactive_mode_accepts_multiple_prompts_from_stdin() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", temp.path())
        .args(["--model", "pm-model"])
        .write_stdin("inspect the repository layout\nquit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "interactive commands: enter a prompt, or `exit` to finish",
        ))
        .stdout(predicate::str::contains(
            "prompt: inspect the repository layout",
        ))
        .stdout(predicate::str::contains("response: Reader inspected "))
        .stdout(predicate::str::contains("awaiting next prompt"))
        .stdout(predicate::str::contains("interactive mode ended"));
}

#[test]
fn interactive_mode_supports_help_status_and_models_commands() {
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
        .write_stdin("/help\n/status\n/models\n/history\n/quit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "interactive commands: `/help`, `/status`, `/snapshot`, `/models`, `/history`, `/exit`",
        ))
        .stdout(predicate::str::contains("Objective: interactive session"))
        .stdout(predicate::str::contains("/sessions/"))
        .stdout(predicate::str::contains(
            "Working summary: interactive session",
        ))
        .stdout(predicate::str::contains("Session history is empty"))
        .stdout(predicate::str::contains("PM: pm-model"))
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
fn resumed_session_accepts_interactive_follow_up_from_stdin() {
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
        .args(["resume", session_id])
        .write_stdin("summarize the current session\nexit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("resuming session"))
        .stdout(predicate::str::contains(
            "prompt: summarize the current session",
        ))
        .stdout(predicate::str::contains("response: Reader inspected "))
        .stdout(predicate::str::contains("interactive mode ended"));
}

#[test]
fn resumed_session_status_command_shows_existing_snapshot() {
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
        .args(["resume", session_id])
        .write_stdin("/status\n/exit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("resuming session"))
        .stdout(predicate::str::contains(
            "Objective: inspect the repository layout",
        ))
        .stdout(predicate::str::contains(
            "Working summary: Reader inspected ",
        ))
        .stdout(predicate::str::contains("awaiting next prompt"))
        .stdout(predicate::str::contains("interactive mode ended"));
}

#[test]
fn resumed_session_history_command_shows_results_and_delegations() {
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
        .args(["resume", session_id])
        .write_stdin("/history\n/exit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Recent results:"))
        .stdout(predicate::str::contains(
            "- reader via pm-model: Reader inspected ",
        ))
        .stdout(predicate::str::contains("Recent delegations:"))
        .stdout(predicate::str::contains(
            "- reader via pm-model: inspect the repository layout",
        ));
}

#[test]
fn natural_language_status_still_routes_as_prompt() {
    let temp = tempdir().expect("tempdir");

    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", temp.path())
        .args(["--model", "pm-model"])
        .write_stdin("inspect status of the repository\n/exit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "prompt: inspect status of the repository",
        ))
        .stdout(predicate::str::contains("response: Reader inspected "));
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

#[test]
fn e2e_resume_flow_inspects_mutates_and_reviews_fixture_repo() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path().join("anvil-home");
    fs::create_dir_all(&home).expect("create anvil home");
    fs::write(temp.path().join("sample.rs"), "fn main() {}\n").expect("write sample");

    let init = ProcessCommand::new("git")
        .args(["init"])
        .current_dir(temp.path())
        .output()
        .expect("git init");
    assert!(init.status.success());
    let config_name = ProcessCommand::new("git")
        .args(["config", "user.name", "Anvil Test"])
        .current_dir(temp.path())
        .output()
        .expect("git config user.name");
    assert!(config_name.status.success());
    let config_email = ProcessCommand::new("git")
        .args(["config", "user.email", "anvil@example.test"])
        .current_dir(temp.path())
        .output()
        .expect("git config user.email");
    assert!(config_email.status.success());
    let add = ProcessCommand::new("git")
        .args(["add", "sample.rs"])
        .current_dir(temp.path())
        .output()
        .expect("git add");
    assert!(add.status.success());
    let commit = ProcessCommand::new("git")
        .args(["commit", "-m", "initial fixture"])
        .current_dir(temp.path())
        .output()
        .expect("git commit");
    assert!(commit.status.success());

    let start = Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", &home)
        .current_dir(temp.path())
        .args([
            "-p",
            "inspect sample",
            "--model",
            "pm-model",
            "--permission-mode",
            "workspace-write",
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
        .env("ANVIL_HOME", &home)
        .current_dir(temp.path())
        .args(["resume", session_id, "-p", "apply update file sample"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "response: Editor applied a bounded mutation",
        ))
        .stdout(predicate::str::contains("Changed files:"))
        .stdout(predicate::str::contains(
            "Next recommendation: Run a focused tester pass",
        ));

    let updated = fs::read_to_string(temp.path().join("sample.rs")).expect("read sample");
    assert!(updated.contains("anvil-mvp: apply update file sample"));

    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", &home)
        .current_dir(temp.path())
        .args(["resume", session_id, "-p", "run a build"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "response: Tester ran `cargo build`",
        ))
        .stdout(predicate::str::contains("Commands run: cargo build"))
        .stdout(predicate::str::contains("Evidence: tool-output:"));

    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", &home)
        .current_dir(temp.path())
        .args(["resume", session_id, "-p", "review the current diff"])
        .assert()
        .success()
        .stdout(predicate::str::contains(
            "response: Reviewer prepared a risk pass for review the current diff across 1 changed files",
        ))
        .stdout(predicate::str::contains("Last delegation: reviewer via pm-model"));
}

#[test]
fn handoff_export_and_import_roundtrip_via_cli() {
    let temp = tempdir().expect("tempdir");
    let home = temp.path().join("anvil-home");
    fs::create_dir_all(&home).expect("create anvil home");

    let start = Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", &home)
        .args(["-p", "inspect sample", "--model", "pm-model"])
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

    let export = Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", &home)
        .args(["handoff", "export", session_id])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();

    let export_path = temp.path().join("handoff.json");
    fs::write(&export_path, export).expect("write handoff export");

    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", &home)
        .args([
            "handoff",
            "import",
            export_path.to_str().expect("utf8 path"),
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("imported handoff into"));

    Command::new(assert_cmd::cargo::cargo_bin!("anvil"))
        .env("ANVIL_HOME", &home)
        .args(["resume", session_id])
        .write_stdin("/status\n/exit\n")
        .assert()
        .success()
        .stdout(predicate::str::contains("Objective: inspect sample"))
        .stdout(predicate::str::contains(
            "Working summary: Reader inspected ",
        ));
}
