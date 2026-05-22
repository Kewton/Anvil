//! ActionSummary answer-scrub regex SSOT (Issue #604).
//!
//! Rust `regex` crate constraints: no look-around, no backreferences,
//! but `(?i)` inline flag is supported. All patterns operate on a single
//! line (`[^\n]*?` is used inside families that traverse across tokens)
//! to keep catastrophic-backtracking risk low.
//!
//! Layer rule (DR3-002): session layer SSOT; photon layer must not import
//! this module. The agent layer is responsible for mapping `ScrubAction::Hit`
//! into an `AutoPromoteSkipReason` (strict mode) or for emitting the
//! `scrubbed` event (warn mode).

use std::sync::OnceLock;

use regex::Regex;

use crate::session::case_photon_bridge::ActionSummary;

/// Default scrub mode (DR1-018 SSOT). All env / Config / `from_env_str_or_default`
/// paths must derive their default from this single constant.
pub const DEFAULT_SCRUB_MODE: ScrubMode = ScrubMode::Strict;

/// Replacement string used by warn-mode to redact a hit field while keeping
/// the field present (so downstream serialize / upsert can continue).
pub const SCRUB_PLACEHOLDER: &str = "[scrubbed]";

/// DR1-006: `Off` is intentionally absent. To fully disable promotion, set
/// `ANVIL_PHOTON_NO_AUTO_PROMOTE=1` (feature disable) or rely on
/// `ANVIL_PHOTON_AUTO_PROMOTE_DRY_RUN=true` (HTTP skip).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScrubMode {
    /// Detection causes the caller (`invoke_photon_auto_promote`) to convert
    /// the outcome into `Skip(AnswerLeak{fields_detected})` (DR2-011 rename).
    Strict,
    /// Detection causes the hit field to be replaced with `SCRUB_PLACEHOLDER`
    /// before forwarding the summary to photon (`scrubbed` event is emitted).
    Warn,
}

impl ScrubMode {
    /// DR1-016: never silently fall back. Typos like `'strcit'` must surface
    /// as `Err` so the Config layer can emit a `tracing::warn!` and apply
    /// `DEFAULT_SCRUB_MODE` deliberately.
    pub fn from_env_str(s: &str) -> Result<Self, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "strict" => Ok(Self::Strict),
            "warn" => Ok(Self::Warn),
            other => Err(format!(
                "unknown scrub mode '{other}' (allowed: strict | warn)"
            )),
        }
    }

    /// Config helper that logs a warning on parse error and falls back to
    /// `DEFAULT_SCRUB_MODE` (DR1-018).
    pub fn from_env_str_or_default(s: &str) -> Self {
        match Self::from_env_str(s) {
            Ok(mode) => mode,
            Err(msg) => {
                tracing::warn!(
                    "ANVIL_PHOTON_AUTO_PROMOTE_SCRUB_MODE parse error: {msg}; \
                     falling back to {:?}",
                    DEFAULT_SCRUB_MODE,
                );
                DEFAULT_SCRUB_MODE
            }
        }
    }
}

/// DR1-011 / DR2-024: two variants only. Mode-specific dispatch is the
/// caller's responsibility.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScrubAction {
    Pass,
    Hit { fields: Vec<String> },
}

/// Alias kept for forward-compat with the design policy SSOT (DR2-024).
pub type ScrubResult = ScrubAction;

/// Photon PR #120 `ANSWER_LEAK_PATTERNS` 6-family mirror.
///
/// Family names are stable and exposed via `ScrubAction::Hit.fields` only
/// indirectly through the `<family>` infix of each path entry (e.g.
/// `facts[0]:output_literal_json`).
struct ScrubPattern {
    family: &'static str,
    regex: Regex,
}

