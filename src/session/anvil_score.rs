//! AnvilScore (Issue #456 / Epic B / DR1-001).
//!
//! Deterministic snapshot of "did this turn make verifiable progress?".
//! Computed once per turn at the orchestration layer (`agent::loop_run::turn`)
//! after `record_feedback*` / `verify_repo_progress` / auto_test have already
//! run, persisted as `SessionSnapshot.last_anvil_score`, and rendered into
//! the Reminder Sidecar prompt to give the LLM stable signal about progress.
//!
//! Module layout (DR1-001):
//!
//! - [`AnvilScore`]: the value object (12 fields, 5 lifecycle groups).
//! - [`AnvilScoreInputs`]: only the `SessionSnapshot` fields the pure function
//!   actually needs, so fixture tests don't have to instantiate the whole
//!   `SessionSnapshot` (DR1-005 / DR2-010).
//! - [`AnvilTestSummary`]: a thin view over `AutoTestResult`. #456 always
//!   passed `None`; #457 wires `auto_test::AutoTestResult` into this view
//!   via `build_anvil_test_summary` in `agent::loop_run::turn` so the
//!   session layer never observes the internal `AutoTestResult` type
//!   (DR3-002).
//! - [`AnvilScoreSnapshot`]: a non-serde lifetime-tagged enum for Reminder
//!   prompt injection that distinguishes the previous turn's persisted score
//!   from the current turn's freshly computed one (DR1-006).
//! - [`compute_anvil_score`]: the deterministic pure function.
//! - [`sanitize`] / [`deserialize_lossy_anvil_score`]: defence against
//!   user-edited / oversized / overflow `last_anvil_score` JSON (DR4-002 /
//!   DR4-003).
//! - [`AnvilScore::format_for_prompt`]: single-source-of-truth renderer with a
//!   `MAX_RENDERED_CHARS = 512` cap (DR1-003 / DR2-004 / DR4-005).

use serde::{Deserialize, Serialize};

use crate::agent::orchestration::RepoVerification;

/// Hard cap on the on-disk `last_anvil_score` JSON slice the lossy
/// deserializer will accept (DR4-003). Larger inputs are dropped to `None`
/// without parsing.
pub const MAX_ANVIL_SCORE_RAW_BYTES: usize = 64 * 1024;

/// Upper bound for any `*_count` / `*_files_changed` field that will be kept
/// after `sanitize`. Counts above this clamp are treated as malformed and
/// dropped to `None` (or, for non-`Option` counters, saturated to this cap).
pub const MAX_ANVIL_SCORE_COUNT: usize = 1_000_000;

/// Upper bound for `consecutive_no_progress_turns`. A session that genuinely
/// hits this is already in a degenerate state; the counter saturates here so
/// later math never overflows.
pub const MAX_NO_PROGRESS_TURNS: usize = 10_000;

