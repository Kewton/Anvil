use std::collections::BTreeMap;
use std::path::Path;

use anvilminimal::minimal_loop::evidence::verify_runtime_acceptance_with_hints;
use anvilminimal::minimal_loop::import_scan::route_bound_closure;
use anvilminimal::minimal_loop::interaction_probe::static_html_probe_selection;
use anvilminimal::planner::profile::{
    ProfileSnapshot, profile_expected_paths, profile_generation_rules, profile_guidance,
    profile_quality_expectations, profile_runtime_contract, profile_setup_scaffold_paths,
    verify_profile_final, verify_profile_invariant,
};
use anvilminimal::planner::profiles::nextjs;

#[derive(Debug, Default)]
struct CorpusCase {
    case_id: String,
    required_paths: Vec<String>,
    verify_commands: Vec<String>,
    required_capabilities: Vec<String>,
    required_evidence: Vec<String>,
    required_obligations: Vec<String>,
    deferred_verify_requirements: Vec<String>,
    evidence_hint_tokens: Vec<String>,
    route_closure_include: Vec<String>,
    route_closure_exclude: Vec<String>,
    evidence_tiers: BTreeMap<String, String>,
    weak_evidence_contains: Vec<String>,
    diagnostics_contains: Vec<String>,
    compile_expect: String,
    probe: Option<ProbeExpectation>,
    json_fields: BTreeMap<String, String>,
    fixture_contains: BTreeMap<String, Vec<String>>,
}

#[derive(Debug, Default)]
struct ProbeExpectation {
    html_fixture: String,
    probe_mode: Option<String>,
    contract_hook_status: Option<String>,
    primary_present: Option<bool>,
    restart_present: Option<bool>,
    first_candidate_text: Option<String>,
    first_candidate_index: Option<usize>,
    candidate_texts: Vec<String>,
}

#[test]
fn generated_app_corpus_matches_detector_and_probe_expectations() {
    let corpus_root = Path::new("tests/corpus/apps");
    let mut case_dirs = std::fs::read_dir(corpus_root)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", corpus_root.display()))
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|ty| ty.is_dir()))
        .map(|entry| entry.path())
        .collect::<Vec<_>>();
    case_dirs.sort();
    assert!(!case_dirs.is_empty(), "corpus has no cases");

    for case_dir in case_dirs {
        assert_source_only_snapshot(&case_dir);
        let expectations_path = case_dir.join("expectations.toml");
        let expectations = parse_expectations(&expectations_path);
        let display = expectations.case_id.as_str();
        assert_nextjs_profile_matches_direct_impl(&case_dir, display);

        let closure = route_bound_closure(&case_dir, "nextjs")
            .into_iter()
            .map(|path| path.to_string_lossy().replace('\\', "/"))
            .collect::<Vec<_>>();
        for expected in &expectations.route_closure_include {
            assert!(
                closure.iter().any(|path| path == expected),
                "{display}: route closure missing {expected}; closure={closure:?}"
            );
        }
        for expected in &expectations.route_closure_exclude {
            assert!(
                !closure.iter().any(|path| path == expected),
                "{display}: route closure unexpectedly includes {expected}; closure={closure:?}"
            );
        }

        let report = verify_runtime_acceptance_with_hints(
            &case_dir,
            &expectations.required_paths,
            &expectations.verify_commands,
            &expectations.required_capabilities,
            &expectations.required_evidence,
            &expectations.required_obligations,
            &expectations.deferred_verify_requirements,
            &expectations.evidence_hint_tokens,
        );
        for (evidence, expected) in &expectations.evidence_tiers {
            let actual = report
                .evidence_tiers
                .get(evidence)
                .map(String::as_str)
                .unwrap_or("absent");
            assert_eq!(
                actual, expected,
                "{display}: evidence tier mismatch for {evidence}; report={report:?}"
            );
        }
        for expected in &expectations.weak_evidence_contains {
            assert!(
                report.weak_evidence.iter().any(|item| item == expected),
                "{display}: weak evidence missing {expected}; weak={:?}; report={report:?}",
                report.weak_evidence
            );
        }
        for expected in &expectations.diagnostics_contains {
            assert!(
                report.diagnostics.iter().any(|item| item == expected),
                "{display}: diagnostic missing {expected}; diagnostics={:?}; report={report:?}",
                report.diagnostics
            );
        }
        assert!(
            matches!(
                expectations.compile_expect.as_str(),
                "not_checked" | "build_passed" | "build_failed"
            ),
            "{display}: unsupported compile expectation {:?}",
            expectations.compile_expect
        );

        if let Some(probe) = expectations.probe {
            let html_path = case_dir.join(&probe.html_fixture);
            let html = std::fs::read_to_string(&html_path).unwrap_or_else(|err| {
                panic!("{display}: failed to read {}: {err}", html_path.display())
            });
            let observation = static_html_probe_selection(&html);
            if let Some(expected) = probe.probe_mode {
                assert_eq!(observation.probe_mode, expected, "{display}: probe mode");
            }
            if let Some(expected) = probe.contract_hook_status {
                assert_eq!(
                    observation.contract_hook_status, expected,
                    "{display}: contract hook status"
                );
            }
            if let Some(expected) = probe.primary_present {
                assert_eq!(
                    observation.primary_present, expected,
                    "{display}: primary hook"
                );
            }
            if let Some(expected) = probe.restart_present {
                assert_eq!(
                    observation.restart_present, expected,
                    "{display}: restart hook"
                );
            }
            if let Some(expected) = probe.first_candidate_text {
                let first = observation
                    .candidate_table
                    .first()
                    .unwrap_or_else(|| panic!("{display}: empty candidate table"));
                assert_eq!(
                    first.text_excerpt, expected,
                    "{display}: first candidate text"
                );
            }
            if let Some(expected) = probe.first_candidate_index {
                let first = observation
                    .candidate_table
                    .first()
                    .unwrap_or_else(|| panic!("{display}: empty candidate table"));
                assert_eq!(first.index, expected, "{display}: first candidate index");
            }
            if !probe.candidate_texts.is_empty() {
                let actual = observation
                    .candidate_table
                    .iter()
                    .map(|candidate| candidate.text_excerpt.clone())
                    .collect::<Vec<_>>();
                assert_eq!(
                    actual
                        .iter()
                        .take(probe.candidate_texts.len())
                        .cloned()
                        .collect::<Vec<_>>(),
                    probe.candidate_texts,
                    "{display}: candidate text prefix; full={actual:?}"
                );
            }
        }

        for (selector, expected) in &expectations.json_fields {
            assert_json_field(&case_dir, display, selector, expected);
        }
        for (fixture, expected_lines) in &expectations.fixture_contains {
            assert_fixture_contains(&case_dir, display, fixture, expected_lines);
        }
    }
}

