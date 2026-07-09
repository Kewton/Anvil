use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use serde_json::json;

use crate::bounded_process::{self, BoundedProcessOutcomeKind};
use crate::eval_events;
use crate::minimal_loop::build_verifier::{
    BuildVerifierRequirement, CompileError, FullCommandOutput,
};
use crate::minimal_loop::dependency_setup::{
    self, NodeDependencySetupAuthority, NodeDependencySetupRequirement,
};
use crate::minimal_loop::verifier_env;
use crate::planner::profile::{
    DomainProfile, ProfileBehaviorProbeReport, ProfileBuildOracle, ProfileQualityExpectations,
    ProfileSnapshot, profile_failure,
};
use crate::planner::verify::{
    NormalizedVerifyCommand, VerificationReport, normalize_verify_command,
};

const DEFAULT_PACKAGE: &str = "anvil_app";
const COMPILE_COMMAND: &str = "python -m compileall -q src";
const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

pub struct PythonCliProfile;

impl DomainProfile for PythonCliProfile {
    fn id(&self) -> &'static str {
        "python-cli"
    }

    fn matches(&self, profile: &str) -> bool {
        matches!(
            crate::planner::profile::canonical_profile_name(profile).as_str(),
            "python-cli"
        )
    }

    fn expected_scaffold_paths(&self, root: &Path, _goal: &str) -> Vec<String> {
        scaffold_paths(root)
    }

    fn setup_scaffold_paths(&self, root: &Path) -> Vec<String> {
        scaffold_paths(root)
    }

    fn before_phase(&self, root: &Path) -> anyhow::Result<ProfileSnapshot> {
        let scaffold_paths = scaffold_paths(root);
        if !scaffold_paths.is_empty() {
            let _ = complete_scaffold(root, &scaffold_paths)?;
        }
        Ok(ProfileSnapshot::None)
    }

    fn complete_scaffold(
        &self,
        root: &Path,
        missing_paths: &[String],
    ) -> anyhow::Result<Vec<String>> {
        complete_scaffold(root, missing_paths)
    }

    fn verify_final(&self, root: &Path, _goal: &str) -> VerificationReport {
        let invariant = verify_invariant_contract(root);
        if !invariant.is_pass() {
            return invariant;
        }
        compile_report(root)
    }

    fn verify_invariant(
        &self,
        root: &Path,
        _goal: &str,
        _snapshot: &ProfileSnapshot,
    ) -> VerificationReport {
        verify_invariant_contract(root)
    }

    fn guidance(&self, _goal: &str) -> Option<String> {
        Some(
            "For the python-cli profile, create a small Python CLI package with pyproject.toml and src/<package>/main.py. Keep deterministic verification separate from dependency setup. The CLI must read stdin or args and print non-empty output that changes when input changes."
                .to_string(),
        )
    }

    fn runtime_contract(&self, _intent: &str, _goal: &str) -> String {
        "- Preserve pyproject.toml and src/<package>/main.py.\n\
- Keep dependency setup separate from deterministic verification.\n\
- Verify syntax with python -m compileall -q src.\n\
- The CLI must exit 0 and print non-empty output that changes when stdin or args change."
            .to_string()
    }

    fn generation_rules(&self, _intent: &str) -> Option<&'static str> {
        Some(
            "- Profile python-cli: create pyproject.toml plus src/<package>/main.py. Prefer stdlib-only code unless dependencies are explicitly required. Use python -m compileall -q src for verification. Do not put pip install in verify commands; dependency setup belongs in setup phases only. The CLI should read stdin or argv and print changed output for changed input.\n",
        )
    }

    fn quality_expectations(&self, root: &Path, _goal: &str) -> ProfileQualityExpectations {
        ProfileQualityExpectations {
            required_artifacts: scaffold_paths(root),
            preferred_verify: vec![COMPILE_COMMAND.to_string()],
            forbidden_verify: vec!["pip install".to_string(), "python -m venv".to_string()],
            dependency_order_hint: Some(
                "Create pyproject.toml and src/<package>/main.py before python -m compileall -q src"
                    .to_string(),
            ),
        }
    }

    fn post_step_repair(&self, root: &Path, _goal: &str) -> anyhow::Result<bool> {
        make_entrypoint_executable(root)
    }

    fn build_oracle(&self, command: &str) -> Option<ProfileBuildOracle> {
        let normalized = command.split_whitespace().collect::<Vec<_>>().join(" ");
        let lower = normalized.to_ascii_lowercase();
        let recognized = lower == COMPILE_COMMAND
            || lower.starts_with("python -m compileall ")
            || lower.starts_with("python3 -m compileall ")
            || lower.starts_with("python -m py_compile ")
            || lower.starts_with("python3 -m py_compile ");
        recognized.then(|| ProfileBuildOracle {
            command: normalized,
            profile: Some(self.id().to_string()),
            requires_dependency_setup: true,
        })
    }

    fn dependency_ready(&self, root: &Path, _command: &str) -> bool {
        dependency_setup::python_cli_dependencies_ready(root)
    }

    fn dependency_missing_reason(&self, root: &Path, _command: &str) -> String {
        dependency_setup::python_cli_dependencies_missing_reason(root)
    }

    fn dependency_setup_requirement(
        &self,
        root: &Path,
        requirement: &BuildVerifierRequirement,
        setup_authority: NodeDependencySetupAuthority,
    ) -> Option<NodeDependencySetupRequirement> {
        Some(dependency_setup::requirement_for_python_cli_dependencies(
            root,
            requirement.profile.as_deref(),
            &requirement.reason,
            setup_authority,
        ))
    }

    fn dependency_missing_output(&self, output: &str) -> bool {
        let lower = output.to_ascii_lowercase();
        lower.contains("modulenotfounderror")
            || lower.contains("no module named")
            || lower.contains("command not found")
            || lower.contains("no such file or directory")
    }

    fn parse_compile_errors(&self, output: &FullCommandOutput) -> Vec<CompileError> {
        parse_python_compile_errors(output.as_str())
    }

    fn infer_required_capabilities(&self, _goal: &str) -> Vec<String> {
        vec!["input_output_contract".to_string()]
    }

    fn infer_required_evidence(
        &self,
        _goal: &str,
        _required_capabilities: &[String],
    ) -> Vec<String> {
        Vec::new()
    }

    fn infer_required_obligations(
        &self,
        _goal: &str,
        _required_capabilities: &[String],
    ) -> Vec<String> {
        vec!["implementation".to_string()]
    }

    fn completion_contract_required(&self, _goal: &str, required_capabilities: &[String]) -> bool {
        required_capabilities
            .iter()
            .any(|capability| capability == "input_output_contract")
    }

    fn behavior_probe(
        &self,
        root: &Path,
        goal: &str,
        _required_capabilities: &[String],
        offline: bool,
    ) -> anyhow::Result<ProfileBehaviorProbeReport> {
        if offline && dependency_setup::python_cli_declares_dependencies(root) {
            return write_behavior_report(
                root,
                "failed",
                vec!["python_cli_behavior_probe_blocked_offline_dependencies".to_string()],
                None,
            );
        }
        let Some(entrypoint) = entrypoint_path(root) else {
            return write_behavior_report(
                root,
                "failed",
                vec!["python_cli_behavior_probe_failed:entrypoint_missing".to_string()],
                None,
            );
        };
        if goal_requests_csv_file_probe(goal) {
            return run_csv_file_behavior_probe(root, &entrypoint);
        }
        let first = run_cli(root, &entrypoint, &[], "anvil\n")?;
        let second = run_cli(root, &entrypoint, &[], "profile\n")?;
        let mut reasons = Vec::new();
        if first.exit_code != Some(0) {
            reasons.push(format!(
                "python_cli_behavior_probe_failed:first_exit_code:{:?}",
                first.exit_code
            ));
        }
        if second.exit_code != Some(0) {
            reasons.push(format!(
                "python_cli_behavior_probe_failed:second_exit_code:{:?}",
                second.exit_code
            ));
        }
        if first.stdout.trim().is_empty() || second.stdout.trim().is_empty() {
            reasons.push("python_cli_behavior_probe_failed:stdout_empty".to_string());
        }
        if first.stdout == second.stdout {
            reasons
                .push("python_cli_behavior_probe_failed:stdout_not_changed_by_input".to_string());
        }
        let details = json!({
            "entrypoint": entrypoint.to_string_lossy(),
            "first_exit_code": first.exit_code,
            "second_exit_code": second.exit_code,
            "first_stdout": eval_events::body_snippet(&first.stdout),
            "second_stdout": eval_events::body_snippet(&second.stdout),
            "first_stderr": eval_events::body_snippet(&first.stderr),
            "second_stderr": eval_events::body_snippet(&second.stderr),
            "changed_by_input": first.stdout != second.stdout,
        });
        if reasons.is_empty() {
            write_behavior_report(root, "pass", Vec::new(), Some(details))
        } else {
            write_behavior_report(root, "failed", reasons, Some(details))
        }
    }
}

