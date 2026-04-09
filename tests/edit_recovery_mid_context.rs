//! Integration tests for Issue #313: file.edit recovery mid-context acquisition.
//!
//! Tests that extract_edit_context returns useful context even when the
//! first-line exact match fails, using token-based fallback matching.
//! Also tests dynamic context sizing for large files.

// ============================================================
// Test 1: token-based fallback when first-line exact match fails
// ============================================================

#[test]
fn test_extract_edit_context_token_fallback_on_first_line_drift() {
    // Simulates the A2 failure shape: model hypothesis drifts from reality.
    // Hypothesis: detectPrompt(cleanedOutput, detectedCliTool || 'claude')
    // Reality:    detectPrompt(cleanOutput, { ...promptOptions, ... })
    // The function name "detectPrompt" is a shared token that should anchor.
    let content = r#"import { something } from 'module';

const foo = 1;
const bar = 2;

function setupPoller() {
    const config = getConfig();
    return config;
}

// Many lines of code...
const x = 10;
const y = 20;
const z = 30;

function helperA() { return 1; }
function helperB() { return 2; }

const promptOptions = buildDetectPromptOptions(cliToolId);
const promptDetection = detectPrompt(cleanOutput, {
    ...promptOptions,
    ...(precomputedLines && { precomputedLines }),
});

if (promptDetection.detected) {
    handleDetection(promptDetection);
}

function cleanup() {
    clearInterval(pollInterval);
}

export { setupPoller, cleanup };
"#;

    // Model's wrong hypothesis — first line doesn't match exactly
    let old_string = "detectPrompt(cleanedOutput, detectedCliTool || 'claude')";

    let result = anvil::tooling::extract_edit_context(content, old_string, 5);
    assert!(
        result.is_some(),
        "should find context via token fallback even when first-line exact match fails"
    );
    let ctx = result.unwrap();
    assert!(
        ctx.contains("detectPrompt"),
        "context should contain the actual detectPrompt call"
    );
    assert!(
        ctx.contains("promptOptions"),
        "context should show nearby lines with promptOptions"
    );
}

// ============================================================
// Test 2: token fallback with multiple candidate tokens
// ============================================================

#[test]
fn test_extract_edit_context_token_fallback_best_scoring_line() {
    let content = r#"line 1
line 2
fn process_data(input: &str) -> Result<Data, Error> {
    let parsed = parse_input(input);
    validate(parsed)
}
line 7
line 8
fn transform(data: Data) -> Output {
    data.convert()
}
"#;

    // Hypothesis has "process_data" and "input" — should match line 3
    let old_string = "fn process_data(raw_input: String) -> Data {";

    let result = anvil::tooling::extract_edit_context(content, old_string, 2);
    assert!(result.is_some(), "token fallback should find process_data");
    let ctx = result.unwrap();
    assert!(
        ctx.contains("process_data"),
        "should anchor on process_data token"
    );
}

// ============================================================
// Test 3: large file gets more context lines
// ============================================================

#[test]
fn test_extract_edit_context_large_file_expanded_context() {
    // Build a 600-line file with target at line 326
    let mut lines: Vec<String> = Vec::new();
    for i in 1..=325 {
        lines.push(format!("const padding_{i} = {i};"));
    }
    lines.push("const target = detectPrompt(cleanOutput, options);".to_string());
    for i in 327..=600 {
        lines.push(format!("const padding_{i} = {i};"));
    }
    let content = lines.join("\n");

    let old_string = "detectPrompt(cleanOutput, options)";

    // With context_lines=5 (old behavior), we'd get 11 lines
    let result_small = anvil::tooling::extract_edit_context(&content, old_string, 5);
    assert!(result_small.is_some());
    let ctx_small = result_small.unwrap();
    let small_line_count = ctx_small.lines().count();

    // With context_lines=12 (large file), we'd get 25 lines
    let result_large = anvil::tooling::extract_edit_context(&content, old_string, 12);
    assert!(result_large.is_some());
    let ctx_large = result_large.unwrap();
    let large_line_count = ctx_large.lines().count();

    assert!(
        large_line_count > small_line_count,
        "large file context ({large_line_count}) should have more lines than small ({small_line_count})"
    );
    assert!(
        ctx_large.contains("326"),
        "context should show line 326 where the target is"
    );
}