fn scrub_patterns() -> &'static [ScrubPattern] {
    static PATTERNS: OnceLock<Vec<ScrubPattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            // output_literal_json: literal `{"key": value}` style JSON output
            ScrubPattern {
                family: "output_literal_json",
                regex: Regex::new(
                    r#"(?i)\{[^{}\n]*"[A-Za-z_][\w-]*"\s*:\s*[^{}\n]+\}"#,
                )
                .expect("output_literal_json regex must compile"),
            },
            // output_literal_text: an exact `output:` / `stdout:` literal trailing answer
            //  e.g. `output: 42`, `stdout: hello`
            ScrubPattern {
                family: "output_literal_text",
                regex: Regex::new(
                    r#"(?i)\b(?:output|stdout|result|response)\s*[:=]\s*[`'"]?[^\s`'"][^\n`'"]*[`'"]?"#,
                )
                .expect("output_literal_text regex must compile"),
            },
            // solution_phrase: "the solution is", "the answer is", "the result equals"
            ScrubPattern {
                family: "solution_phrase",
                regex: Regex::new(
                    r"(?i)\bthe\s+(?:answer|result|output|response|solution|expected\s+(?:output|value))\s+(?:is|will\s+be|should\s+be|equals?|=)\b",
                )
                .expect("solution_phrase regex must compile"),
            },
            // verification_prespoil: "prints a json object", "stdout will contain", "outputs the json response"
            ScrubPattern {
                family: "verification_prespoil",
                regex: Regex::new(
                    r"(?i)\b(?:prints?|outputs?|returns?|shows?|emits?|writes?|stdout)\s+[^\n]{0,40}?\b(?:json|payload|value|number|string|answer|result)\b",
                )
                .expect("verification_prespoil regex must compile"),
            },
            // numeric_answer_equality: `identifier = 42`, `x equals 3.14`, `score is -7`
            ScrubPattern {
                family: "numeric_answer_equality",
                regex: Regex::new(
                    r"(?i)\b[A-Za-z_]\w{1,}\s*(?:=|equals?|is)\s*-?\d+(?:\.\d+)?\b",
                )
                .expect("numeric_answer_equality regex must compile"),
            },
            // line_specific_change: "change line 12 to", "edit line 7", "replace line 3 with"
            ScrubPattern {
                family: "line_specific_change",
                regex: Regex::new(
                    r"(?i)\b(?:change|edit|replace|update|insert|set)\s+line\s+\d+\b",
                )
                .expect("line_specific_change regex must compile"),
            },
        ]
    })
}

/// Scan all scrub-eligible text fields (`facts[].text`, `next_hints[].target`,
/// `avoid[].text`) of `summary` against the 6-family regex SSOT.
///
/// Mode semantics:
/// - In `Warn`, hit text fields are replaced with `SCRUB_PLACEHOLDER` so that
///   the caller can continue with upsert.
/// - In `Strict`, no mutation is performed; the caller converts the hit into a
///   `Skip(AnswerLeak)` decision.
///
/// The returned path entries follow the shape `"<section>[<index>]:<family>"`
/// (e.g. `"facts[1]:numeric_answer_equality"`) so consumers can both group by
/// family for telemetry and drill into a specific entry.
pub fn scrub_action_summary(summary: &mut ActionSummary, mode: ScrubMode) -> ScrubResult {
    let patterns = scrub_patterns();
    let mut hits: Vec<String> = Vec::new();

    for (idx, fact) in summary.facts.iter_mut().enumerate() {
        if let Some(family) = first_family_hit(patterns, &fact.text) {
            hits.push(format!("facts[{idx}]:{family}"));
            if mode == ScrubMode::Warn {
                fact.text = SCRUB_PLACEHOLDER.to_string();
            }
        }
    }
    for (idx, avoid) in summary.avoid.iter_mut().enumerate() {
        if let Some(family) = first_family_hit(patterns, &avoid.text) {
            hits.push(format!("avoid[{idx}]:{family}"));
            if mode == ScrubMode::Warn {
                avoid.text = SCRUB_PLACEHOLDER.to_string();
            }
        }
    }
    for (idx, hint) in summary.next_hints.iter_mut().enumerate() {
        if let Some(family) = first_family_hit(patterns, &hint.target) {
            hits.push(format!("next_hints[{idx}]:{family}"));
            if mode == ScrubMode::Warn {
                hint.target = SCRUB_PLACEHOLDER.to_string();
            }
        }
    }

    if hits.is_empty() {
        ScrubAction::Pass
    } else {
        ScrubAction::Hit { fields: hits }
    }
}