struct CsvProbeFixture {
    display_path: String,
    content: &'static str,
    expected_tokens: &'static [&'static str],
}

fn goal_requests_csv_file_probe(goal: &str) -> bool {
    let lower = goal.to_ascii_lowercase();
    lower.contains("csv")
        || goal.contains("CSV")
        || goal.contains("ＣＳＶ")
        || goal.contains("ファイル")
            && (goal.contains("合計")
                || goal.contains("平均")
                || goal.contains("最大")
                || goal.contains("最小")
                || goal.contains("数値"))
}

fn run_csv_file_behavior_probe(
    root: &Path,
    entrypoint: &Path,
) -> anyhow::Result<ProfileBehaviorProbeReport> {
    let fixtures = write_csv_probe_fixtures(root)?;
    let first = run_cli(root, entrypoint, &[fixtures[0].display_path.as_str()], "")?;
    let second = run_cli(root, entrypoint, &[fixtures[1].display_path.as_str()], "")?;
    let mut reasons = Vec::new();
    if first.exit_code != Some(0) {
        reasons.push(format!(
            "python_cli_behavior_probe_failed:first_exit_code:{:?}",
            first.exit_code
        ));
    }
    if second.exit_code != Some(0) {
        reasons.push(format!(
            "python_cli_behavior_probe_failed:second_exit_code:{:?}",
            second.exit_code
        ));
    }
    if first.stdout.trim().is_empty() || second.stdout.trim().is_empty() {
        reasons.push("python_cli_behavior_probe_failed:stdout_empty".to_string());
    }
    if first.stdout == second.stdout {
        reasons.push("python_cli_behavior_probe_failed:stdout_not_changed_by_input".to_string());
    }
    for token in fixtures[0].expected_tokens {
        if !first.stdout.contains(token) {
            reasons.push(format!(
                "python_cli_behavior_probe_failed:first_stdout_missing_aggregate:{token}"
            ));
        }
    }
    for token in fixtures[1].expected_tokens {
        if !second.stdout.contains(token) {
            reasons.push(format!(
                "python_cli_behavior_probe_failed:second_stdout_missing_aggregate:{token}"
            ));
        }
    }
    if !contains_any_aggregate_label(&first.stdout) {
        reasons.push(
            "python_cli_behavior_probe_failed:first_stdout_missing_aggregate_label".to_string(),
        );
    }
    if !contains_any_aggregate_label(&second.stdout) {
        reasons.push(
            "python_cli_behavior_probe_failed:second_stdout_missing_aggregate_label".to_string(),
        );
    }
    let details = json!({
        "mode": "csv_file_arg",
        "entrypoint": entrypoint.to_string_lossy(),
        "first_fixture_csv": fixtures[0].display_path.clone(),
        "second_fixture_csv": fixtures[1].display_path.clone(),
        "first_fixture_content": fixtures[0].content,
        "second_fixture_content": fixtures[1].content,
        "first_expected_aggregate_tokens": fixtures[0].expected_tokens,
        "second_expected_aggregate_tokens": fixtures[1].expected_tokens,
        "first_exit_code": first.exit_code,
        "second_exit_code": second.exit_code,
        "first_stdout": eval_events::body_snippet(&first.stdout),
        "second_stdout": eval_events::body_snippet(&second.stdout),
        "first_stderr": eval_events::body_snippet(&first.stderr),
        "second_stderr": eval_events::body_snippet(&second.stderr),
        "changed_by_input": first.stdout != second.stdout,
        "argv_invocation": true,
    });
    if reasons.is_empty() {
        write_behavior_report(root, "pass", Vec::new(), Some(details))
    } else {
        write_behavior_report(root, "failed", reasons, Some(details))
    }
}

