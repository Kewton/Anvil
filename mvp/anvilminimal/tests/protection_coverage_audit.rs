use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy)]
struct ProtectionRule {
    category: &'static str,
    site_predicate: &'static str,
    required_wrapper: &'static str,
    allowlist: &'static [&'static str],
    audit: fn(&AuditCorpus, &ProtectionRule) -> Vec<String>,
}

struct AuditCorpus {
    files: BTreeMap<String, String>,
}

const PROTECTION_RULES: &[ProtectionRule] = &[
    ProtectionRule {
        category: "compile_output_source_of_truth",
        site_predicate: "compile diagnostic parsers",
        required_wrapper: "FullCommandOutput",
        allowlist: &[
            "src/minimal_loop/build_verifier.rs",
            "src/planner/profile.rs",
            "src/planner/profiles/python_cli.rs",
            "src/planner/runner.rs",
        ],
        audit: audit_compile_output_source_of_truth,
    },
    ProtectionRule {
        category: "verify_normalization_boundary",
        site_predicate: "verify command execution",
        required_wrapper: "NormalizedVerifyCommand",
        allowlist: &[
            "src/minimal_loop/verifier_env.rs",
            "src/minimal_loop/build_verifier.rs",
            "src/minimal_loop/completion.rs",
            "src/minimal_loop/loop_run.rs",
            "src/planner/profiles/python_cli.rs",
            "src/planner/verify.rs",
        ],
        audit: audit_verify_normalization_boundary,
    },
    ProtectionRule {
        category: "bounded_execution_chokepoints",
        site_predicate: "provider chat and child process launch",
        required_wrapper: "provider_call / bounded_process",
        allowlist: &["src/provider_call.rs", "src/bounded_process.rs"],
        audit: audit_bounded_execution_chokepoints,
    },
    ProtectionRule {
        category: "terminal_records",
        site_predicate: "CLI command exit paths",
        required_wrapper: "DirectCommandCompletionGuard / tui_command_stop",
        allowlist: &["src/lib.rs", "tests/conformance/mod.rs"],
        audit: audit_terminal_records,
    },
];

#[test]
fn protection_coverage_table_is_green_for_current_tree() {
    let corpus = AuditCorpus::from_manifest_dir();
    let violations = audit_protection_coverage(&corpus);
    assert!(
        violations.is_empty(),
        "protection coverage audit failed:\n{}",
        violations.join("\n")
    );
}

#[test]
fn protection_coverage_table_rejects_unregistered_mock_sites() {
    let corpus = AuditCorpus::from_pairs([
        (
            "src/new_provider.rs",
            r#"fn f(client: &mut dyn Client) { let _ = client.chat("m", &[], &[], false); }"#,
        ),
        (
            "src/new_spawn.rs",
            r#"fn f() { let _ = std::process::Command::new("sh").output(); }"#,
        ),
        (
            "src/new_verify.rs",
            r#"fn f(root: &std::path::Path) { let _ = crate::minimal_loop::verifier_env::run_checked("npm run build", root, false); }"#,
        ),
        (
            "src/new_runtime_bash.rs",
            r#"fn runtime_bash_policy_decision(command: &str) { let _ = crate::planner::verify::diagnose_verify_command(command); }"#,
        ),
        (
            "src/planner/lint.rs",
            r#"fn lint(command: &str) { let _ = crate::planner::verify::validate_verify_command(command); }"#,
        ),
        (
            "src/new_compile.rs",
            r#"fn f(excerpt: &str) { let _ = crate::minimal_loop::build_verifier::parse_compile_errors(excerpt); }"#,
        ),
        (
            "src/config.rs",
            "pub enum Action { Repl, Prompt(String), NewEntry(String) }",
        ),
        (
            "src/lib.rs",
            r#"fn run() {} fn direct_command_for_action(action: &Action) -> Option<&'static str> { match action { Action::Repl => None, Action::Prompt(_) => Some("--prompt") } }"#,
        ),
    ]);
    let violations = audit_protection_coverage(&corpus).join("\n");
    for category in [
        "compile_output_source_of_truth",
        "verify_normalization_boundary",
        "bounded_execution_chokepoints",
        "terminal_records",
    ] {
        assert!(
            violations.contains(category),
            "mock site did not trip {category}; violations:\n{violations}"
        );
    }
}