/// 1 turn の検証可能な進捗を deterministic に集約した snapshot。
///
/// # Lifecycle Groups
///
/// field は意味的 lifecycle で 5 群に分かれる。後続実装者は群を跨いだ
/// 安易な field 追加を行わないこと。
///
/// - **outcome** (turn-local): 当 turn の検証結果
/// - **delta** (turn-local): 直前 turn baseline との差分
/// - **next-turn baseline** (turn-local 絶対値): 次 turn delta 計算の baseline
/// - **repo diff** (turn-local): RepoVerification 由来の分類別件数
/// - **counter/accumulator/flag**: その他 (session-cumulative 例外を含む)
///
/// # Default semantics
///
/// `Default` は『未計測 turn (run_turn が AnvilScore を計算する前の中間状態)』を
/// 表す。malformed deserialize の lossy 復旧では `Default::default()` ではなく
/// `Option::None` (= `last_anvil_score = None`) に落とすこと。両者を意味的に
/// 区別する (DR1-010 / 設計判断 #4 / 設計判断 #9)。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct AnvilScore {
    // ── Group 1: outcome (turn-local) ──
    pub build_passed: Option<bool>,
    pub tests_passed: Option<bool>,

    // ── Group 2: delta (turn-local, 直前 turn 比) ──
    pub compile_errors_delta: Option<i32>,
    pub test_failures_delta: Option<i32>,

    // ── Group 3: next-turn baseline (turn-local 絶対値) ──
    // #456 時点では fixture test のみが Some(N) を生成する。
    // 本番経路で Some(N) を埋めるのは #457 (auto_test wire-up)。
    // 型としては #456 で導入する必要がある (delta baseline を session.json に
    // persist する責務を AnvilScore が担うため、後出し追加は backward compat の
    // serde shape を変更してしまう、DR1-004)。
    pub compile_error_count: Option<usize>,
    pub test_failure_count: Option<usize>,

    // ── Group 4: repo diff (turn-local) ──
    pub implementation_files_changed: Option<usize>,
    pub test_files_changed: Option<usize>,
    pub setup_files_changed: Option<usize>,

    // ── Group 5: counter / accumulator / flag ──
    /// turn-local counter。turn 開始 0 reset。`UnsafeCommandBlocked` 発火点で
    /// `+= 1`。
    pub unsafe_actions_blocked: usize,
    /// session-cumulative 値の denormalize copy。Reminder 注入便宜のため
    /// AnvilScore に持つ例外。正本は
    /// `SessionSnapshot.consecutive_no_progress_turns`。拡大解釈してこれ以上
    /// session-scoped 値を AnvilScore に追加しないこと。
    pub consecutive_no_progress_turns: usize,
    /// turn-local bool。少なくとも 1 件の repo edit が成功し、impl/test diff が
    /// 非ゼロの turn に true。
    pub user_visible_artifact: bool,
}

impl AnvilScore {
    /// Renderer が prompt 投入用に行末で truncate する文字数上限。
    /// `WorkingMemory::MAX_ACTIVE_PRECAUTIONS_CHARS` (precaution renderer 既存
    /// pattern) と揃えるため、associated const として `AnvilScore` に持つ
    /// (DR2-004)。自由 module-level const は採用しない。
    pub const MAX_RENDERED_CHARS: usize = 512;

    /// Renders for prompt injection. Always truncates at
    /// [`Self::MAX_RENDERED_CHARS`] (DR1-003).
    ///
    /// Output is a fixed list of ASCII `key: value` lines:
    /// - keys are hard-coded snake_case identifiers from this struct.
    /// - values are `true` / `false` / `none` / decimal integer only.
    /// - no path / session_id / command / stdout / stderr / user text /
    ///   workspace_key is rendered (DR4-005 / DR4-006).
    /// - no newlines or control characters appear inside values; the lines
    ///   themselves are the only `\n` in the output.
    ///
    /// Caller MUST NOT re-truncate. The renderer is the single source of truth
    /// for both shape and budget.
    pub fn format_for_prompt(&self) -> String {
        fn render_opt_bool(v: Option<bool>) -> &'static str {
            match v {
                Some(true) => "true",
                Some(false) => "false",
                None => "none",
            }
        }
        fn render_opt_i32(v: Option<i32>) -> String {
            match v {
                Some(n) => n.to_string(),
                None => "none".to_string(),
            }
        }
        fn render_opt_usize(v: Option<usize>) -> String {
            match v {
                Some(n) => n.to_string(),
                None => "none".to_string(),
            }
        }

        // Field order is fixed and matches the struct declaration order so
        // truncation at MAX_RENDERED_CHARS deterministically drops the
        // tail-most (Group 5) fields first. Security-relevant counters
        // (unsafe_actions_blocked) are rendered before user_visible_artifact
        // so they survive truncation in normal cases.
        let mut buf = String::with_capacity(256);
        buf.push_str("build_passed: ");
        buf.push_str(render_opt_bool(self.build_passed));
        buf.push('\n');
        buf.push_str("tests_passed: ");
        buf.push_str(render_opt_bool(self.tests_passed));
        buf.push('\n');
        buf.push_str("compile_errors_delta: ");
        buf.push_str(&render_opt_i32(self.compile_errors_delta));
        buf.push('\n');
        buf.push_str("test_failures_delta: ");
        buf.push_str(&render_opt_i32(self.test_failures_delta));
        buf.push('\n');
        buf.push_str("compile_error_count: ");
        buf.push_str(&render_opt_usize(self.compile_error_count));
        buf.push('\n');
        buf.push_str("test_failure_count: ");
        buf.push_str(&render_opt_usize(self.test_failure_count));
        buf.push('\n');
        buf.push_str("implementation_files_changed: ");
        buf.push_str(&render_opt_usize(self.implementation_files_changed));
        buf.push('\n');
        buf.push_str("test_files_changed: ");
        buf.push_str(&render_opt_usize(self.test_files_changed));
        buf.push('\n');
        buf.push_str("setup_files_changed: ");
        buf.push_str(&render_opt_usize(self.setup_files_changed));
        buf.push('\n');
        buf.push_str("unsafe_actions_blocked: ");
        buf.push_str(&self.unsafe_actions_blocked.to_string());
        buf.push('\n');
        buf.push_str("consecutive_no_progress_turns: ");
        buf.push_str(&self.consecutive_no_progress_turns.to_string());
        buf.push('\n');
        buf.push_str("user_visible_artifact: ");
        buf.push_str(if self.user_visible_artifact {
            "true"
        } else {
            "false"
        });