fn write_csv_probe_fixtures(root: &Path) -> anyhow::Result<Vec<CsvProbeFixture>> {
    let dir = root
        .join(".anvil")
        .join("evidence")
        .join("python-cli-fixtures");
    std::fs::create_dir_all(&dir)?;
    let specs = [
        (
            "input-a.csv",
            "name,score,count\nalpha,10,2\nbeta,20,4\ngamma,30,6\n",
            &[
                "score", "60", "20", "30", "10", "count", "12", "4", "6", "2",
            ][..],
        ),
        (
            "input-b.csv",
            "name,score,count\nalpha,3,1\nbeta,9,5\ngamma,12,7\n",
            &["score", "24", "8", "12", "3", "count", "13", "7", "1"][..],
        ),
    ];
    let mut fixtures = Vec::with_capacity(specs.len());
    for (file_name, content, expected_tokens) in specs {
        let path = dir.join(file_name);
        std::fs::write(&path, content)?;
        fixtures.push(CsvProbeFixture {
            display_path: display_path(root, &path),
            content,
            expected_tokens,
        });
    }
    Ok(fixtures)
}

fn contains_any_aggregate_label(stdout: &str) -> bool {
    let lower = stdout.to_ascii_lowercase();
    ["sum", "total", "avg", "average", "mean", "max", "min"]
        .iter()
        .any(|label| lower.contains(label))
        || stdout.contains("合計")
        || stdout.contains("平均")
        || stdout.contains("最大")
        || stdout.contains("最小")
}