fn first_family_hit(patterns: &[ScrubPattern], text: &str) -> Option<&'static str> {
    for ScrubPattern { family, regex } in patterns {
        if regex.is_match(text) {
            return Some(*family);
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::case_photon_bridge::{ActionSummary, Avoid, Fact, Hint};

    fn make_summary_with_facts(texts: &[&str]) -> ActionSummary {
        ActionSummary {
            schema_version: "action-memory.v0.2".to_string(),
            summary_id: "anvil-case-test".to_string(),
            repo_id: "workspace:test".to_string(),
            task_signature: "test signature".to_string(),
            facts: texts
                .iter()
                .map(|t| Fact {
                    text: (*t).to_string(),
                    confidence: 0.6,
                })
                .collect(),
            avoid: Vec::new(),
            next_hints: Vec::new(),
            provenance: None,
        }
    }

    fn make_summary_with_avoid(texts: &[&str]) -> ActionSummary {
        ActionSummary {
            schema_version: "action-memory.v0.2".to_string(),
            summary_id: "anvil-case-test".to_string(),
            repo_id: "workspace:test".to_string(),
            task_signature: "test signature".to_string(),
            facts: Vec::new(),
            avoid: texts
                .iter()
                .map(|t| Avoid {
                    text: (*t).to_string(),
                    confidence: 0.6,
                })
                .collect(),
            next_hints: Vec::new(),
            provenance: None,
        }
    }

    fn make_summary_with_hints(targets: &[&str]) -> ActionSummary {
        ActionSummary {
            schema_version: "action-memory.v0.2".to_string(),
            summary_id: "anvil-case-test".to_string(),
            repo_id: "workspace:test".to_string(),
            task_signature: "test signature".to_string(),
            facts: Vec::new(),
            avoid: Vec::new(),
            next_hints: targets
                .iter()
                .map(|t| Hint {
                    kind: "verify".to_string(),
                    target: (*t).to_string(),
                })
                .collect(),
            provenance: None,
        }
    }

    // ---- Default + ScrubMode parsing ----

    #[test]
    fn default_scrub_mode_is_strict() {
        assert_eq!(DEFAULT_SCRUB_MODE, ScrubMode::Strict);
    }

    #[test]
    fn from_env_str_accepts_strict_and_warn() {
        assert_eq!(
            ScrubMode::from_env_str("strict").unwrap(),
            ScrubMode::Strict
        );
        assert_eq!(ScrubMode::from_env_str("Warn").unwrap(), ScrubMode::Warn);
        assert_eq!(
            ScrubMode::from_env_str("  STRICT  ").unwrap(),
            ScrubMode::Strict
        );
    }

    #[test]
    fn from_env_str_rejects_unknown_value() {
        let err = ScrubMode::from_env_str("off").unwrap_err();
        assert!(err.contains("unknown scrub mode"));
        let err2 = ScrubMode::from_env_str("strcit").unwrap_err();
        assert!(err2.contains("unknown scrub mode"));
    }

    #[test]
    fn from_env_str_or_default_falls_back_to_strict() {
        assert_eq!(
            ScrubMode::from_env_str_or_default("bogus"),
            DEFAULT_SCRUB_MODE,
        );
    }

    // ---- 6-family positive + negative fixtures (Task 1.1 Red phase) ----

    // family 1: output_literal_json
    #[test]
    fn output_literal_json_positive_braced_pair() {
        let mut s = make_summary_with_facts(&[r#"the output is {"answer": 42}"#]);
        match scrub_action_summary(&mut s, ScrubMode::Strict) {
            ScrubAction::Hit { fields } => {
                assert!(
                    fields.iter().any(|f| f.contains("output_literal_json")),
                    "expected output_literal_json hit, got {fields:?}",
                );
            }
            ScrubAction::Pass => panic!("expected hit on {{\"answer\": 42}}"),
        }
    }

    #[test]
    fn output_literal_json_positive_multi_key() {
        let mut s = make_summary_with_facts(&[r#"emits {"score":99,"label":"ok"}"#]);
        let res = scrub_action_summary(&mut s, ScrubMode::Strict);
        assert!(matches!(res, ScrubAction::Hit { .. }));
    }

    #[test]
    fn output_literal_json_negative_plain_prose() {
        let mut s = make_summary_with_facts(&["summarize.py reads JSON files from disk"]);
        assert_eq!(
            scrub_action_summary(&mut s, ScrubMode::Strict),
            ScrubAction::Pass,
        );
    }

    // family 2: output_literal_text
    #[test]
    fn output_literal_text_positive_stdout_colon() {
        let mut s = make_summary_with_facts(&["stdout: 7"]);
        let res = scrub_action_summary(&mut s, ScrubMode::Strict);
        match res {
            ScrubAction::Hit { fields } => assert!(
                fields.iter().any(|f| f.contains("output_literal_text")),
                "fields={fields:?}",
            ),
            ScrubAction::Pass => panic!("expected hit on stdout: 7"),
        }
    }

    #[test]
    fn output_literal_text_positive_result_equals() {
        let mut s = make_summary_with_facts(&["result = hello world"]);
        let res = scrub_action_summary(&mut s, ScrubMode::Strict);
        assert!(matches!(res, ScrubAction::Hit { .. }));
    }

    #[test]
    fn output_literal_text_negative_documentation() {
        let mut s = make_summary_with_facts(&["The stdout stream is captured by pytest."]);
        assert_eq!(
            scrub_action_summary(&mut s, ScrubMode::Strict),
            ScrubAction::Pass,
        );
    }

    // family 3: solution_phrase
    #[test]
    fn solution_phrase_positive_answer_is() {
        let mut s = make_summary_with_facts(&["the answer is computed at runtime"]);
        match scrub_action_summary(&mut s, ScrubMode::Strict) {
            ScrubAction::Hit { fields } => assert!(
                fields.iter().any(|f| f.contains("solution_phrase")),
                "fields={fields:?}",
            ),
            ScrubAction::Pass => panic!("expected hit on 'the answer is'"),
        }
    }

    #[test]
    fn solution_phrase_positive_result_equals() {
        let mut s = make_summary_with_facts(&["the result equals zero on first call"]);
        let res = scrub_action_summary(&mut s, ScrubMode::Strict);
        assert!(matches!(res, ScrubAction::Hit { .. }));
    }

    #[test]
    fn solution_phrase_negative_neutral_prose() {
        let mut s = make_summary_with_facts(&["the function performs the calculation"]);
        assert_eq!(
            scrub_action_summary(&mut s, ScrubMode::Strict),
            ScrubAction::Pass,
        );
    }

    // family 4: verification_prespoil
    #[test]
    fn verification_prespoil_positive_prints_json() {
        let mut s = make_summary_with_facts(&["prints a json payload after the run"]);
        match scrub_action_summary(&mut s, ScrubMode::Strict) {
            ScrubAction::Hit { fields } => assert!(
                fields.iter().any(|f| f.contains("verification_prespoil")),
                "fields={fields:?}",
            ),
            ScrubAction::Pass => panic!("expected hit on 'prints a json payload'"),
        }
    }

    #[test]
    fn verification_prespoil_positive_stdout_will_contain() {
        let mut s = make_summary_with_facts(&["stdout will contain the answer line"]);
        let res = scrub_action_summary(&mut s, ScrubMode::Strict);
        assert!(matches!(res, ScrubAction::Hit { .. }));
    }

    #[test]
    fn verification_prespoil_negative_describes_library() {
        let mut s = make_summary_with_facts(&["uses serde_json for serialization"]);
        assert_eq!(
            scrub_action_summary(&mut s, ScrubMode::Strict),
            ScrubAction::Pass,
        );
    }

    // family 5: numeric_answer_equality
    #[test]
    fn numeric_answer_equality_positive_identifier_equals_int() {
        let mut s = make_summary_with_facts(&["score = 42"]);
        match scrub_action_summary(&mut s, ScrubMode::Strict) {
            ScrubAction::Hit { fields } => assert!(
                fields.iter().any(|f| f.contains("numeric_answer_equality")),
                "fields={fields:?}",
            ),
            ScrubAction::Pass => panic!("expected hit on 'score = 42'"),
        }
    }

    #[test]
    fn numeric_answer_equality_positive_equals_float() {
        let mut s = make_summary_with_facts(&["pi equals 3.14"]);
        let res = scrub_action_summary(&mut s, ScrubMode::Strict);
        assert!(matches!(res, ScrubAction::Hit { .. }));
    }

    #[test]
    fn numeric_answer_equality_negative_plain_word() {
        let mut s = make_summary_with_facts(&["compute the metric across runs"]);
        assert_eq!(
            scrub_action_summary(&mut s, ScrubMode::Strict),
            ScrubAction::Pass,
        );
    }

    // family 6: line_specific_change
    #[test]
    fn line_specific_change_positive_change_line_n() {
        let mut s = make_summary_with_facts(&["change line 12 to return Ok(())"]);
        match scrub_action_summary(&mut s, ScrubMode::Strict) {
            ScrubAction::Hit { fields } => assert!(
                fields.iter().any(|f| f.contains("line_specific_change")),
                "fields={fields:?}",
            ),
            ScrubAction::Pass => panic!("expected hit on 'change line 12'"),
        }
    }

    #[test]
    fn line_specific_change_positive_replace_line_n() {
        let mut s = make_summary_with_facts(&["replace line 7 with the new branch"]);
        let res = scrub_action_summary(&mut s, ScrubMode::Strict);
        assert!(matches!(res, ScrubAction::Hit { .. }));
    }

    #[test]
    fn line_specific_change_negative_generic_diff_talk() {
        let mut s = make_summary_with_facts(&["adjust the test fixtures around the parser"]);
        assert_eq!(
            scrub_action_summary(&mut s, ScrubMode::Strict),
            ScrubAction::Pass,
        );
    }

    // ---- coverage of avoid / next_hints walkers ----

    #[test]
    fn scrub_walks_avoid_section() {
        let mut s = make_summary_with_avoid(&["the answer is 99"]);
        let res = scrub_action_summary(&mut s, ScrubMode::Strict);
        match res {
            ScrubAction::Hit { fields } => assert!(
                fields.iter().any(|f| f.starts_with("avoid[0]")),
                "fields={fields:?}",
            ),
            ScrubAction::Pass => panic!("expected avoid hit"),
        }
    }

    #[test]
    fn scrub_walks_next_hints_section_targets() {
        let mut s = make_summary_with_hints(&["change line 4 here"]);
        let res = scrub_action_summary(&mut s, ScrubMode::Strict);
        match res {
            ScrubAction::Hit { fields } => assert!(
                fields.iter().any(|f| f.starts_with("next_hints[0]")),
                "fields={fields:?}",
            ),
            ScrubAction::Pass => panic!("expected next_hints hit"),
        }
    }

    // ---- mode dispatch: warn rewrites, strict does not ----

    #[test]
    fn warn_mode_replaces_hit_text_with_placeholder() {
        let mut s = make_summary_with_facts(&["the answer is 7"]);
        let res = scrub_action_summary(&mut s, ScrubMode::Warn);
        assert!(matches!(res, ScrubAction::Hit { .. }));
        assert_eq!(s.facts[0].text, SCRUB_PLACEHOLDER);
    }

    #[test]
    fn strict_mode_does_not_mutate_hit_text() {
        let original = "the answer is 7";
        let mut s = make_summary_with_facts(&[original]);
        let res = scrub_action_summary(&mut s, ScrubMode::Strict);
        assert!(matches!(res, ScrubAction::Hit { .. }));
        assert_eq!(s.facts[0].text, original);
    }

    #[test]
    fn pass_action_for_completely_neutral_summary() {
        let mut s = make_summary_with_facts(&["uses tokio for async I/O"]);
        assert_eq!(
            scrub_action_summary(&mut s, ScrubMode::Strict),
            ScrubAction::Pass,
        );
    }
}