        truncate_chars(buf, Self::MAX_RENDERED_CHARS)
    }
}

/// Truncate `s` so the resulting string contains at most `max_chars` Unicode
/// scalar values. Bytes-aware so we never split a multi-byte UTF-8 sequence
/// (which would otherwise corrupt the prompt).
fn truncate_chars(s: String, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s;
    }
    s.chars().take(max_chars).collect()
}

/// Inputs to `compute_anvil_score`. Only the four `SessionSnapshot` fields the
/// pure function actually depends on, so fixture tests don't need to build
/// `SessionSnapshot::default()` (DR1-005 / DR2-010).
#[derive(Debug, Clone)]
pub struct AnvilScoreInputs<'a> {
    pub unsafe_blocks_this_turn: usize,
    pub repo_edit_succeeded_this_turn: bool,
    pub consecutive_no_progress_turns: usize,
    /// Borrow of the previous turn's persisted score, used only for delta
    /// baselines (DR1-005).
    pub prev: Option<&'a AnvilScore>,
}

/// Auto-test result view used by `compute_anvil_score`. #456 always passes
/// `None`; #457 will convert `auto_test::AutoTestResult` into this view at the
/// orchestration boundary so the session layer never sees the agent-internal
/// `AutoTestResult` type (DR3-002).
#[derive(Debug, Clone, Default)]
pub struct AnvilTestSummary {
    pub build_passed: Option<bool>,
    pub tests_passed: Option<bool>,
    pub compile_error_count: Option<usize>,
    pub test_failure_count: Option<usize>,
}

/// Reminder prompt injection wrapper. Distinguishes:
///
/// - `PreviousTurn`: the score persisted from the *previous* turn, passed by
///   the iteration-internal Reminder hook so the sidecar can compare what
///   the agent already knew.
/// - `CurrentTurn`: the score freshly computed for *this* turn, passed by the
///   post-loop Reminder hook so the sidecar sees the latest verification
///   result.
///
/// The variant is rendered into a `[Previous AnvilScore]` / `[Current
/// AnvilScore]` label by `build_reminder_prompt`, preventing the sidecar from
/// silently mixing them up (DR1-006).
///
/// Lifetime-tagged + reference-bearing → not `Serialize` / `Deserialize`. In-
/// memory only; flows exclusively through `ReminderInputs` (DR2-007).
#[derive(Debug, Clone, Copy)]
pub enum AnvilScoreSnapshot<'a> {
    PreviousTurn(&'a AnvilScore),
    CurrentTurn(&'a AnvilScore),
}

impl<'a> AnvilScoreSnapshot<'a> {
    pub fn label(&self) -> &'static str {
        match self {
            AnvilScoreSnapshot::PreviousTurn(_) => "[Previous AnvilScore]",
            AnvilScoreSnapshot::CurrentTurn(_) => "[Current AnvilScore]",
        }
    }

    pub fn score(&self) -> &'a AnvilScore {
        match self {
            AnvilScoreSnapshot::PreviousTurn(s) | AnvilScoreSnapshot::CurrentTurn(s) => s,
        }
    }
}

