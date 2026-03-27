//! Shared contract types used across modules.
//!
//! These types form the schema for snapshots, console rendering, and
//! persistent session state.  They are intentionally plain data with
//! `Serialize`/`Deserialize` support.

pub mod tokens;

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Sub-agent payload types (Issue #129)
// ---------------------------------------------------------------------------

/// Sub-agent の実行終了理由。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminationReason {
    /// ANVIL_FINAL未発火時のフォールバック完了検出 (Issue #159).
    FallbackCompleted,
    Timeout,
    MaxIterations,
    LoopDetected,
    /// Tool call count limit reached (Issue #172).
    MaxToolCalls,
    /// Normal completion. Also serves as fallback for unknown variants via `#[serde(other)]`.
    #[default]
    #[serde(other)]
    Completed,
}

impl std::fmt::Display for TerminationReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Completed => write!(f, "completed"),
            Self::Timeout => write!(f, "timeout"),
            Self::MaxIterations => write!(f, "max_iterations"),
            Self::LoopDetected => write!(f, "loop_detected"),
            Self::MaxToolCalls => write!(f, "max_tool_calls"),
            Self::FallbackCompleted => write!(f, "fallback_completed"),
        }
    }
}

/// Sub-agent が探索で発見した個別の知見。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    /// 発見事項の短いタイトル
    pub title: String,
    /// 根拠を含む詳細説明
    pub detail: String,
    /// 関連するファイルパス、シンボル名、行参照など
    pub related_code: Vec<String>,
}

/// Sub-agent の構造化返却 payload。
/// 成功時もエラー時も同一構造で返す。
/// termination_reason / error はシステムが設定するフィールドであり、LLM出力からは設定されない。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubAgentPayload {
    /// 探索で発見した関連ファイルパス
    pub found_files: Vec<String>,
    /// 主要な発見事項
    pub key_findings: Vec<Finding>,
    /// LLMが生成したフリーテキストのサマリー
    pub raw_summary: String,
    /// 結果の信頼度（0.0-1.0、clamp適用）
    /// 現在は情報提供目的のみ。将来的に親エージェントが低信頼度時の再探索判断に使用可能。
    #[serde(default)]
    pub confidence: Option<f32>,
    /// 実行終了理由（システムが設定、LLM出力には含まない）
    #[serde(default)]
    pub termination_reason: TerminationReason,
    /// エラー時のメッセージ（成功時はNone）
    #[serde(default)]
    pub error: Option<String>,
}

