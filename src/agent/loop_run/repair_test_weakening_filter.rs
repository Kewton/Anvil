use super::repair_assertion_analysis::{
    assert_line_update_has_observed_pair, changed_assert_equalities, changed_assert_lines,
    changed_non_assert_lines, count_assert_lines, observed_assert_equal_pairs,
    python_assert_line_is_obvious_weakening, python_assert_line_observes_fixture_state,
};
use super::repair_python_test_analysis::{
    excerpt_asserts_fixture_state, excerpt_connects_fixture_to_system_under_test,
    line_mentions_identifier, pytest_local_mutable_fixture_names,
};
use super::spec_authority::WeakeningPattern;

pub(crate) fn filter_weakening_for_observed_assert_update(
    patterns: Vec<WeakeningPattern>,
    context: &super::repair_job::RepairJob,
    before: &str,
    after: &str,
) -> Vec<WeakeningPattern> {
    use WeakeningPattern::{AssertionDeleted, LiteralOnlyExpectedChange};

    if patterns.is_empty() {
        return patterns;
    }
    let only_expected_assert_update = patterns
        .iter()
        .all(|pattern| matches!(pattern, AssertionDeleted | LiteralOnlyExpectedChange));
    if !only_expected_assert_update {
        return patterns;
    }
    if count_assert_lines(after) < count_assert_lines(before) {
        return patterns;
    }
    if classify_allowed_change_kind(context, before, after).is_some() {
        Vec::new()
    } else {
        patterns
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AllowedChangeKind {
    ExpectedLiteral,
    DisconnectedFixtureObservation,
    TestOnlyMissingImportSymbol,
}

fn classify_allowed_change_kind(
    context: &super::repair_job::RepairJob,
    before: &str,
    after: &str,
) -> Option<AllowedChangeKind> {
    if !expectation_alignment_allowed_by_authority(context) {
        return None;
    }
    if observed_assert_update_matches_repair_context(context, before, after) {
        return Some(AllowedChangeKind::ExpectedLiteral);
    }
    if disconnected_fixture_assertion_update_matches(context, before, after) {
        return Some(AllowedChangeKind::DisconnectedFixtureObservation);
    }
    test_only_missing_import_symbol_update_matches(context, before, after)
        .then_some(AllowedChangeKind::TestOnlyMissingImportSymbol)
}

fn expectation_alignment_allowed_by_authority(context: &super::repair_job::RepairJob) -> bool {
    let Some(plan) = context.semantic_plan.as_ref() else {
        return false;
    };
    if matches!(
        plan.semantic_cause,
        super::VerifierDiagnosticFailureKind::TestBug
    ) || matches!(
        plan.semantic_report.failure_kind,
        super::VerifierDiagnosticFailureKind::TestBug
    ) {
        return true;
    }
    !matches!(
        plan.spec_authority,
        super::spec_authority::SpecAuthority::UserRequest
    )
}

fn observed_assert_update_matches_repair_context(
    context: &super::repair_job::RepairJob,
    before: &str,
    after: &str,
) -> bool {
    let deleted_asserts = changed_assert_equalities(before, after);
    let added_asserts = changed_assert_equalities(after, before);
    if deleted_asserts.is_empty()
        || added_asserts.is_empty()
        || deleted_asserts.len() != added_asserts.len()
    {
        return false;
    }

    let observed_pairs = observed_pairs_for_repair_context(context);
    if observed_pairs.is_empty() {
        return false;
    }

    deleted_asserts.iter().all(|(old_lhs, old_expected)| {
        added_asserts.iter().any(|(new_lhs, new_expected)| {
            old_lhs == new_lhs
                && observed_pairs
                    .iter()
                    .any(|(actual, expected)| actual == new_expected && expected == old_expected)
        })
    }) && added_asserts.iter().all(|(new_lhs, new_expected)| {
        deleted_asserts.iter().any(|(old_lhs, old_expected)| {
            old_lhs == new_lhs
                && observed_pairs
                    .iter()
                    .any(|(actual, expected)| actual == new_expected && expected == old_expected)
        })
    })
}

fn disconnected_fixture_assertion_update_matches(
    context: &super::repair_job::RepairJob,
    before: &str,
    after: &str,
) -> bool {
    let disconnected_fixtures = pytest_local_mutable_fixture_names(before)
        .into_iter()
        .filter(|name| {
            excerpt_asserts_fixture_state(before, name)
                && !excerpt_connects_fixture_to_system_under_test(before, name)
        })
        .collect::<Vec<_>>();
    if disconnected_fixtures.is_empty() {
        return false;
    }
    if count_assert_lines(after) < count_assert_lines(before) {
        return false;
    }
    let deleted_asserts = changed_assert_lines(before, after);
    if deleted_asserts.is_empty() {
        return false;
    }
    let added_asserts = changed_assert_lines(after, before);
    if added_asserts.len() < deleted_asserts.len() {
        return false;
    }
    let observed_pairs = observed_pairs_for_repair_context(context);
    deleted_asserts.iter().all(|line| {
        let fixture_observation = disconnected_fixtures
            .iter()
            .any(|name| python_assert_line_observes_fixture_state(line, name));
        fixture_observation
            || added_asserts
                .iter()
                .any(|added| assert_line_update_has_observed_pair(line, added, &observed_pairs))
    }) && added_asserts
        .iter()
        .all(|line| !python_assert_line_is_obvious_weakening(line))
}

fn test_only_missing_import_symbol_update_matches(
    context: &super::repair_job::RepairJob,
    before: &str,
    after: &str,
) -> bool {
    let diagnostic_text = repair_context_diagnostic_text(context);
    let Some((name, module)) = missing_import_name_from_output(&diagnostic_text) else {
        return false;
    };
    if !source_imports_name_from_module(before, &module, &name)
        || source_imports_name_from_module(after, &module, &name)
    {
        return false;
    }
    if count_assert_lines(after) < count_assert_lines(before) {
        return false;
    }
    let deleted_asserts = changed_assert_lines(before, after);
    let added_asserts = changed_assert_lines(after, before);
    if !deleted_asserts.is_empty() && added_asserts.len() < deleted_asserts.len() {
        return false;
    }
    let changed_non_asserts = changed_non_assert_lines(before, after);
    if !changed_non_asserts
        .iter()
        .any(|line| line_mentions_identifier(line, &name))
    {
        return false;
    }
    added_asserts
        .iter()
        .all(|line| !python_assert_line_is_obvious_weakening(line))
}

fn repair_context_diagnostic_text(context: &super::repair_job::RepairJob) -> String {
    format!(
        "{}\n{}\n{}",
        context.failure_signature,
        context.output_excerpt,
        context.repair_error.as_deref().unwrap_or("")
    )
}

fn observed_pairs_for_repair_context(
    context: &super::repair_job::RepairJob,
) -> Vec<(String, String)> {
    let pairs = super::failure_packet::FailurePacket::from_repair_job(context)
        .observed_expected_pairs
        .into_iter()
        .map(|pair| (pair.observed, pair.expected))
        .collect::<Vec<_>>();
    if pairs.is_empty() {
        observed_assert_equal_pairs(&repair_context_diagnostic_text(context))
    } else {
        pairs
    }
}

fn missing_import_name_from_output(output: &str) -> Option<(String, String)> {
    for quote in ["'", "\""] {
        let marker = format!("ImportError: cannot import name {quote}");
        let Some(start) = output.find(&marker) else {
            continue;
        };
        let rest = &output[start + marker.len()..];
        let Some(name_end) = rest.find(quote) else {
            continue;
        };
        let name = rest[..name_end].trim();
        let from_marker = format!(" from {quote}");
        let rest = &rest[name_end + quote.len()..];
        let Some(module_start) = rest.find(&from_marker) else {
            continue;
        };
        let rest = &rest[module_start + from_marker.len()..];
        let Some(module_end) = rest.find(quote) else {
            continue;
        };
        let module = rest[..module_end].trim();
        if identifier_is_safe(name) && module_name_is_safe(module) {
            return Some((name.to_string(), module.to_string()));
        }
    }
    None
}

fn source_imports_name_from_module(source: &str, module: &str, name: &str) -> bool {
    source.lines().any(|line| {
        let stripped = strip_inline_comment(line);
        let trimmed = stripped.trim();
        let Some(rest) = trimmed.strip_prefix("from ") else {
            return false;
        };
        let Some((from_module, imports)) = rest.split_once(" import ") else {
            return false;
        };
        from_module.trim() == module && import_list_contains_name(imports, name)
    })
}

fn import_list_contains_name(imports: &str, name: &str) -> bool {
    imports.split(',').any(|entry| {
        let imported = entry
            .split_whitespace()
            .next()
            .unwrap_or_default()
            .trim_matches(|ch: char| ch == '(' || ch == ')' || ch == '\\');
        imported == name
    })
}

fn strip_inline_comment(line: &str) -> String {
    line.split_once('#')
        .map(|(head, _)| head)
        .unwrap_or(line)
        .to_string()
}

fn module_name_is_safe(module: &str) -> bool {
    module.split('.').count() >= 2 && module.split('.').all(identifier_is_safe)
}

fn identifier_is_safe(name: &str) -> bool {
    let mut chars = name.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::super::VerifierDiagnosticFailureKind;
    use super::super::repair_job::{RepairJob, SemanticRepairPlan};
    use super::super::semantic_failure::parse_semantic_failure_report;
    use super::super::spec_authority::SpecAuthority;
    use super::super::task_contract::ArtifactRole;
    use super::*;

    fn test_bug_repair_job_with_output(output_excerpt: &str) -> RepairJob {
        let report = parse_semantic_failure_report(&serde_json::json!({
            "failure_kind": "test_bug",
            "confidence": 0.95,
            "preferred_repair_role": "test",
            "repair_hypothesis": "generated test expected literals contradict explicit behavior",
            "failure_clusters": [{
                "observed": "implementation follows cumulative score",
                "expected": "tests expect isolated criterion score",
                "input_shape": "assert equal",
                "assertion_shape": "assert_equal",
                "involved_artifacts": ["test"],
                "affected_cases": ["test_length_at_least_8"],
            }],
        }))
        .expect("semantic report parses");
        let cluster_id = report.failure_clusters[0].cluster_key.clone();

        RepairJob {
            output_excerpt: output_excerpt.to_string(),
            semantic_plan: Some(SemanticRepairPlan {
                semantic_report: report,
                failure_cluster_id: cluster_id,
                semantic_cause: VerifierDiagnosticFailureKind::TestBug,
                spec_authority: SpecAuthority::UserRequest,
                preferred_repair_role: ArtifactRole::Test,
                repair_hypothesis: "generated test expected literals contradict explicit behavior"
                    .to_string(),
                expected_improvement: None,
                assessment_generation_at_creation: 0,
            }),
            ..RepairJob::new_for_test()
        }
    }

    #[test]
    fn missing_import_parser_extracts_symbol_and_module() {
        let parsed = missing_import_name_from_output(
            "ImportError: cannot import name 'COUNTER' from 'app.main' (/work/app/main.py)",
        );

        assert_eq!(
            parsed,
            Some(("COUNTER".to_string(), "app.main".to_string()))
        );
    }

    #[test]
    fn source_import_detection_handles_parenthesized_imports() {
        let source = "from app.main import (COUNTER, app)\n";

        assert!(source_imports_name_from_module(
            source, "app.main", "COUNTER"
        ));
    }

    #[test]
    fn generated_test_expectation_repair_uses_structured_flattened_observed_pairs() {
        let before = r#"
def test_length_at_least_8():
    assert score_password("abcdefgh") == 1
    assert score_password("abc") == 0

def test_contains_uppercase():
    assert score_password("A") == 1
    assert score_password("abc") == 0
"#;
        let after = r#"
def test_length_at_least_8():
    assert score_password("abcdefgh") == 2
    assert score_password("abc") == 1

def test_contains_uppercase():
    assert score_password("A") == 1
    assert score_password("abc") == 1
"#;
        let output = r#"FAILED tests/test_password_strength.py::test_length_at_least_8 > assert score_password("abcdefgh") == 1 E AssertionError: assert 2 == 1 E where 2 = score_password('abcdefgh') FAILED tests/test_password_strength.py::test_contains_uppercase > assert score_password("abc") == 0 E AssertionError: assert 1 == 0 E where 1 = score_password('abc')"#;
        let job = test_bug_repair_job_with_output(output);

        let filtered = filter_weakening_for_observed_assert_update(
            vec![
                WeakeningPattern::AssertionDeleted,
                WeakeningPattern::LiteralOnlyExpectedChange,
            ],
            &job,
            before,
            after,
        );

        assert!(filtered.is_empty());
    }
}