fn assert_nextjs_profile_matches_direct_impl(case_dir: &Path, display: &str) {
    let goal = display;
    assert_eq!(
        verify_profile_final(case_dir, "nextjs", goal),
        nextjs::verify(case_dir, goal),
        "{display}: Next.js final verification changed behind DomainProfile"
    );
    assert_eq!(
        verify_profile_invariant(case_dir, "nextjs", goal, &ProfileSnapshot::None),
        nextjs::verify_invariant(case_dir, goal),
        "{display}: Next.js invariant verification changed behind DomainProfile"
    );
    assert_eq!(
        profile_expected_paths(case_dir, "nextjs", goal),
        nextjs::expected_paths(case_dir, goal),
        "{display}: Next.js expected paths changed behind DomainProfile"
    );
    assert_eq!(
        profile_setup_scaffold_paths(case_dir, "nextjs"),
        nextjs::setup_scaffold_paths(case_dir),
        "{display}: Next.js setup scaffold paths changed behind DomainProfile"
    );
    assert_eq!(
        profile_guidance("nextjs", goal),
        Some(nextjs::guidance(goal)),
        "{display}: Next.js guidance changed behind DomainProfile"
    );
    assert_eq!(
        profile_runtime_contract("nextjs", "create", goal),
        nextjs::runtime_contract("create", goal),
        "{display}: Next.js runtime contract changed behind DomainProfile"
    );
    assert_eq!(
        profile_generation_rules("nextjs", "implement"),
        Some(nextjs::generation_rules("implement")),
        "{display}: Next.js generation rules changed behind DomainProfile"
    );
    assert_eq!(
        profile_quality_expectations(case_dir, "nextjs", goal),
        nextjs::quality_expectations(case_dir, goal),
        "{display}: Next.js quality expectations changed behind DomainProfile"
    );
}

fn assert_source_only_snapshot(case_dir: &Path) {
    for generated in ["node_modules", ".next", ".anvil", "target"] {
        assert!(
            !case_dir.join(generated).exists(),
            "{} must not contain generated directory {generated}",
            case_dir.display()
        );
    }
}