/// Compute the AnvilScore for the current turn (DR1-001 / DR1-005 / DR3-002).
///
/// Pure / deterministic / no I/O. fixture tests can call this directly. The
/// caller (turn.rs) gathers inputs from `SessionSnapshot` / `RepoVerification`
/// / `AutoTestResult` (via `build_anvil_test_summary`) and passes them in.
///
/// `repo` is `None` when this turn never ran `verify_repo_progress`; in that
/// case all `*_files_changed` fields stay `None` (the absence is observable
/// downstream rather than collapsed to `Some(0)`).
///
/// `auto_test` is `None` for the `Tester` / `NoVerifier` / `Skip` /
/// `TransportError` branches (no `AutoTestResult` was produced). When
/// `Some`, the four supplied fields populate the matching AnvilScore fields
/// verbatim. Wiring landed in #457 (`build_anvil_test_summary` adapter in
/// `agent::loop_run::turn`).
pub fn compute_anvil_score(
    inputs: &AnvilScoreInputs<'_>,
    repo: Option<&RepoVerification>,
    auto_test: Option<&AnvilTestSummary>,
) -> AnvilScore {
    // Group 1 / Group 3 / partial Group 2: directly from auto_test.
    let build_passed = auto_test.and_then(|t| t.build_passed);
    let tests_passed = auto_test.and_then(|t| t.tests_passed);
    let compile_error_count = auto_test.and_then(|t| t.compile_error_count);
    let test_failure_count = auto_test.and_then(|t| t.test_failure_count);

    // Group 2: deltas. Compute in i128 to avoid usize underflow / i32 overflow
    // (DR4-002), then drop to None when out of i32 range. None when either
    // operand is missing.
    fn delta_i32(curr: Option<usize>, prev: Option<usize>) -> Option<i32> {
        let curr = curr? as i128;
        let prev = prev? as i128;
        let diff = curr - prev;
        if (i32::MIN as i128..=i32::MAX as i128).contains(&diff) {
            Some(diff as i32)
        } else {
            None
        }
    }
    let prev_compile = inputs.prev.and_then(|p| p.compile_error_count);
    let prev_test = inputs.prev.and_then(|p| p.test_failure_count);
    let compile_errors_delta = delta_i32(compile_error_count, prev_compile);
    let test_failures_delta = delta_i32(test_failure_count, prev_test);

    // Group 4: repo diff classification (DR1-007 helper). When verify_repo_progress
    // didn't run, all three stay None so downstream knows the data is missing
    // rather than treating absent verification as zero diff.
    let (impl_changed, test_changed, setup_changed) = match repo {
        Some(r) => (
            Some(r.implementation_files_changed),
            Some(r.test_files_changed),
            Some(r.setup_files_changed),
        ),
        None => (None, None, None),
    };

    // Group 5: counters / flag.
    let unsafe_actions_blocked = inputs.unsafe_blocks_this_turn;
    let consecutive_no_progress_turns = inputs.consecutive_no_progress_turns;
    // user_visible_artifact: at least one Write/Edit succeeded AND
    // RepoVerification observed a non-zero impl-or-test diff. This is an
    // intersection rather than union: a Write that happened but produced no
    // observable diff (e.g. wrote to .anvil/ which is filtered) doesn't count
    // as user-visible.
    let user_visible_artifact = inputs.repo_edit_succeeded_this_turn
        && repo
            .map(|r| r.implementation_files_changed > 0 || r.test_files_changed > 0)
            .unwrap_or(false);

    AnvilScore {
        build_passed,
        tests_passed,
        compile_errors_delta,
        test_failures_delta,
        compile_error_count,
        test_failure_count,
        implementation_files_changed: impl_changed,
        test_files_changed: test_changed,
        setup_files_changed: setup_changed,
        unsafe_actions_blocked,
        consecutive_no_progress_turns,
        user_visible_artifact,
    }
}