fn audit_protection_coverage(corpus: &AuditCorpus) -> Vec<String> {
    let mut violations = Vec::new();
    for rule in PROTECTION_RULES {
        for violation in (rule.audit)(corpus, rule) {
            violations.push(format!(
                "{} [{} -> {}]: {}",
                rule.category, rule.site_predicate, rule.required_wrapper, violation
            ));
        }
    }
    violations
}

fn audit_compile_output_source_of_truth(
    corpus: &AuditCorpus,
    rule: &ProtectionRule,
) -> Vec<String> {
    let mut violations = Vec::new();
    let build_verifier = corpus.file("src/minimal_loop/build_verifier.rs");
    if !build_verifier.contains("pub fn parse_compile_errors(output: &FullCommandOutput)") {
        violations.push("parse_compile_errors must require &FullCommandOutput".to_string());
    }
    if build_verifier.contains("pub fn parse_compile_errors(output: &str)") {
        violations.push("parse_compile_errors still accepts a raw string".to_string());
    }
    if corpus
        .file("src/planner/runner.rs")
        .contains("release_evidence_compile_excerpt_fields")
    {
        violations.push("release evidence still has an excerpt-to-parser fallback".to_string());
    }
    for (path, text) in corpus.rust_files() {
        for (line_index, line) in text.lines().enumerate() {
            if line.contains("parse_compile_errors(")
                && !rule.allowlist.contains(&path.as_str())
                && !line.trim_start().starts_with("///")
            {
                violations.push(format!("{path}:{}: {}", line_index + 1, line.trim()));
            }
            if line.contains("parse_compile_errors(") && line.contains("output_excerpt") {
                violations.push(format!(
                    "{path}:{} parses an output excerpt",
                    line_index + 1
                ));
            }
        }
    }
    violations
}

fn audit_verify_normalization_boundary(corpus: &AuditCorpus, rule: &ProtectionRule) -> Vec<String> {
    let mut violations = Vec::new();
    let verifier_env = corpus.file("src/minimal_loop/verifier_env.rs");
    if !verifier_env.contains("command: &NormalizedVerifyCommand") {
        violations
            .push("verifier_env execution APIs must take NormalizedVerifyCommand".to_string());
    }
    let loop_run = corpus.file("src/minimal_loop/loop_run.rs");
    if loop_run.contains("registry.execute_with_cancel(")
        && !(loop_run.contains("runtime_bash_policy_decision(")
            && loop_run.contains("set_bash_command(")
            && loop_run.contains("normalize_runtime_bash_command_for_boundary(")
            && loop_run.contains("execute_split_runtime_bash("))
    {
        violations.push(
            "in-session tool execution boundary is not routed through normalizer".to_string(),
        );
    }
    if loop_run.contains("fn runtime_bash_policy_decision(")
        && !loop_run.contains("normalize_runtime_bash_command_for_boundary(")
    {
        violations
            .push("runtime_bash_policy entry point bypasses shared runtime normalizer".to_string());
    }
    let planner_lint = corpus.file("src/planner/lint.rs");
    if planner_lint.contains("validate_verify_command(")
        && !planner_lint.contains("normalize_planner_verify_command(")
    {
        violations
            .push("StepPlan verify-policy lint bypasses shared planner normalizer".to_string());
    }
    for (path, text) in corpus.rust_files() {
        for (line_index, line) in text.lines().enumerate() {
            if line.contains("runtime_bash_policy_decision(")
                && !rule.allowlist.contains(&path.as_str())
                && !line.trim_start().starts_with("///")
            {
                violations.push(format!(
                    "{path}:{} runtime bash policy entry point is not registered",
                    line_index + 1
                ));
            }
            let calls_executor = line.contains("verifier_env::run_checked(")
                || (path == "src/minimal_loop/verifier_env.rs"
                    && line.contains("pub fn run_checked("))
                || line.contains("run_structured_for_verify_with_profile(");
            if calls_executor
                && !rule.allowlist.contains(&path.as_str())
                && !line.trim_start().starts_with("///")
            {
                violations.push(format!("{path}:{}: {}", line_index + 1, line.trim()));
            }
            if calls_executor && line.contains('"') && !line.trim_start().starts_with("///") {
                violations.push(format!(
                    "{path}:{} passes a string literal to a verifier executor",
                    line_index + 1
                ));
            }
        }
    }
    violations
}

