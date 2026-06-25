#[test]
#[ignore]
fn tui_pty_smoke() {
    if std::env::var("ANVIL_PTY_TESTS").ok().as_deref() != Some("1") {
        return;
    }
    if cfg!(windows) {
        return;
    }
    let bin = env!("CARGO_BIN_EXE_anvilminimal");
    let tmp = tempfile::tempdir().unwrap();
    let output = run_script_bsd(bin, tmp.path()).or_else(|_| run_script_linux(bin, tmp.path()));
    let output = output.expect("script(1) PTY helper must be available for release/manual UAT");
    let text = String::from_utf8_lossy(&output.stdout).to_string()
        + &String::from_utf8_lossy(&output.stderr);
    assert!(
        text.contains("anvil>"),
        "PTY output did not contain prompt. output={text:?}"
    );
    assert!(
        text.contains("local-first agent") || text.contains("anvilminimal"),
        "PTY output did not contain startup banner. output={text:?}"
    );
}

fn run_script_bsd(bin: &str, cwd: &std::path::Path) -> std::io::Result<std::process::Output> {
    let mut child = std::process::Command::new("script")
        .arg("-q")
        .arg("/dev/null")
        .arg(bin)
        .arg("--yes")
        .arg("--cwd")
        .arg(cwd)
        .arg("--no-footer")
        .env("ANVIL_NO_SPINNER", "1")
        .env("ANVIL_NO_INTERRUPT", "1")
        .env("ANVIL_NO_MARKDOWN", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    use std::io::Write;
    child.stdin.as_mut().unwrap().write_all(b"/exit\n")?;
    child.wait_with_output()
}

fn run_script_linux(bin: &str, cwd: &std::path::Path) -> std::io::Result<std::process::Output> {
    let command = format!("{} --yes --cwd {} --no-footer", bin, cwd.to_string_lossy());
    let mut child = std::process::Command::new("script")
        .arg("-q")
        .arg("-c")
        .arg(command)
        .arg("/dev/null")
        .env("ANVIL_NO_SPINNER", "1")
        .env("ANVIL_NO_INTERRUPT", "1")
        .env("ANVIL_NO_MARKDOWN", "1")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    use std::io::Write;
    child.stdin.as_mut().unwrap().write_all(b"/exit\n")?;
    child.wait_with_output()
}