impl SubAgentPayload {
    /// フォールバック用コンストラクタ（JSON パース失敗時や部分結果構築時に使用）
    pub fn fallback(raw_summary: String, reason: TerminationReason) -> Self {
        Self {
            found_files: vec![],
            key_findings: vec![],
            raw_summary,
            confidence: None,
            termination_reason: reason,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Application lifecycle types
// ---------------------------------------------------------------------------

/// The runtime lifecycle states of the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RuntimeState {
    Ready,
    Thinking,
    Working,
    AwaitingApproval,
    Interrupted,
    Done,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AppEvent {
    ConfigLoaded,
    ProviderBootstrapped,
    StartupCompleted,
    StateChanged,
    PlanItemAdded,
    PlanFocusChanged,
    PlanCleared,
    PlanCheckpointSaved,
    SessionCompacted,
    SessionLoaded,
    SessionSaved,
    SessionNormalizedAfterInterrupt,
    UndoExecuted,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusView {
    pub line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanView {
    pub items: Vec<String>,
    pub active_index: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalView {
    pub tool_name: String,
    pub summary: String,
    pub risk: String,
    pub tool_call_id: String,
    #[serde(skip)]
    pub diff_preview: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InterruptView {
    pub interrupted_what: String,
    pub saved_status: String,
    pub next_actions: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolLogView {
    pub tool_name: String,
    pub action: String,
    pub target: String,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
}

/// Context usage warning level based on threshold evaluation.
///
/// Used as `Option<ContextWarningLevel>` where `None` means no warning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContextWarningLevel {
    /// Usage >= 80%: warning
    Warning,
    /// Usage >= 90%: critical
    Critical,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextUsageView {
    pub estimated_tokens: usize,
    pub max_tokens: u32,
}

impl ContextUsageView {
    /// Context usage ratio clamped to 0.0..=1.0. Returns 0.0 if max_tokens is 0.
    pub fn usage_ratio(&self) -> f64 {
        if self.max_tokens == 0 {
            return 0.0;
        }
        let ratio = self.estimated_tokens as f64 / self.max_tokens as f64;
        ratio.clamp(0.0, 1.0)
    }

    /// Warning level based on usage thresholds (>=0.9 Critical, >=0.8 Warning, else None).
    pub fn warning_level(&self) -> Option<ContextWarningLevel> {
        let ratio = self.usage_ratio();
        if ratio >= 0.9 {
            Some(ContextWarningLevel::Critical)
        } else if ratio >= 0.8 {
            Some(ContextWarningLevel::Warning)
        } else {
            None
        }
    }

    /// Usage percentage for display (0..=100).
    pub fn usage_percent(&self) -> u32 {
        (self.usage_ratio() * 100.0).round() as u32
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct InferencePerformanceView {
    /// tokens/sec * 10 (integer). e.g. 32.5 tok/s -> 325
    pub tokens_per_sec_tenths: Option<u64>,
    /// Generated token count (for session persistence / debug)
    pub eval_tokens: Option<u64>,
    /// Evaluation time in milliseconds (for session persistence / debug)
    pub eval_duration_ms: Option<u64>,
    /// Actual prompt token count from provider response.
    /// Ollama: `prompt_eval_count`, OpenAI: `prompt_tokens`.
    #[serde(default)]
    pub prompt_tokens: Option<u64>,
}

impl InferencePerformanceView {
    /// Return a formatted string for TUI display.
    pub fn formatted_tokens_per_sec(&self) -> Option<String> {
        self.tokens_per_sec_tenths
            .map(|tenths| format!("{}.{}tok/s", tenths / 10, tenths % 10))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ConsoleMessageRole {
    User,
    Assistant,
    Tool,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleMessageView {
    pub role: ConsoleMessageRole,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsoleRenderContext {
    pub snapshot: AppStateSnapshot,
    pub model_name: String,
    pub messages: Vec<ConsoleMessageView>,
    pub history_summary: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppStateSnapshot {
    /// Current lifecycle state. Set in every state.
    pub state: RuntimeState,
    /// Most recent event that caused this snapshot. Set in every state.
    #[serde(default)]
    pub last_event: Option<AppEvent>,
    /// Human-readable status line. Set in every state.
    #[serde(default)]
    pub status: StatusView,
    /// Active plan items. Used in: Thinking, Working, Done.
    #[serde(default)]
    pub plan: Option<PlanView>,
    /// LLM reasoning steps. Used in: Thinking.
    #[serde(default)]
    pub reasoning_summary: Vec<String>,
    /// Pending approval details. Used in: AwaitingApproval.
    #[serde(default)]
    pub approval: Option<ApprovalView>,
    /// Interrupt details. Used in: Interrupted.
    #[serde(default)]
    pub interrupt: Option<InterruptView>,
    /// Tool execution log entries. Used in: Working, Done.
    #[serde(default)]
    pub tool_logs: Vec<ToolLogView>,
    /// Wall-clock milliseconds for the current turn. Used in: Thinking, Working, Done, Interrupted, Error.
    #[serde(default)]
    pub elapsed_ms: Option<u128>,
    /// Token budget usage. Set in every state.
    #[serde(default)]
    pub context_usage: Option<ContextUsageView>,
    /// Summary of what was accomplished. Used in: Done.
    #[serde(default)]
    pub completion_summary: Option<String>,
    /// Session persistence status. Used in: Done, Interrupted.
    #[serde(default)]
    pub saved_status: Option<String>,
    /// Error description. Used in: Error.
    #[serde(default)]
    pub error_summary: Option<String>,
    /// Suggested recovery actions. Used in: Error, Interrupted.
    #[serde(default)]
    pub recommended_actions: Vec<String>,
    /// Context overflow warning level. Used in: Done.
    #[serde(default)]
    pub context_warning: Option<ContextWarningLevel>,
    /// Inference performance metrics. Used in: Done.
    #[serde(default)]
    pub inference_performance: Option<InferencePerformanceView>,
}

impl AppStateSnapshot {
    pub fn new(state: RuntimeState) -> Self {
        Self {
            state,
            last_event: None,
            status: StatusView {
                line: String::new(),
            },
            plan: None,
            reasoning_summary: Vec::new(),
            approval: None,
            interrupt: None,
            tool_logs: Vec::new(),
            elapsed_ms: None,
            context_usage: None,
            completion_summary: None,
            saved_status: None,
            error_summary: None,
            recommended_actions: Vec::new(),
            context_warning: None,
            inference_performance: None,
        }
    }

    pub fn with_event(mut self, event: AppEvent) -> Self {
        self.last_event = Some(event);
        self
    }

    pub fn with_status(mut self, status: String) -> Self {
        self.status = StatusView { line: status };
        self
    }

    pub fn with_plan(mut self, items: Vec<String>, active_index: Option<usize>) -> Self {
        self.plan = Some(PlanView {
            items,
            active_index,
        });
        self
    }

    pub fn with_reasoning_summary(mut self, reasoning_summary: Vec<String>) -> Self {
        self.reasoning_summary = reasoning_summary;
        self
    }

    pub fn with_approval(
        mut self,
        tool_name: String,
        summary: String,
        risk: String,
        tool_call_id: String,
    ) -> Self {
        self.approval = Some(ApprovalView {
            tool_name,
            summary,
            risk,
            tool_call_id,
            diff_preview: None,
        });
        self
    }

    pub fn with_diff_preview(mut self, preview: Option<String>) -> Self {
        if let Some(ref mut approval) = self.approval {
            approval.diff_preview = preview;
        }
        self
    }

    pub fn with_interrupt(
        mut self,
        interrupted_what: String,
        saved_status: String,
        next_actions: Vec<String>,
    ) -> Self {
        self.interrupt = Some(InterruptView {
            interrupted_what,
            saved_status,
            next_actions,
        });
        self
    }

    pub fn with_tool_logs(mut self, tool_logs: Vec<ToolLogView>) -> Self {
        self.tool_logs = tool_logs;
        self
    }

    pub fn with_elapsed_ms(mut self, elapsed_ms: u128) -> Self {
        self.elapsed_ms = Some(elapsed_ms);
        self
    }

    pub fn with_context_usage(mut self, estimated_tokens: usize, max_tokens: u32) -> Self {
        self.context_usage = Some(ContextUsageView {
            estimated_tokens,
            max_tokens,
        });
        self
    }

    pub fn with_completion_summary(
        mut self,
        completion_summary: impl Into<String>,
        saved_status: impl Into<String>,
    ) -> Self {
        self.completion_summary = Some(completion_summary.into());
        self.saved_status = Some(saved_status.into());
        self
    }

    pub fn with_error_summary(
        mut self,
        error_summary: impl Into<String>,
        recommended_actions: Vec<String>,
    ) -> Self {
        self.error_summary = Some(error_summary.into());
        self.recommended_actions = recommended_actions;
        self
    }

    pub fn with_context_warning(mut self, level: ContextWarningLevel) -> Self {
        self.context_warning = Some(level);
        self
    }

    pub fn with_inference_performance(mut self, perf: InferencePerformanceView) -> Self {
        self.inference_performance = Some(perf);
        self
    }
}

#[cfg(test)]
impl AppStateSnapshot {
    /// Assert that the snapshot has the expected fields populated for its state.
    pub fn assert_valid_for_state(&self) {
        match self.state {
            RuntimeState::AwaitingApproval => {
                assert!(
                    self.approval.is_some(),
                    "AwaitingApproval must have approval"
                );
            }
            RuntimeState::Done => {
                assert!(
                    self.completion_summary.is_some(),
                    "Done must have completion_summary"
                );
            }
            RuntimeState::Error => {
                assert!(
                    self.error_summary.is_some(),
                    "Error must have error_summary"
                );
            }
            RuntimeState::Interrupted => {
                assert!(self.interrupt.is_some(), "Interrupted must have interrupt");
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usage_ratio_normal() {
        let usage = ContextUsageView {
            estimated_tokens: 5000,
            max_tokens: 10000,
        };
        assert!((usage.usage_ratio() - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn usage_ratio_zero_max_tokens() {
        let usage = ContextUsageView {
            estimated_tokens: 100,
            max_tokens: 0,
        };
        assert!((usage.usage_ratio() - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn usage_ratio_clamped_above_one() {
        let usage = ContextUsageView {
            estimated_tokens: 15000,
            max_tokens: 10000,
        };
        assert!((usage.usage_ratio() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn usage_ratio_boundary_values() {
        let usage_80 = ContextUsageView {
            estimated_tokens: 8000,
            max_tokens: 10000,
        };
        assert!((usage_80.usage_ratio() - 0.8).abs() < f64::EPSILON);

        let usage_90 = ContextUsageView {
            estimated_tokens: 9000,
            max_tokens: 10000,
        };
        assert!((usage_90.usage_ratio() - 0.9).abs() < f64::EPSILON);
    }

    #[test]
    fn warning_level_below_80_is_none() {
        let usage = ContextUsageView {
            estimated_tokens: 7999,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), None);
    }

    #[test]
    fn warning_level_at_80_is_warning() {
        let usage = ContextUsageView {
            estimated_tokens: 8000,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Warning));
    }

    #[test]
    fn warning_level_at_89_is_warning() {
        let usage = ContextUsageView {
            estimated_tokens: 8999,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Warning));
    }

    #[test]
    fn warning_level_at_90_is_critical() {
        let usage = ContextUsageView {
            estimated_tokens: 9000,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Critical));
    }

    #[test]
    fn warning_level_at_100_is_critical() {
        let usage = ContextUsageView {
            estimated_tokens: 10000,
            max_tokens: 10000,
        };
        assert_eq!(usage.warning_level(), Some(ContextWarningLevel::Critical));
    }

    #[test]
    fn usage_percent_normal() {
        let usage = ContextUsageView {
            estimated_tokens: 2200,
            max_tokens: 10000,
        };
        assert_eq!(usage.usage_percent(), 22);
    }

    #[test]
    fn usage_percent_rounding() {
        let usage = ContextUsageView {
            estimated_tokens: 3333,
            max_tokens: 10000,
        };
        assert_eq!(usage.usage_percent(), 33);
    }

    #[test]
    fn usage_percent_zero_max() {
        let usage = ContextUsageView {
            estimated_tokens: 100,
            max_tokens: 0,
        };
        assert_eq!(usage.usage_percent(), 0);
    }

    #[test]
    fn inference_performance_default_is_all_none() {
        let perf = InferencePerformanceView::default();
        assert_eq!(perf.tokens_per_sec_tenths, None);
        assert_eq!(perf.eval_tokens, None);
        assert_eq!(perf.eval_duration_ms, None);
        assert_eq!(perf.formatted_tokens_per_sec(), None);
    }

    #[test]
    fn inference_performance_formatted_tokens_per_sec() {
        let perf = InferencePerformanceView {
            tokens_per_sec_tenths: Some(325),
            eval_tokens: Some(100),
            eval_duration_ms: Some(3077),
            ..Default::default()
        };
        assert_eq!(
            perf.formatted_tokens_per_sec(),
            Some("32.5tok/s".to_string())
        );
    }

    #[test]
    fn inference_performance_formatted_tokens_per_sec_zero_fraction() {
        let perf = InferencePerformanceView {
            tokens_per_sec_tenths: Some(100),
            ..Default::default()
        };
        assert_eq!(
            perf.formatted_tokens_per_sec(),
            Some("10.0tok/s".to_string())
        );
    }

    #[test]
    fn inference_performance_formatted_tokens_per_sec_none_when_no_tenths() {
        let perf = InferencePerformanceView {
            eval_tokens: Some(50),
            eval_duration_ms: Some(1000),
            ..Default::default()
        };
        assert_eq!(perf.formatted_tokens_per_sec(), None);
    }

    #[test]
    fn inference_performance_serialize_deserialize() {
        let perf = InferencePerformanceView {
            tokens_per_sec_tenths: Some(325),
            eval_tokens: Some(100),
            eval_duration_ms: Some(3077),
            ..Default::default()
        };
        let json = serde_json::to_string(&perf).expect("serialize");
        let back: InferencePerformanceView = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(perf, back);
    }

    #[test]
    fn app_state_snapshot_backward_compat_without_inference_performance() {
        // Old JSON without inference_performance field should still deserialize
        let json = r#"{
            "state": "Done",
            "status": {"line": "Done."},
            "reasoning_summary": [],
            "tool_logs": [],
            "recommended_actions": []
        }"#;
        let snapshot: AppStateSnapshot = serde_json::from_str(json).expect("deserialize");
        assert!(snapshot.inference_performance.is_none());
    }

    #[test]
    fn tool_log_view_elapsed_ms_default_none() {
        let json = r#"{"tool_name":"Read","action":"open","target":"src/main.rs"}"#;
        let view: ToolLogView = serde_json::from_str(json).expect("deserialize");
        assert_eq!(view.elapsed_ms, None);
    }

    #[test]
    fn tool_log_view_elapsed_ms_round_trip() {
        let view = ToolLogView {
            tool_name: "Read".to_string(),
            action: "open".to_string(),
            target: "src/main.rs".to_string(),
            elapsed_ms: Some(1234),
        };
        let json = serde_json::to_string(&view).expect("serialize");
        let back: ToolLogView = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.elapsed_ms, Some(1234));
    }

    #[test]
    fn app_state_snapshot_backward_compat_without_elapsed_ms() {
        // Old JSON with tool_logs that lack elapsed_ms should still deserialize
        let json = r#"{
            "state": "Working",
            "status": {"line": "Working..."},
            "reasoning_summary": [],
            "tool_logs": [{"tool_name":"Read","action":"open","target":"src/main.rs"}],
            "recommended_actions": []
        }"#;
        let snapshot: AppStateSnapshot = serde_json::from_str(json).expect("deserialize");
        assert_eq!(snapshot.tool_logs.len(), 1);
        assert_eq!(snapshot.tool_logs[0].elapsed_ms, None);
    }

    #[test]
    fn inference_perf_without_prompt_tokens() {
        // Old JSON without prompt_tokens field should still deserialize
        let json = r#"{"tokens_per_sec_tenths":325,"eval_tokens":100,"eval_duration_ms":3077}"#;
        let perf: InferencePerformanceView = serde_json::from_str(json).expect("deserialize");
        assert_eq!(perf.tokens_per_sec_tenths, Some(325));
        assert_eq!(perf.prompt_tokens, None);
    }

    #[test]
    fn inference_perf_with_prompt_tokens() {
        let perf = InferencePerformanceView {
            tokens_per_sec_tenths: Some(325),
            eval_tokens: Some(100),
            eval_duration_ms: Some(3077),
            prompt_tokens: Some(500),
        };
        let json = serde_json::to_string(&perf).expect("serialize");
        let back: InferencePerformanceView = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(perf, back);
        assert_eq!(back.prompt_tokens, Some(500));
    }

    #[test]
    fn app_state_snapshot_with_inference_performance_builder() {
        let perf = InferencePerformanceView {
            tokens_per_sec_tenths: Some(200),
            eval_tokens: Some(80),
            eval_duration_ms: Some(4000),
            ..Default::default()
        };
        let snapshot =
            AppStateSnapshot::new(RuntimeState::Done).with_inference_performance(perf.clone());
        assert_eq!(snapshot.inference_performance, Some(perf));
    }

    // ============================================================
    // TerminationReason tests (Issue #129, Task 1.1)
    // ============================================================

    #[test]
    fn termination_reason_default_is_completed() {
        assert_eq!(TerminationReason::default(), TerminationReason::Completed);
    }

    #[test]
    fn termination_reason_display() {
        assert_eq!(TerminationReason::Completed.to_string(), "completed");
        assert_eq!(TerminationReason::Timeout.to_string(), "timeout");
        assert_eq!(
            TerminationReason::MaxIterations.to_string(),
            "max_iterations"
        );
        assert_eq!(TerminationReason::LoopDetected.to_string(), "loop_detected");
        assert_eq!(
            TerminationReason::MaxToolCalls.to_string(),
            "max_tool_calls"
        );
    }

    #[test]
    fn termination_reason_serde_roundtrip() {
        for reason in [
            TerminationReason::Completed,
            TerminationReason::Timeout,
            TerminationReason::MaxIterations,
            TerminationReason::LoopDetected,
            TerminationReason::MaxToolCalls,
        ] {
            let json = serde_json::to_string(&reason).expect("serialize");
            let back: TerminationReason = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(reason, back);
        }
    }

    #[test]
    fn termination_reason_serde_snake_case() {
        let json = serde_json::to_string(&TerminationReason::MaxIterations).expect("serialize");
        assert_eq!(json, r#""max_iterations""#);
        let back: TerminationReason =
            serde_json::from_str(r#""max_iterations""#).expect("deserialize");
        assert_eq!(back, TerminationReason::MaxIterations);
    }

    // ============================================================
    // Finding tests (Issue #129, Task 1.2)
    // ============================================================

    #[test]
    fn finding_serde_roundtrip() {
        let finding = Finding {
            title: "Found pattern".to_string(),
            detail: "The module uses X pattern".to_string(),
            related_code: vec!["src/main.rs:10".to_string(), "src/lib.rs:20".to_string()],
        };
        let json = serde_json::to_string(&finding).expect("serialize");
        let back: Finding = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(finding, back);
    }

    #[test]
    fn finding_empty_related_code() {
        let finding = Finding {
            title: "Simple finding".to_string(),
            detail: "No code refs".to_string(),
            related_code: vec![],
        };
        let json = serde_json::to_string(&finding).expect("serialize");
        let back: Finding = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(finding, back);
    }

    // ============================================================
    // SubAgentPayload tests (Issue #129, Task 1.3)
    // ============================================================

    #[test]
    fn subagent_payload_full_roundtrip() {
        let payload = SubAgentPayload {
            found_files: vec!["src/main.rs".to_string()],
            key_findings: vec![Finding {
                title: "Entry point".to_string(),
                detail: "Main function found".to_string(),
                related_code: vec!["src/main.rs:1".to_string()],
            }],
            raw_summary: "Found the entry point".to_string(),
            confidence: Some(0.9),
            termination_reason: TerminationReason::Completed,
            error: None,
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let back: SubAgentPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.found_files, payload.found_files);
        assert_eq!(back.key_findings, payload.key_findings);
        assert_eq!(back.raw_summary, payload.raw_summary);
        assert_eq!(back.confidence, payload.confidence);
        assert_eq!(back.termination_reason, payload.termination_reason);
        assert_eq!(back.error, payload.error);
    }

    #[test]
    fn subagent_payload_defaults_for_optional_fields() {
        // JSON without optional fields should deserialize with defaults
        let json = r#"{"found_files":[],"key_findings":[],"raw_summary":"hello"}"#;
        let payload: SubAgentPayload = serde_json::from_str(json).expect("deserialize");
        assert_eq!(payload.confidence, None);
        assert_eq!(payload.termination_reason, TerminationReason::Completed);
        assert_eq!(payload.error, None);
    }

    #[test]
    fn subagent_payload_fallback_constructor() {
        let payload =
            SubAgentPayload::fallback("partial result".to_string(), TerminationReason::Timeout);
        assert!(payload.found_files.is_empty());
        assert!(payload.key_findings.is_empty());
        assert_eq!(payload.raw_summary, "partial result");
        assert_eq!(payload.confidence, None);
        assert_eq!(payload.termination_reason, TerminationReason::Timeout);
        assert_eq!(payload.error, None);
    }

    #[test]
    fn subagent_payload_with_error() {
        let payload = SubAgentPayload {
            found_files: vec![],
            key_findings: vec![],
            raw_summary: String::new(),
            confidence: None,
            termination_reason: TerminationReason::Timeout,
            error: Some("timed out during exploration".to_string()),
        };
        let json = serde_json::to_string(&payload).expect("serialize");
        let back: SubAgentPayload = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.error, Some("timed out during exploration".to_string()));
        assert_eq!(back.termination_reason, TerminationReason::Timeout);
    }
}