fn audit_bounded_execution_chokepoints(corpus: &AuditCorpus, _: &ProtectionRule) -> Vec<String> {
    let mut violations = Vec::new();
    for (path, text) in corpus.src_rust_files() {
        let mut in_test_mod = false;
        for (line_index, line) in text.lines().enumerate() {
            if path != "src/provider_call.rs" && line.contains(".chat(") {
                violations.push(format!(
                    "{path}:{} direct provider .chat call",
                    line_index + 1
                ));
            }
            if path != "src/bounded_process.rs" && raw_process_invocation(line) && !in_test_mod {
                violations.push(format!(
                    "{path}:{} direct child-process invocation: {}",
                    line_index + 1,
                    line.trim()
                ));
            }
            let trimmed = line.trim();
            if (trimmed.starts_with("mod tests") || trimmed.starts_with("pub mod tests"))
                && line.contains('{')
                && preceding_cfg_test(text, line_index)
            {
                in_test_mod = true;
            }
        }
    }
    violations
}

fn audit_terminal_records(corpus: &AuditCorpus, _: &ProtectionRule) -> Vec<String> {
    let mut violations = Vec::new();
    let lib = corpus.file("src/lib.rs");
    for required in [
        "DirectCommandCompletionGuard::start(&config)",
        "emit_direct_command_stop_with_status",
        "\"event\": \"tui_command_stop\"",
        "\"build_commit\": build_info::COMMIT",
        "\"build_timestamp\": build_info::TIMESTAMP",
    ] {
        if !lib.contains(required) {
            violations.push(format!("missing terminal guard marker `{required}`"));
        }
    }
    for variant in action_variants(corpus.file("src/config.rs")) {
        if !lib.contains(&format!("Action::{variant}")) {
            violations.push(format!(
                "Action::{variant} is not registered in direct_command_for_action"
            ));
        }
    }
    if !corpus
        .file("tests/conformance/mod.rs")
        .contains("conformance_honest_terminal_covers_simulated_panic_exit")
    {
        violations.push("terminal panic exit conformance pathway is missing".to_string());
    }
    violations
}

fn action_variants(config_source: &str) -> Vec<String> {
    let Some(start) = config_source.find("pub enum Action") else {
        return Vec::new();
    };
    let Some(open) = config_source[start..]
        .find('{')
        .map(|index| start + index + 1)
    else {
        return Vec::new();
    };
    let Some(close) = config_source[open..].find('}').map(|index| open + index) else {
        return Vec::new();
    };
    config_source[open..close]
        .lines()
        .filter_map(|line| {
            let token = line
                .trim()
                .trim_end_matches(',')
                .split(['(', '{', ' '])
                .next()
                .unwrap_or_default();
            (!token.is_empty()).then(|| token.to_string())
        })
        .collect()
}

fn raw_process_invocation(line: &str) -> bool {
    if line.contains(".spawn(") && !line.contains(".spawn(move") {
        return true;
    }
    if line.contains(".output(") {
        return true;
    }
    line.contains(".status()") && !line.contains("response.status()")
}

fn preceding_cfg_test(text: &str, line_index: usize) -> bool {
    text.lines()
        .take(line_index)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .take_while(|line| line.trim().is_empty() || line.trim().starts_with("#["))
        .any(|line| line.trim() == "#[cfg(test)]")
}

impl AuditCorpus {
    fn from_manifest_dir() -> Self {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = BTreeMap::new();
        for dir in ["src", "tests"] {
            for path in rust_files(&root.join(dir)) {
                let relative = path
                    .strip_prefix(root)
                    .expect("relative source path")
                    .to_string_lossy()
                    .replace('\\', "/");
                let text = fs::read_to_string(&path)
                    .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
                files.insert(relative, text);
            }
        }
        Self { files }
    }

    fn from_pairs<const N: usize>(pairs: [(&str, &str); N]) -> Self {
        Self {
            files: pairs
                .into_iter()
                .map(|(path, text)| (path.to_string(), text.to_string()))
                .collect(),
        }
    }

    fn file(&self, path: &str) -> &str {
        self.files.get(path).map(String::as_str).unwrap_or("")
    }

    fn rust_files(&self) -> impl Iterator<Item = (String, &str)> {
        self.files
            .iter()
            .filter(|(path, _)| {
                path.ends_with(".rs") && path.as_str() != "tests/protection_coverage_audit.rs"
            })
            .map(|(path, text)| (path.clone(), text.as_str()))
    }

    fn src_rust_files(&self) -> impl Iterator<Item = (String, &str)> {
        self.rust_files()
            .filter(|(path, _)| path.starts_with("src/"))
    }
}

fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    collect_rust_files(dir, &mut out);
    out
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap_or_else(|err| panic!("failed to read {dir:?}: {err}")) {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}