// ============================================================
// Test 4: exact match still takes priority over token fallback
// ============================================================

#[test]
fn test_extract_edit_context_exact_match_priority() {
    let content = "line 1\nline 2\nfn hello() {\n    println!(\"hi\");\n}\nline 6";
    let old_string = "fn hello() {\n    println!(\"world\");\n}";

    let result = anvil::tooling::extract_edit_context(content, old_string, 2);
    assert!(result.is_some());
    let ctx = result.unwrap();
    // Exact first-line match should work as before
    assert!(ctx.contains("fn hello()"));
}

// ============================================================
// Test 5: token fallback returns None when no tokens match
// ============================================================

#[test]
fn test_extract_edit_context_token_fallback_no_match() {
    let content = "alpha\nbeta\ngamma\ndelta";
    let old_string = "completely_unrelated_function(x, y, z)";

    let result = anvil::tooling::extract_edit_context(content, old_string, 5);
    assert!(
        result.is_none(),
        "should return None when no tokens match at all"
    );
}

// ============================================================
// Test 6: mid-file context for 500+ line file with drifted hypothesis
// ============================================================

#[test]
fn test_mid_file_context_large_file_drifted_hypothesis() {
    // Reproduction of Issue #313 A2 failure shape:
    // - 599-line file
    // - Target at line ~326
    // - Model hypothesis first line completely wrong
    let mut lines: Vec<String> = Vec::new();
    for i in 1..=320 {
        lines.push(format!("// filler line {i}"));
    }
    // Real code around line 321-330
    lines.push("const promptOptions = buildDetectPromptOptions(cliToolId);".to_string());
    lines.push("const promptDetection = detectPrompt(cleanOutput, {".to_string());
    lines.push("    ...promptOptions,".to_string());
    lines.push("    ...(precomputedLines && { precomputedLines }),".to_string());
    lines.push("});".to_string());
    lines.push("".to_string());
    for i in 327..=599 {
        lines.push(format!("// filler line {i}"));
    }
    let content = lines.join("\n");

    // Model's completely wrong hypothesis
    let old_string = "detectPrompt(cleanedOutput, detectedCliTool || 'claude')";

    let result = anvil::tooling::extract_edit_context(&content, old_string, 8);
    assert!(
        result.is_some(),
        "token fallback should find detectPrompt in mid-file despite drifted hypothesis"
    );
    let ctx = result.unwrap();
    assert!(
        ctx.contains("detectPrompt"),
        "context must contain the real detectPrompt call"
    );
    assert!(
        ctx.contains("promptOptions"),
        "context should show nearby promptOptions"
    );
}

// ============================================================
// Test 7: build_edit_not_found_with_context uses scaled context_lines
// ============================================================
// (This is tested indirectly via extract_edit_context parameter scaling;
//  the build function is private, but the public extract_edit_context
//  receives the scaled value.)

// ============================================================
// Test 8: token extraction handles various identifier patterns
// ============================================================

#[test]
fn test_extract_edit_context_token_fallback_various_identifiers() {
    let content = r#"use std::collections::HashMap;

fn main() {
    let my_special_variable = compute_result(42);
    println!("{}", my_special_variable);
}

fn compute_result(n: i32) -> i32 {
    n * 2
}
"#;

    // Hypothesis with wrong structure but shared identifiers
    let old_string = "let my_special_variable = compute_result(input_value);";

    let result = anvil::tooling::extract_edit_context(content, old_string, 3);
    assert!(result.is_some(), "should match via shared identifiers");
    let ctx = result.unwrap();
    assert!(ctx.contains("my_special_variable"));
    assert!(ctx.contains("compute_result"));
}