fn parse_expectations(path: &Path) -> CorpusCase {
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    let mut case = CorpusCase {
        compile_expect: "not_checked".to_string(),
        ..CorpusCase::default()
    };
    let mut section = String::new();
    for (index, raw_line) in logical_toml_lines(&text) {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line
                .trim_start_matches('[')
                .trim_end_matches(']')
                .trim()
                .to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            panic!("{}:{}: expected key = value", path.display(), index + 1);
        };
        let key = key.trim();
        let value = value.trim();
        match section.as_str() {
            "" => match key {
                "case_id" => case.case_id = parse_string(value),
                "required_paths" => case.required_paths = parse_string_array(value),
                "verify_commands" => case.verify_commands = parse_string_array(value),
                "required_capabilities" => case.required_capabilities = parse_string_array(value),
                "required_evidence" => case.required_evidence = parse_string_array(value),
                "required_obligations" => case.required_obligations = parse_string_array(value),
                "deferred_verify_requirements" => {
                    case.deferred_verify_requirements = parse_string_array(value);
                }
                "evidence_hint_tokens" => case.evidence_hint_tokens = parse_string_array(value),
                "source" => {
                    let _ = parse_string(value);
                }
                _ => panic!("{}:{}: unknown root key {key}", path.display(), index + 1),
            },
            "route_closure" => match key {
                "include" => case.route_closure_include = parse_string_array(value),
                "exclude" => case.route_closure_exclude = parse_string_array(value),
                _ => panic!(
                    "{}:{}: unknown route_closure key {key}",
                    path.display(),
                    index + 1
                ),
            },
            "evidence" => {
                case.evidence_tiers
                    .insert(key.to_string(), parse_tier(value));
            }
            "weak_evidence" => match key {
                "contains" => case.weak_evidence_contains = parse_string_array(value),
                _ => panic!(
                    "{}:{}: unknown weak_evidence key {key}",
                    path.display(),
                    index + 1
                ),
            },
            "diagnostics" => match key {
                "contains" => case.diagnostics_contains = parse_string_array(value),
                _ => panic!(
                    "{}:{}: unknown diagnostics key {key}",
                    path.display(),
                    index + 1
                ),
            },
            "compile" => match key {
                "expect" => case.compile_expect = parse_string(value),
                _ => panic!(
                    "{}:{}: unknown compile key {key}",
                    path.display(),
                    index + 1
                ),
            },
            "probe" => {
                let probe = case.probe.get_or_insert_with(ProbeExpectation::default);
                match key {
                    "html_fixture" => probe.html_fixture = parse_string(value),
                    "probe_mode" => probe.probe_mode = Some(parse_string(value)),
                    "contract_hook_status" => {
                        probe.contract_hook_status = Some(parse_string(value));
                    }
                    "primary_present" => probe.primary_present = Some(parse_bool(value)),
                    "restart_present" => probe.restart_present = Some(parse_bool(value)),
                    "first_candidate_text" => {
                        probe.first_candidate_text = Some(parse_string(value));
                    }
                    "first_candidate_index" => {
                        probe.first_candidate_index = Some(parse_usize(value));
                    }
                    "candidate_texts" => probe.candidate_texts = parse_string_array(value),
                    _ => panic!("{}:{}: unknown probe key {key}", path.display(), index + 1),
                }
            }
            "json_fields" => {
                case.json_fields
                    .insert(key.to_string(), parse_string(value));
            }
            "fixture_contains" => {
                case.fixture_contains
                    .insert(key.to_string(), parse_string_array(value));
            }
            _ => panic!(
                "{}:{}: unknown expectations section [{}]",
                path.display(),
                index + 1,
                section
            ),
        }
    }
    if case.case_id.is_empty() {
        case.case_id = path
            .parent()
            .and_then(Path::file_name)
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();
    }
    case
}

fn assert_fixture_contains(
    case_dir: &Path,
    display: &str,
    fixture: &str,
    expected_lines: &[String],
) {
    let fixture_path = case_dir.join(fixture);
    let text = std::fs::read_to_string(&fixture_path).unwrap_or_else(|err| {
        panic!(
            "{display}: failed to read fixture {}: {err}",
            fixture_path.display()
        )
    });
    for expected in expected_lines {
        assert!(
            text.contains(expected),
            "{display}: fixture {fixture} missing expected text {expected:?}"
        );
    }
}