fn scaffold_paths(root: &Path) -> Vec<String> {
    vec![
        "pyproject.toml".to_string(),
        format!("src/{}/main.py", package_name(root)),
    ]
}

pub fn complete_scaffold(root: &Path, missing_paths: &[String]) -> anyhow::Result<Vec<String>> {
    let mut created = Vec::new();
    let entrypoint_rel = missing_paths
        .iter()
        .find(|path| python_entrypoint_rel(path))
        .cloned()
        .unwrap_or_else(|| format!("src/{}/main.py", package_name(root)));
    let package = entrypoint_rel
        .strip_prefix("src/")
        .and_then(|tail| tail.strip_suffix("/main.py"))
        .filter(|name| safe_python_package_name(name))
        .unwrap_or(DEFAULT_PACKAGE);

    for rel in missing_paths {
        if rel == "pyproject.toml" {
            if write_absent(&root.join(rel), &canonical_pyproject(package))? {
                created.push(rel.clone());
            }
            continue;
        }
        if python_entrypoint_rel(rel) && write_absent(&root.join(rel), canonical_main_py())? {
            make_entrypoint_executable(root)?;
            created.push(rel.clone());
        }
    }
    Ok(created)
}

fn python_entrypoint_rel(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.starts_with("src/")
        && normalized.ends_with("/main.py")
        && normalized
            .strip_prefix("src/")
            .and_then(|tail| tail.strip_suffix("/main.py"))
            .is_some_and(safe_python_package_name)
}

fn safe_python_package_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        && !name.chars().next().is_some_and(|ch| ch.is_ascii_digit())
}

fn write_absent(path: &Path, content: &str) -> anyhow::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(true)
}