/// Sanitize an AnvilScore loaded from untrusted session.json (DR4-002).
///
/// Returns `None` when the input is so degenerate that we'd rather drop the
/// whole field than try to clean it (e.g. a usize already past
/// `MAX_ANVIL_SCORE_COUNT` that we can't safely subtract from). Otherwise
/// returns the same score with out-of-range values replaced by `None` /
/// saturating clamps.
pub(crate) fn sanitize(mut score: AnvilScore) -> Option<AnvilScore> {
    fn clamp_opt_count(v: &mut Option<usize>) {
        if let Some(n) = *v
            && n > MAX_ANVIL_SCORE_COUNT
        {
            *v = None;
        }
    }
    // *_count / *_files_changed: drop above MAX_ANVIL_SCORE_COUNT.
    clamp_opt_count(&mut score.compile_error_count);
    clamp_opt_count(&mut score.test_failure_count);
    clamp_opt_count(&mut score.implementation_files_changed);
    clamp_opt_count(&mut score.test_files_changed);
    clamp_opt_count(&mut score.setup_files_changed);

    // *_delta: drop i32::MIN as a special-cased extreme value. Any value
    // already inside i32 range (the type itself enforces) is fine.
    if score.compile_errors_delta == Some(i32::MIN) {
        score.compile_errors_delta = None;
    }
    if score.test_failures_delta == Some(i32::MIN) {
        score.test_failures_delta = None;
    }

    // unsafe_actions_blocked: saturating clamp to MAX_ANVIL_SCORE_COUNT.
    if score.unsafe_actions_blocked > MAX_ANVIL_SCORE_COUNT {
        score.unsafe_actions_blocked = MAX_ANVIL_SCORE_COUNT;
    }
    // consecutive_no_progress_turns: saturating clamp.
    if score.consecutive_no_progress_turns > MAX_NO_PROGRESS_TURNS {
        score.consecutive_no_progress_turns = MAX_NO_PROGRESS_TURNS;
    }
    Some(score)
}