fn assert_json_field(case_dir: &Path, display: &str, selector: &str, expected: &str) {
    let Some((fixture, field_path)) = selector.split_once(':') else {
        panic!("{display}: json_fields selector must be <fixture>:<field>, got {selector}");
    };
    let json_path = case_dir.join(fixture);
    let text = std::fs::read_to_string(&json_path)
        .unwrap_or_else(|err| panic!("{display}: failed to read {}: {err}", json_path.display()));
    let value: serde_json::Value = serde_json::from_str(&text)
        .unwrap_or_else(|err| panic!("{display}: invalid json {}: {err}", json_path.display()));
    let actual = json_scalar_at_path(&value, field_path).unwrap_or_else(|| {
        panic!(
            "{display}: json field {field_path} missing or non-scalar in {}",
            json_path.display()
        )
    });
    assert_eq!(
        actual, expected,
        "{display}: json field mismatch for {selector}"
    );
}

fn json_scalar_at_path(value: &serde_json::Value, path: &str) -> Option<String> {
    let mut current = value;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    match current {
        serde_json::Value::String(value) => Some(value.clone()),
        serde_json::Value::Bool(value) => Some(value.to_string()),
        serde_json::Value::Number(value) => Some(value.to_string()),
        serde_json::Value::Null => Some("null".to_string()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => None,
    }
}

fn strip_comment(line: &str) -> &str {
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in line.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = in_string;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if ch == '#' && !in_string {
            return &line[..index];
        }
    }
    line
}

fn logical_toml_lines(text: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut pending = String::new();
    let mut pending_line = 0usize;
    for (index, raw_line) in text.lines().enumerate() {
        let line = strip_comment(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        if pending.is_empty() {
            pending_line = index;
            pending.push_str(line);
        } else {
            pending.push(' ');
            pending.push_str(line);
        }
        if pending.contains('=') && array_bracket_depth(&pending) > 0 {
            continue;
        }
        out.push((pending_line, std::mem::take(&mut pending)));
    }
    if !pending.is_empty() {
        out.push((pending_line, pending));
    }
    out
}

fn array_bracket_depth(line: &str) -> i32 {
    let mut depth = 0i32;
    let mut in_string = false;
    let mut escaped = false;
    for ch in line.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = in_string;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if in_string {
            continue;
        }
        match ch {
            '[' => depth += 1,
            ']' => depth -= 1,
            _ => {}
        }
    }
    depth
}

fn parse_string(value: &str) -> String {
    let value = value.trim();
    assert!(
        value.starts_with('"') && value.ends_with('"'),
        "expected string: {value}"
    );
    unescape_basic(&value[1..value.len() - 1])
}

fn parse_string_array(value: &str) -> Vec<String> {
    let value = value.trim();
    assert!(
        value.starts_with('[') && value.ends_with(']'),
        "expected array: {value}"
    );
    let inner = value[1..value.len() - 1].trim();
    if inner.is_empty() {
        return Vec::new();
    }
    split_array_items(inner)
        .into_iter()
        .map(parse_string)
        .collect()
}

fn split_array_items(value: &str) -> Vec<&str> {
    let mut items = Vec::new();
    let mut start = 0;
    let mut in_string = false;
    let mut escaped = false;
    for (index, ch) in value.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = in_string;
            continue;
        }
        if ch == '"' {
            in_string = !in_string;
            continue;
        }
        if ch == ',' && !in_string {
            items.push(value[start..index].trim());
            start = index + 1;
        }
    }
    items.push(value[start..].trim());
    items
}

fn parse_tier(value: &str) -> String {
    match parse_string(value).to_ascii_lowercase().as_str() {
        "strong" => "strong".to_string(),
        "weak" => "weak".to_string(),
        "absent" => "absent".to_string(),
        other => panic!("unknown evidence tier {other}"),
    }
}

fn parse_bool(value: &str) -> bool {
    match value.trim() {
        "true" => true,
        "false" => false,
        other => panic!("expected bool: {other}"),
    }
}

fn parse_usize(value: &str) -> usize {
    value
        .trim()
        .parse::<usize>()
        .unwrap_or_else(|err| panic!("expected usize {value}: {err}"))
}

fn unescape_basic(value: &str) -> String {
    value
        .replace(r#"\""#, "\"")
        .replace(r#"\\"#, "\\")
        .replace(r#"\n"#, "\n")
}