fn canonical_pyproject(package: &str) -> String {
    let script = package.replace('_', "-");
    format!(
        "[project]\nname = \"{script}\"\nversion = \"0.1.0\"\nrequires-python = \">=3.9\"\ndependencies = []\n\n[project.scripts]\n{script} = \"{package}.main:main\"\n"
    )
}

fn canonical_main_py() -> &'static str {
    r#"#!/usr/bin/env python3
import sys


def main() -> int:
    data = " ".join(sys.argv[1:]).strip() or sys.stdin.read().strip()
    if not data:
        data = "ready"
    print(f"anvil_app: {data}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
"#
}

fn verify_invariant_contract(root: &Path) -> VerificationReport {
    if !root.join("pyproject.toml").is_file() {
        return profile_failure("pyproject.toml missing");
    }
    let Some(entrypoint) = entrypoint_path(root) else {
        return profile_failure(format!(
            "Python CLI entrypoint missing: expected {}",
            scaffold_paths(root)
                .get(1)
                .cloned()
                .unwrap_or_else(|| "src/<package>/main.py".to_string())
        ));
    };
    if !entrypoint.is_file() {
        return profile_failure("Python CLI entrypoint missing");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let Ok(metadata) = std::fs::metadata(&entrypoint) else {
            return profile_failure("Python CLI entrypoint unreadable");
        };
        if metadata.permissions().mode() & 0o111 == 0 {
            return profile_failure(format!(
                "Python CLI entrypoint not executable: {}",
                display_path(root, &entrypoint)
            ));
        }
    }
    VerificationReport::pass()
}

fn compile_report(root: &Path) -> VerificationReport {
    let normalized_command: NormalizedVerifyCommand =
        normalize_verify_command(COMPILE_COMMAND).expect("compile command");
    match verifier_env::run_checked(&normalized_command, root, false) {
        Ok(_) => VerificationReport::pass(),
        Err(err) => {
            let text = err.to_string();
            let full_output =
                FullCommandOutput::from_bounded_executor(root, COMPILE_COMMAND, &text);
            let errors = parse_python_compile_errors(full_output.as_str());
            let mut report = VerificationReport::pass();
            if errors.is_empty() {
                report.push_command_failure(COMPILE_COMMAND, text);
            } else {
                report.push_compile_errors(COMPILE_COMMAND, errors);
            }
            report
        }
    }
}

fn make_entrypoint_executable(root: &Path) -> anyhow::Result<bool> {
    let Some(entrypoint) = entrypoint_path(root) else {
        return Ok(false);
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let metadata = std::fs::metadata(&entrypoint)?;
        let mut permissions = metadata.permissions();
        let mode = permissions.mode();
        if mode & 0o111 != 0 {
            return Ok(false);
        }
        permissions.set_mode(mode | 0o755);
        std::fs::set_permissions(entrypoint, permissions)?;
        Ok(true)
    }
    #[cfg(not(unix))]
    {
        let _ = entrypoint;
        Ok(false)
    }
}

fn package_name(root: &Path) -> String {
    pyproject_name(root)
        .map(|name| python_package_from_project_name(&name))
        .or_else(|| first_src_package(root))
        .unwrap_or_else(|| DEFAULT_PACKAGE.to_string())
}

fn pyproject_name(root: &Path) -> Option<String> {
    let raw = std::fs::read_to_string(root.join("pyproject.toml")).ok()?;
    let mut in_project = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            in_project = trimmed == "[project]";
            continue;
        }
        if in_project
            && trimmed.starts_with("name")
            && let Some((_, value)) = trimmed.split_once('=')
        {
            let name = value.trim().trim_matches('"').trim_matches('\'');
            if !name.is_empty() {
                return Some(name.to_string());
            }
        }
    }
    None
}

fn python_package_from_project_name(name: &str) -> String {
    let mut out = name
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    if out.chars().next().is_some_and(|ch| ch.is_ascii_digit()) {
        out.insert(0, '_');
    }
    if out.is_empty() {
        DEFAULT_PACKAGE.to_string()
    } else {
        out
    }
}