/// Field-level lossy deserializer for `SessionSnapshot.last_anvil_score`
/// (DR1-008 / DR3-001). Drops malformed / oversized JSON to `None` instead of
/// failing the whole `SessionSnapshot` deserialize so:
///
/// - `SessionStore::load_or_new` continues to load the rest of the session
///   (resume keeps working);
/// - `iter_session_dirs` doesn't drop the entire session directory (sessions
///   list / clean continues to see the entry).
///
/// Path: parse `last_anvil_score` as a `RawValue` first to get a slice of the
/// original JSON without expanding to a `serde_json::Value`. Reject anything
/// over `MAX_ANVIL_SCORE_RAW_BYTES` (DR4-003), then attempt the inner parse.
/// Pass through `sanitize` to clamp / drop overflow numerics (DR4-002).
pub fn deserialize_lossy_anvil_score<'de, D>(
    deserializer: D,
) -> Result<Option<AnvilScore>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw: Option<Box<serde_json::value::RawValue>> = Option::deserialize(deserializer)?;
    Ok(raw
        .filter(|r| r.get().len() <= MAX_ANVIL_SCORE_RAW_BYTES)
        .and_then(|r| serde_json::from_str::<AnvilScore>(r.get()).ok())
        .and_then(sanitize))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::orchestration::RepoVerification;

    fn empty_inputs<'a>() -> AnvilScoreInputs<'a> {
        AnvilScoreInputs {
            unsafe_blocks_this_turn: 0,
            repo_edit_succeeded_this_turn: false,
            consecutive_no_progress_turns: 0,
            prev: None,
        }
    }

    fn repo_with(impls: usize, tests: usize, setups: usize) -> RepoVerification {
        RepoVerification {
            changed_files: Vec::new(),
            implementation_files_changed: impls,
            test_files_changed: tests,
            setup_files_changed: setups,
            other_files_changed: 0,
            deleted_files_changed: 0,
        }
    }

    // ---------------------------------------------------------------
    // compute_anvil_score deterministic / fixture tests
    // ---------------------------------------------------------------

    #[test]
    fn compute_with_no_inputs_returns_default_shape() {
        let score = compute_anvil_score(&empty_inputs(), None, None);
        assert_eq!(score.build_passed, None);
        assert_eq!(score.tests_passed, None);
        assert_eq!(score.compile_errors_delta, None);
        assert_eq!(score.test_failures_delta, None);
        assert_eq!(score.compile_error_count, None);
        assert_eq!(score.test_failure_count, None);
        assert_eq!(score.implementation_files_changed, None);
        assert_eq!(score.test_files_changed, None);
        assert_eq!(score.setup_files_changed, None);
        assert_eq!(score.unsafe_actions_blocked, 0);
        assert_eq!(score.consecutive_no_progress_turns, 0);
        assert!(!score.user_visible_artifact);
    }

    #[test]
    fn compute_records_repo_diff_when_repo_present() {
        let repo = repo_with(2, 1, 0);
        let score = compute_anvil_score(&empty_inputs(), Some(&repo), None);
        assert_eq!(score.implementation_files_changed, Some(2));
        assert_eq!(score.test_files_changed, Some(1));
        assert_eq!(score.setup_files_changed, Some(0));
    }

    #[test]
    fn compute_user_visible_artifact_requires_edit_and_diff() {
        // Only edit succeeded, diff is zero → false
        let inputs = AnvilScoreInputs {
            repo_edit_succeeded_this_turn: true,
            ..empty_inputs()
        };
        let score = compute_anvil_score(&inputs, Some(&repo_with(0, 0, 1)), None);
        assert!(!score.user_visible_artifact);

        // Both edit and impl diff → true
        let score = compute_anvil_score(&inputs, Some(&repo_with(1, 0, 0)), None);
        assert!(score.user_visible_artifact);

        // edit + test diff (no impl) → true
        let score = compute_anvil_score(&inputs, Some(&repo_with(0, 1, 0)), None);
        assert!(score.user_visible_artifact);

        // edit succeeded but no repo verification → false (signal absent)
        let score = compute_anvil_score(&inputs, None, None);
        assert!(!score.user_visible_artifact);
    }

    #[test]
    fn compute_unsafe_counter_propagates() {
        let inputs = AnvilScoreInputs {
            unsafe_blocks_this_turn: 3,
            ..empty_inputs()
        };
        let score = compute_anvil_score(&inputs, None, None);
        assert_eq!(score.unsafe_actions_blocked, 3);
    }

    #[test]
    fn compute_consecutive_no_progress_propagates() {
        let inputs = AnvilScoreInputs {
            consecutive_no_progress_turns: 4,
            ..empty_inputs()
        };
        let score = compute_anvil_score(&inputs, None, None);
        assert_eq!(score.consecutive_no_progress_turns, 4);
    }

    #[test]
    fn compute_deltas_use_prev_baseline() {
        let prev = AnvilScore {
            compile_error_count: Some(5),
            test_failure_count: Some(2),
            ..Default::default()
        };
        let inputs = AnvilScoreInputs {
            prev: Some(&prev),
            ..empty_inputs()
        };
        let auto_test = AnvilTestSummary {
            compile_error_count: Some(3),
            test_failure_count: Some(2),
            ..Default::default()
        };
        let score = compute_anvil_score(&inputs, None, Some(&auto_test));
        assert_eq!(score.compile_errors_delta, Some(-2));
        assert_eq!(score.test_failures_delta, Some(0));
        assert_eq!(score.compile_error_count, Some(3));
        assert_eq!(score.test_failure_count, Some(2));
    }

    #[test]
    fn compute_delta_none_when_either_side_missing() {
        let prev = AnvilScore {
            compile_error_count: None,
            test_failure_count: Some(2),
            ..Default::default()
        };
        let inputs = AnvilScoreInputs {
            prev: Some(&prev),
            ..empty_inputs()
        };
        let auto_test = AnvilTestSummary {
            compile_error_count: Some(3),
            test_failure_count: None,
            ..Default::default()
        };
        let score = compute_anvil_score(&inputs, None, Some(&auto_test));
        assert_eq!(score.compile_errors_delta, None);
        assert_eq!(score.test_failures_delta, None);
    }

    #[test]
    fn compute_pass_results_propagate_from_auto_test() {
        let auto_test = AnvilTestSummary {
            build_passed: Some(true),
            tests_passed: Some(false),
            ..Default::default()
        };
        let score = compute_anvil_score(&empty_inputs(), None, Some(&auto_test));
        assert_eq!(score.build_passed, Some(true));
        assert_eq!(score.tests_passed, Some(false));
    }

    /// AC: test 未実行時に `tests_passed = None` になる
    #[test]
    fn compute_tests_passed_none_when_no_auto_test() {
        let score = compute_anvil_score(&empty_inputs(), None, None);
        assert_eq!(score.tests_passed, None);
    }

    /// Performance smoke test: the pure function must be well under 5ms even
    /// when called repeatedly. We don't assert a timing budget here (CI
    /// jitter), but we do call it 1k times to make sure the API stays cheap.
    #[test]
    fn compute_is_cheap_to_call() {
        let repo = repo_with(2, 1, 0);
        let auto_test = AnvilTestSummary {
            build_passed: Some(true),
            tests_passed: Some(true),
            compile_error_count: Some(0),
            test_failure_count: Some(0),
        };
        for _ in 0..1_000 {
            let _ = compute_anvil_score(&empty_inputs(), Some(&repo), Some(&auto_test));
        }
    }

    // ---------------------------------------------------------------
    // format_for_prompt rendering / truncation / secrecy
    // ---------------------------------------------------------------

    #[test]
    fn format_for_prompt_renders_fixed_keys() {
        let score = AnvilScore::default();
        let out = score.format_for_prompt();
        for key in [
            "build_passed:",
            "tests_passed:",
            "compile_errors_delta:",
            "test_failures_delta:",
            "compile_error_count:",
            "test_failure_count:",
            "implementation_files_changed:",
            "test_files_changed:",
            "setup_files_changed:",
            "unsafe_actions_blocked:",
            "consecutive_no_progress_turns:",
            "user_visible_artifact:",
        ] {
            assert!(out.contains(key), "missing key {key} in:\n{out}");
        }
    }

    #[test]
    fn format_for_prompt_uses_only_safe_value_alphabet() {
        let score = AnvilScore {
            build_passed: Some(true),
            tests_passed: Some(false),
            compile_errors_delta: Some(-3),
            compile_error_count: Some(7),
            test_failure_count: Some(2),
            implementation_files_changed: Some(1),
            test_files_changed: Some(0),
            setup_files_changed: Some(0),
            unsafe_actions_blocked: 1,
            consecutive_no_progress_turns: 4,
            user_visible_artifact: true,
            ..Default::default()
        };
        let out = score.format_for_prompt();
        // Each value (everything after `: `) must be in the allowlist.
        for line in out.lines() {
            let value = line.split_once(": ").map(|(_, v)| v).unwrap_or("");
            let ok = value == "true"
                || value == "false"
                || value == "none"
                || value.parse::<i64>().is_ok();
            assert!(ok, "value {value:?} on line {line:?} not allowlisted");
            // No control chars / no `<`, `>`, `/` injection chars.
            for c in value.chars() {
                assert!(
                    !c.is_control(),
                    "control char in value {value:?} on line {line:?}"
                );
            }
        }
        assert!(!out.contains('<'));
        assert!(!out.contains('>'));
    }

    #[test]
    fn format_for_prompt_omits_path_session_command() {
        // A score that, if naively serialized via Debug / serde_json, would
        // not contain these tokens. We verify the renderer never emits any of
        // them itself (DR4-005 / DR4-006). `user_visible_artifact` is a
        // legitimate key, so we don't gate on bare `user`.
        let score = AnvilScore::default();
        let out = score.format_for_prompt();
        for forbidden in [
            "/",
            "session_id",
            "command",
            "stdout",
            "stderr",
            "user_task",
            "user_text",
            "user_prompt",
            "workspace_key",
            ".rs",
            ".ts",
            ".py",
        ] {
            assert!(
                !out.contains(forbidden),
                "unexpected `{forbidden}` in renderer output:\n{out}"
            );
        }
    }

    #[test]
    fn format_for_prompt_truncates_at_max_rendered_chars() {
        // Default render is well under 512; force a value width that would
        // push us over the cap and verify truncation is applied.
        // We do this by re-running the renderer against a score whose values
        // are all in their longest representation. With i32::MAX-ish numbers
        // and full Some(...) coverage, the renderer string is still ~300 chars
        // because keys are fixed-width. So we additionally pin truncation by
        // calling truncate_chars directly with a known oversize value.
        let huge = "x".repeat(AnvilScore::MAX_RENDERED_CHARS + 100);
        let truncated = truncate_chars(huge, AnvilScore::MAX_RENDERED_CHARS);
        assert_eq!(truncated.chars().count(), AnvilScore::MAX_RENDERED_CHARS);
    }

    #[test]
    fn format_for_prompt_under_max_for_typical_score() {
        let score = AnvilScore {
            build_passed: Some(true),
            tests_passed: Some(true),
            compile_errors_delta: Some(-2),
            test_failures_delta: Some(-1),
            compile_error_count: Some(0),
            test_failure_count: Some(0),
            implementation_files_changed: Some(2),
            test_files_changed: Some(1),
            setup_files_changed: Some(0),
            unsafe_actions_blocked: 1,
            consecutive_no_progress_turns: 0,
            user_visible_artifact: true,
        };
        let out = score.format_for_prompt();
        assert!(
            out.chars().count() <= AnvilScore::MAX_RENDERED_CHARS,
            "render exceeded cap"
        );
    }

    // ---------------------------------------------------------------
    // sanitize / deserialize_lossy_anvil_score
    // ---------------------------------------------------------------

    #[test]
    fn sanitize_drops_oversized_count() {
        let score = AnvilScore {
            compile_error_count: Some(MAX_ANVIL_SCORE_COUNT + 1),
            ..Default::default()
        };
        let sanitized = sanitize(score).expect("some");
        assert_eq!(sanitized.compile_error_count, None);
    }

    #[test]
    fn sanitize_keeps_normal_count() {
        let score = AnvilScore {
            compile_error_count: Some(42),
            test_failure_count: Some(7),
            ..Default::default()
        };
        let sanitized = sanitize(score.clone()).expect("some");
        assert_eq!(sanitized.compile_error_count, Some(42));
        assert_eq!(sanitized.test_failure_count, Some(7));
    }

    #[test]
    fn sanitize_drops_i32_min_delta() {
        let score = AnvilScore {
            compile_errors_delta: Some(i32::MIN),
            test_failures_delta: Some(i32::MIN),
            ..Default::default()
        };
        let sanitized = sanitize(score).expect("some");
        assert_eq!(sanitized.compile_errors_delta, None);
        assert_eq!(sanitized.test_failures_delta, None);
    }

    #[test]
    fn sanitize_clamps_unsafe_actions_blocked() {
        let score = AnvilScore {
            unsafe_actions_blocked: usize::MAX,
            ..Default::default()
        };
        let sanitized = sanitize(score).expect("some");
        assert_eq!(sanitized.unsafe_actions_blocked, MAX_ANVIL_SCORE_COUNT);
    }

    #[test]
    fn sanitize_clamps_no_progress_turns() {
        let score = AnvilScore {
            consecutive_no_progress_turns: usize::MAX,
            ..Default::default()
        };
        let sanitized = sanitize(score).expect("some");
        assert_eq!(
            sanitized.consecutive_no_progress_turns,
            MAX_NO_PROGRESS_TURNS
        );
    }

    // ---------------------------------------------------------------
    // AnvilScoreSnapshot helpers
    // ---------------------------------------------------------------

    #[test]
    fn anvil_score_snapshot_label_distinguishes_variants() {
        let s = AnvilScore::default();
        assert_eq!(
            AnvilScoreSnapshot::PreviousTurn(&s).label(),
            "[Previous AnvilScore]"
        );
        assert_eq!(
            AnvilScoreSnapshot::CurrentTurn(&s).label(),
            "[Current AnvilScore]"
        );
    }

    #[test]
    fn anvil_score_partial_json_parses_with_defaults() {
        // With `#[serde(default)]` on the struct, missing fields default to
        // their `Default` value rather than failing the whole parse. This is
        // required for forward / backward compat when older anvil writes
        // shorter scores or when malformed `last_anvil_score` slips into the
        // RawValue path before `sanitize`.
        let payload = r#"{"compile_error_count":1000001,"unsafe_actions_blocked":1000005}"#;
        let parsed = serde_json::from_str::<AnvilScore>(payload).expect("parses");
        let sanitized = sanitize(parsed).expect("sanitize some");
        assert_eq!(sanitized.compile_error_count, None);
        assert_eq!(sanitized.unsafe_actions_blocked, MAX_ANVIL_SCORE_COUNT);
    }

    #[test]
    fn anvil_score_snapshot_score_returns_inner() {
        let s = AnvilScore {
            unsafe_actions_blocked: 9,
            ..Default::default()
        };
        assert_eq!(AnvilScoreSnapshot::PreviousTurn(&s).score(), &s);
        assert_eq!(AnvilScoreSnapshot::CurrentTurn(&s).score(), &s);
    }
}