fn first_src_package(root: &Path) -> Option<String> {
    let entries = std::fs::read_dir(root.join("src")).ok()?;
    let mut packages = entries
        .flatten()
        .filter(|entry| entry.path().join("main.py").is_file())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
        .collect::<Vec<_>>();
    packages.sort();
    packages.into_iter().next()
}

fn entrypoint_path(root: &Path) -> Option<PathBuf> {
    let expected = root.join("src").join(package_name(root)).join("main.py");
    if expected.is_file() {
        return Some(expected);
    }
    let package = first_src_package(root)?;
    Some(root.join("src").join(package).join("main.py"))
}

fn parse_python_compile_errors(output: &str) -> Vec<CompileError> {
    let lines = strip_ansi(output)
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    let mut errors = Vec::new();
    let mut index = 0usize;
    while index < lines.len() {
        let line = lines[index].trim();
        if let Some((path, line_number)) = parse_python_file_line(line) {
            let message = lines
                .iter()
                .skip(index + 1)
                .find_map(|line| {
                    let trimmed = line.trim();
                    trimmed
                        .starts_with("SyntaxError:")
                        .then(|| trimmed.to_string())
                        .or_else(|| {
                            trimmed
                                .starts_with("IndentationError:")
                                .then(|| trimmed.to_string())
                        })
                        .or_else(|| {
                            trimmed
                                .starts_with("TabError:")
                                .then(|| trimmed.to_string())
                        })
                })
                .unwrap_or_else(|| "SyntaxError: invalid syntax".to_string());
            let excerpt = lines
                .iter()
                .skip(index + 1)
                .take(4)
                .map(String::as_str)
                .collect::<Vec<_>>()
                .join("\n");
            push_compile_error(
                &mut errors,
                CompileError {
                    path,
                    line: line_number,
                    column: 0,
                    message,
                    excerpt,
                    symbol: None,
                    route_bound: None,
                },
            );
        }
        index += 1;
    }
    errors
}

fn parse_python_file_line(line: &str) -> Option<(String, usize)> {
    let line = line.trim();
    let start = line.find("File \"")? + "File \"".len();
    let rest = &line[start..];
    let end = rest.find('"')?;
    let path = rest[..end].trim_start_matches("./").replace('\\', "/");
    let after = &rest[end + 1..];
    let marker = "line ";
    let line_start = after.find(marker)? + marker.len();
    let line_number = after[line_start..]
        .split(|ch: char| !ch.is_ascii_digit())
        .next()?
        .parse::<usize>()
        .ok()?;
    Some((path, line_number))
}

fn push_compile_error(errors: &mut Vec<CompileError>, error: CompileError) {
    if !errors.iter().any(|existing| {
        existing.path == error.path
            && existing.line == error.line
            && existing.message == error.message
    }) {
        errors.push(error);
    }
}

fn strip_ansi(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\u{1b}' && chars.peek() == Some(&'[') {
            let _ = chars.next();
            for next in chars.by_ref() {
                if next.is_ascii_alphabetic() {
                    break;
                }
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[derive(Debug, Clone)]
struct CliRun {
    exit_code: Option<i32>,
    stdout: String,
    stderr: String,
}

fn run_cli(
    root: &Path,
    entrypoint: &Path,
    args: &[&str],
    stdin_text: &str,
) -> anyhow::Result<CliRun> {
    let python = python_interpreter(root);
    let mut command = verifier_env::normalized_command_at_root(python, root);
    command
        .arg(entrypoint)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = bounded_process::spawn_child(&mut command)?;
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(stdin_text.as_bytes())?;
    }
    drop(child.stdin.take());
    let output = bounded_process::wait_with_timeout(child, PROBE_TIMEOUT)?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    if output.kind == BoundedProcessOutcomeKind::TimedOut {
        return Ok(CliRun {
            exit_code: None,
            stdout,
            stderr: "python_cli_behavior_probe_timeout\n".to_string() + &stderr,
        });
    }
    Ok(CliRun {
        exit_code: output.status.and_then(|status| status.code()),
        stdout,
        stderr,
    })
}

fn python_interpreter(root: &Path) -> PathBuf {
    let venv = dependency_setup::python_cli_venv_python(root);
    if venv.is_file() {
        venv
    } else {
        PathBuf::from("python")
    }
}

fn write_behavior_report(
    root: &Path,
    status: &'static str,
    reasons: Vec<String>,
    details: Option<serde_json::Value>,
) -> anyhow::Result<ProfileBehaviorProbeReport> {
    let dir = root.join(".anvil").join("evidence");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join("python-cli-behavior.json");
    let value = json!({
        "profile": "python-cli",
        "status": status,
        "ok": status == "pass",
        "reasons": reasons,
        "details": details.unwrap_or_else(|| json!({})),
    });
    std::fs::write(
        &path,
        format!("{}\n", serde_json::to_string_pretty(&value)?),
    )?;
    Ok(ProfileBehaviorProbeReport {
        status,
        reasons: value
            .get("reasons")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(serde_json::Value::as_str)
            .map(str::to_string)
            .collect(),
        evidence_path: Some(display_path(root, &path)),
    })
}

fn display_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn python_compile_parser_extracts_syntax_error() {
        let output = r#"
*** Error compiling 'src/demo/main.py'...
  File "src/demo/main.py", line 2
    if True print("x")
            ^^^^^
SyntaxError: invalid syntax
"#;
        let errors = parse_python_compile_errors(output);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].path, "src/demo/main.py");
        assert_eq!(errors[0].line, 2);
        assert_eq!(errors[0].message, "SyntaxError: invalid syntax");
    }

    #[test]
    fn package_name_normalizes_project_name() {
        assert_eq!(python_package_from_project_name("hello-cli"), "hello_cli");
        assert_eq!(python_package_from_project_name("7-tool"), "_7_tool");
    }

    #[test]
    fn complete_scaffold_creates_python_cli_pyproject_and_entrypoint() {
        let dir = tempfile::tempdir().unwrap();

        let created = complete_scaffold(
            dir.path(),
            &[
                "pyproject.toml".to_string(),
                "src/csv_stats/main.py".to_string(),
            ],
        )
        .unwrap();

        assert_eq!(
            created,
            vec![
                "pyproject.toml".to_string(),
                "src/csv_stats/main.py".to_string()
            ]
        );
        let pyproject = std::fs::read_to_string(dir.path().join("pyproject.toml")).unwrap();
        assert!(pyproject.contains("name = \"csv-stats\""), "{pyproject}");
        assert!(
            pyproject.contains("csv-stats = \"csv_stats.main:main\""),
            "{pyproject}"
        );
        let main = std::fs::read_to_string(dir.path().join("src/csv_stats/main.py")).unwrap();
        assert!(main.contains("def main()"), "{main}");
        assert!(verify_invariant_contract(dir.path()).is_pass());
    }

    #[test]
    fn before_phase_pre_provisions_python_cli_scaffold() {
        let dir = tempfile::tempdir().unwrap();

        crate::planner::profile::profile_before_phase(dir.path(), "python-cli").unwrap();

        assert!(dir.path().join("pyproject.toml").is_file());
        assert!(dir.path().join("src/anvil_app/main.py").is_file());
    }

    #[test]
    fn complete_scaffold_never_overwrites_python_cli_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/anvil_app")).unwrap();
        let pyproject = "[project]\nname = \"custom-app\"\nversion = \"9.9.9\"\n";
        let main = "print('custom')\n";
        std::fs::write(dir.path().join("pyproject.toml"), pyproject).unwrap();
        std::fs::write(dir.path().join("src/anvil_app/main.py"), main).unwrap();

        let created = complete_scaffold(
            dir.path(),
            &[
                "pyproject.toml".to_string(),
                "src/anvil_app/main.py".to_string(),
            ],
        )
        .unwrap();

        assert!(created.is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("pyproject.toml")).unwrap(),
            pyproject
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/anvil_app/main.py")).unwrap(),
            main
        );
    }
}
