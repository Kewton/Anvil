use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ExecutionMode {
    Plan,
    Act,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum TaskProfile {
    #[default]
    Generic,
    Coding,
    Content,
    Ui,
    Research,
}

impl TaskProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskProfile::Generic => "generic",
            TaskProfile::Coding => "coding",
            TaskProfile::Content => "content",
            TaskProfile::Ui => "ui",
            TaskProfile::Research => "research",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum WorkMode {
    #[default]
    Auto,
    TypeScriptUi,
    Python,
    Docs,
    AnswerOnly,
    GenericCode,
    Unknown,
}

impl WorkMode {
    pub fn as_str(self) -> &'static str {
        match self {
            WorkMode::Auto => "auto",
            WorkMode::TypeScriptUi => "typescript-ui",
            WorkMode::Python => "python",
            WorkMode::Docs => "docs",
            WorkMode::AnswerOnly => "answer-only",
            WorkMode::GenericCode => "generic-code",
            WorkMode::Unknown => "unknown",
        }
    }

    pub fn policy(self) -> ModePolicy {
        match self {
            WorkMode::TypeScriptUi => ModePolicy {
                repo_edit_required: true,
                allow_ui_deterministic_fallback: true,
                allow_polish_fallback: true,
                allow_python_deterministic_fallback: false,
                allow_docs_deterministic_fallback: false,
                quality_gate_enabled: true,
                include_working_memory: true,
                allow_repo_context: true,
            },
            WorkMode::Python => ModePolicy {
                repo_edit_required: true,
                allow_ui_deterministic_fallback: false,
                allow_polish_fallback: false,
                allow_python_deterministic_fallback: true,
                allow_docs_deterministic_fallback: false,
                quality_gate_enabled: false,
                include_working_memory: true,
                allow_repo_context: true,
            },
            WorkMode::Docs => ModePolicy {
                repo_edit_required: true,
                allow_ui_deterministic_fallback: false,
                allow_polish_fallback: false,
                allow_python_deterministic_fallback: false,
                allow_docs_deterministic_fallback: true,
                quality_gate_enabled: false,
                include_working_memory: true,
                allow_repo_context: true,
            },
            WorkMode::AnswerOnly => ModePolicy {
                repo_edit_required: false,
                allow_ui_deterministic_fallback: false,
                allow_polish_fallback: false,
                allow_python_deterministic_fallback: false,
                allow_docs_deterministic_fallback: false,
                quality_gate_enabled: false,
                include_working_memory: false,
                allow_repo_context: true,
            },
            WorkMode::GenericCode | WorkMode::Unknown => ModePolicy {
                repo_edit_required: true,
                allow_ui_deterministic_fallback: false,
                allow_polish_fallback: false,
                allow_python_deterministic_fallback: false,
                allow_docs_deterministic_fallback: false,
                quality_gate_enabled: false,
                include_working_memory: true,
                allow_repo_context: true,
            },
            WorkMode::Auto => ModePolicy {
                repo_edit_required: true,
                allow_ui_deterministic_fallback: true,
                allow_polish_fallback: true,
                allow_python_deterministic_fallback: false,
                allow_docs_deterministic_fallback: false,
                quality_gate_enabled: true,
                include_working_memory: true,
                allow_repo_context: true,
            },
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModePolicy {
    pub repo_edit_required: bool,
    pub allow_ui_deterministic_fallback: bool,
    pub allow_polish_fallback: bool,
    pub allow_python_deterministic_fallback: bool,
    pub allow_docs_deterministic_fallback: bool,
    pub quality_gate_enabled: bool,
    pub include_working_memory: bool,
    pub allow_repo_context: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ModeClassification {
    pub work_mode: WorkMode,
    pub intent: &'static str,
    pub allows_file_edits: bool,
    pub requires_tests: bool,
    pub confidence: f32,
    pub ambiguity: bool,
    pub alternative_gap: f32,
    pub reason: &'static str,
    #[serde(default)]
    pub evidence: Vec<&'static str>,
    #[serde(default)]
    pub alternatives: Vec<WorkModeCandidate>,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct WorkModeCandidate {
    pub work_mode: WorkMode,
    pub intent: &'static str,
    pub confidence: f32,
    pub evidence: Vec<&'static str>,
}

pub fn infer_work_mode_from_text(raw: &str) -> WorkMode {
    classify_work_mode_json(raw).work_mode
}

/// Issue #576: Confidence threshold below which WorkMode classification is
/// considered uncertain enough to warrant a second-pass LLM confirmation.
///
/// SSoT used by:
/// - `classify_work_mode_json` (for the `ambiguity` flag computation)
/// - `should_request_confirmation` (the second-pass gate)
pub(crate) const WORK_MODE_CONFIRM_CONFIDENCE_THRESHOLD: f32 = 0.90;

/// Issue #576: pure predicate that decides whether the WorkMode classification
/// is uncertain enough to call the LLM second-pass confirmation. The
/// `raw_input` argument is retained for future signature extension but is
/// currently unused. agent layer types must not be imported here (DR3-002).
pub fn should_request_confirmation(c: &ModeClassification, raw_input: &str) -> bool {
    let _ = raw_input; // reserved for future heuristics
    c.ambiguity || c.confidence < WORK_MODE_CONFIRM_CONFIDENCE_THRESHOLD
}

pub fn classify_work_mode_json(raw: &str) -> ModeClassification {
    let lower = raw.to_ascii_lowercase();
    let explicit_edit = contains_any(
        &lower,
        &[
            "create",
            "build",
            "develop",
            "implement",
            "scaffold",
            "write",
            "edit",
            "update",
            "modify",
            "fix",
            "refactor",
            "add",
            "生成",
            "作成",
            "開発",
            "実装",
            "編集",
            "更新",
            "変更して",
            "修正",
            "追加",
            "追記",
        ],
    );
    // CB2-001: Use explicit prohibitive phrases for "no changes" instead of the
    // bare substring "no changes", which would otherwise match descriptive
    // contexts like "the app says no changes detected" and incorrectly route
    // legitimate edit requests to `AnswerOnly`. The phrases below all encode an
    // imperative "do not make changes" intent.
    let explicit_no_edit = contains_any(
        &lower,
        &[
            "do not modify",
            "don't modify",
            "do not edit",
            "don't edit",
            "do not change",
            "don't change",
            "no edits",
            "no changes please",
            "make no changes",
            "without changes",
            "no file changes",
            "without changing files",
            "read only",
            "read-only",
            "変更しない",
            "編集しない",
            "ファイルは変更しない",
            "変更せず",
            "編集せず",
            "読み取り専用",
        ],
    );
    let answer_request = contains_any(
        &lower,
        &[
            "summarize",
            "explain",
            "tell me",
            "analyze",
            "investigate",
            "review",
            "what is",
            "how is",
            "要約",
            "説明",
            "教えて",
            "整理",
            "調査",
            "検討",
            "レビュー",
            "どうですか",
            "とは",
        ],
    );

    let typescript_ui = contains_any(
        &lower,
        &[
            "next.js",
            "nextjs",
            "react",
            "nuxt",
            "vue",
            "vite",
            "typescript",
            "tsx",
            "browser ui",
            "web ui",
            "web app",
            "frontend",
            "front-end",
            "ui",
            "ux",
            "画面",
            "アプリ",
            "ゲーム",
            "フロントエンド",
        ],
    );
    let explicit_ui_framework = contains_any(
        &lower,
        &["next.js", "nextjs", "react", "nuxt", "vue", "vite", "tsx"],
    );
    let python = contains_any(
        &lower,
        &[
            "python",
            ".py",
            "pytest",
            "pip",
            "venv",
            "csv",
            "pandas",
            "python3",
            "fastapi",
            "flask",
            "django",
            "pydantic",
            "sqlalchemy",
        ],
    );
    let explicit_python_artifact = contains_any(
        &lower,
        &[
            "python",
            ".py",
            "pytest",
            "pip",
            "venv",
            "pandas",
            "python3",
            "fastapi",
            "flask",
            "django",
            "pydantic",
            "sqlalchemy",
        ],
    );
    let docs = contains_any(
        &lower,
        &[
            "readme",
            "markdown",
            ".md",
            "docs/",
            "documentation",
            "docs",
            "doc",
            "document",
            "ドキュメント",
            "設計書",
            "仕様書",
            "手順書",
            "文章",
        ],
    );
    let explicit_docs_artifact = contains_any(
        &lower,
        &[
            "readme",
            ".md",
            "docs/",
            "仕様書",
            "設計書",
            "手順書",
            "documentation",
            "document",
        ],
    );
    let primary_code_task = request_has_primary_code_task(raw, &lower);

    let mut candidates = Vec::<WorkModeCandidate>::new();
    if explicit_no_edit || (answer_request && !explicit_edit) {
        candidates.push(WorkModeCandidate {
            work_mode: WorkMode::AnswerOnly,
            intent: "answer",
            confidence: if explicit_no_edit { 0.95 } else { 0.82 },
            evidence: if explicit_no_edit {
                vec!["explicit-no-edit"]
            } else {
                vec!["answer-request", "no-edit-intent"]
            },
        });
    }
    if docs {
        let mut confidence: f32 = 0.84;
        let mut evidence = vec!["docs-artifact"];
        if explicit_docs_artifact {
            confidence += 0.08;
            evidence.push("explicit-docs-target");
        }
        if explicit_edit {
            confidence += 0.03;
            evidence.push("edit-intent");
        }
        if primary_code_task {
            confidence = confidence.min(if explicit_python_artifact || explicit_ui_framework {
                0.55
            } else {
                0.62
            });
            evidence.push("secondary-docs-for-code-task");
        }
        candidates.push(WorkModeCandidate {
            work_mode: WorkMode::Docs,
            intent: "docs",
            confidence: confidence.min(0.96),
            evidence,
        });
    }
    if python {
        let mut confidence: f32 = if explicit_python_artifact { 0.88 } else { 0.68 };
        let mut evidence = vec!["python-or-data-signal"];
        if explicit_python_artifact {
            evidence.push("explicit-python-artifact");
        }
        if docs && !primary_code_task {
            confidence = 0.52;
            evidence.push("weakened-by-docs-target");
        }
        candidates.push(WorkModeCandidate {
            work_mode: WorkMode::Python,
            intent: "python-code",
            confidence,
            evidence,
        });
    }
    if typescript_ui {
        let mut confidence: f32 = if explicit_ui_framework { 0.88 } else { 0.78 };
        let mut evidence = vec!["ui-or-frontend-signal"];
        if explicit_ui_framework {
            evidence.push("explicit-ui-framework");
        }
        if docs && !primary_code_task {
            confidence = confidence.min(0.48);
            evidence.push("weakened-by-docs-target");
        }
        candidates.push(WorkModeCandidate {
            work_mode: WorkMode::TypeScriptUi,
            intent: "ui-code",
            confidence,
            evidence,
        });
    }
    if explicit_edit {
        candidates.push(WorkModeCandidate {
            work_mode: WorkMode::GenericCode,
            intent: "code",
            confidence: 0.7,
            evidence: vec!["edit-intent"],
        });
    }
    if candidates.is_empty() {
        candidates.push(WorkModeCandidate {
            work_mode: WorkMode::Unknown,
            intent: "unknown",
            confidence: 0.35,
            evidence: vec!["insufficient-mode-signals"],
        });
    }
    candidates.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| mode_priority(b.work_mode).cmp(&mode_priority(a.work_mode)))
    });

    let selected = candidates[0].clone();
    let second_confidence = candidates
        .get(1)
        .map(|candidate| candidate.confidence)
        .unwrap_or(0.0);
    let alternative_gap = (selected.confidence - second_confidence).max(0.0);
    let ambiguity =
        alternative_gap < 0.15 && selected.confidence < WORK_MODE_CONFIRM_CONFIDENCE_THRESHOLD;
    let requires_tests =
        selected.work_mode != WorkMode::AnswerOnly && request_requires_tests(&lower, raw);
    ModeClassification {
        work_mode: selected.work_mode,
        intent: selected.intent,
        allows_file_edits: selected.work_mode != WorkMode::AnswerOnly,
        requires_tests: selected.work_mode != WorkMode::Docs && requires_tests,
        confidence: selected.confidence,
        ambiguity,
        alternative_gap,
        reason: mode_reason(selected.work_mode),
        evidence: selected.evidence.clone(),
        alternatives: candidates,
    }
}

fn mode_priority(mode: WorkMode) -> u8 {
    match mode {
        WorkMode::AnswerOnly => 6,
        WorkMode::Docs => 5,
        WorkMode::Python => 4,
        WorkMode::TypeScriptUi => 3,
        WorkMode::GenericCode => 2,
        WorkMode::Unknown | WorkMode::Auto => 1,
    }
}

fn mode_reason(mode: WorkMode) -> &'static str {
    match mode {
        WorkMode::AnswerOnly => "request is read-only or asks for analysis without edit intent",
        WorkMode::Docs => "documentation signals outscore competing mode signals",
        WorkMode::Python => {
            "Python runtime, file, or data-processing signals outscore alternatives"
        }
        WorkMode::TypeScriptUi => {
            "UI, frontend, TypeScript, or browser app signals outscore alternatives"
        }
        WorkMode::GenericCode => {
            "request has edit intent without a specific language or artifact mode"
        }
        WorkMode::Unknown | WorkMode::Auto => "request lacks enough mode-specific signals",
    }
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn request_has_primary_code_task(raw: &str, lower: &str) -> bool {
    let production_action = contains_any(
        lower,
        &[
            "create",
            "build",
            "develop",
            "implement",
            "scaffold",
            "fix",
            "refactor",
        ],
    ) || contains_any(raw, &["作成", "開発", "実装", "修正", "構築"]);
    let code_subject = contains_any(
        lower,
        &[
            "crud",
            "endpoint",
            "server",
            "backend",
            "frontend",
            "web app",
            "browser app",
            "cli",
            "component",
            "service",
            "module",
            "library",
            "crate",
            "package",
            "tool",
            "program",
            "command",
        ],
    ) || contains_ascii_token(lower, "api")
        || contains_any(
            raw,
            &[
                "エンドポイント",
                "サーバ",
                "バックエンド",
                "フロントエンド",
                "アプリ",
                "機能",
                "ライブラリ",
                "クレート",
                "パッケージ",
                "ツール",
                "コマンド",
            ],
        )
        || mentions_stack_as_build_target(raw, lower);

    production_action && code_subject
}

fn contains_ascii_token(haystack: &str, needle: &str) -> bool {
    haystack.match_indices(needle).any(|(idx, _)| {
        let before = haystack[..idx]
            .chars()
            .next_back()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric());
        let after_idx = idx + needle.len();
        let after = haystack[after_idx..]
            .chars()
            .next()
            .is_none_or(|ch| !ch.is_ascii_alphanumeric());
        before && after
    })
}

fn mentions_stack_as_build_target(raw: &str, lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "with fastapi",
            "using fastapi",
            "fastapi app",
            "fastapi api",
            "with flask",
            "using flask",
            "flask app",
            "with django",
            "using django",
            "django app",
            "rust library",
            "rust crate",
            "rust package",
            "cargo project",
        ],
    ) || contains_any(
        raw,
        &["FastAPIで", "Flaskで", "Djangoで", "Pythonで", "Rustで"],
    ) || (raw.contains("Rust") && contains_any(raw, &["ライブラリ", "クレート", "パッケージ"]))
}

fn request_requires_tests(lower: &str, raw: &str) -> bool {
    lower.contains("test")
        || lower.contains("pytest")
        || lower.contains("unittest")
        || raw.contains("テスト")
        || raw.contains("検証")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, Default)]
pub enum PlanStage {
    #[default]
    Stage1,
    Stage2,
    Stage3,
    Ready,
}

impl PlanStage {
    pub fn as_str(self) -> &'static str {
        match self {
            PlanStage::Stage1 => "stage1",
            PlanStage::Stage2 => "stage2",
            PlanStage::Stage3 => "stage3",
            PlanStage::Ready => "ready",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            PlanStage::Stage1 => "Stage 1",
            PlanStage::Stage2 => "Stage 2",
            PlanStage::Stage3 => "Stage 3",
            PlanStage::Ready => "Ready",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModeState {
    pub mode: ExecutionMode,
    pub active_plan_path: Option<PathBuf>,
    #[serde(default)]
    pub task_profile: TaskProfile,
    #[serde(default)]
    pub work_mode: WorkMode,
    #[serde(default)]
    pub plan_stage: PlanStage,
}

impl Default for ModeState {
    fn default() -> Self {
        Self {
            mode: ExecutionMode::Act,
            active_plan_path: None,
            task_profile: TaskProfile::Generic,
            work_mode: WorkMode::Auto,
            plan_stage: PlanStage::Stage1,
        }
    }
}

impl ModeState {
    pub fn policy(&self) -> ModePolicy {
        self.work_mode.policy()
    }

    pub fn enter_plan(
        &mut self,
        plan_dir: PathBuf,
        task_profile: TaskProfile,
    ) -> Result<PathBuf, String> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|err| format!("failed to build timestamp: {err}"))?
            .as_secs();
        let path = plan_dir.join(format!("plan-{ts}.md"));
        self.mode = ExecutionMode::Plan;
        self.active_plan_path = Some(path.clone());
        self.task_profile = task_profile;
        self.work_mode = WorkMode::Auto;
        self.plan_stage = PlanStage::Stage1;
        Ok(path)
    }

    pub fn approve(&mut self) {
        self.mode = ExecutionMode::Act;
        self.plan_stage = PlanStage::Ready;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn infer_work_mode_detects_answer_only_without_repo_edits() {
        assert_eq!(
            infer_work_mode_from_text("READMEを要約してください。ファイルは変更しないでください。"),
            WorkMode::AnswerOnly
        );
        assert_eq!(
            infer_work_mode_from_text("現在の設計について教えて"),
            WorkMode::AnswerOnly
        );
    }

    #[test]
    fn infer_work_mode_detects_specific_edit_domains() {
        assert_eq!(
            infer_work_mode_from_text("Next.jsで家計簿UIアプリを作成してください"),
            WorkMode::TypeScriptUi
        );
        assert_eq!(
            infer_work_mode_from_text("PythonでCSVを集計するCLIを作成してください"),
            WorkMode::Python
        );
        assert_eq!(
            infer_work_mode_from_text("READMEを更新してください"),
            WorkMode::Docs
        );
        assert_eq!(
            infer_work_mode_from_text("FastAPIプロジェクトのREADMEを更新してください"),
            WorkMode::Docs
        );
        assert_eq!(
            infer_work_mode_from_text(
                "FastAPIでCRUD APIを開発してREADMEとテストコードも実装してください"
            ),
            WorkMode::Python
        );
        assert_eq!(
            infer_work_mode_from_text("Next.jsアプリを作成してREADMEも追加してください"),
            WorkMode::TypeScriptUi
        );
        assert_eq!(
            infer_work_mode_from_text(
                "Node.jsでToDo管理CLIを開発してください。README.mdとテストコードも作成してください。"
            ),
            WorkMode::GenericCode
        );
        assert_eq!(
            infer_work_mode_from_text(
                "Rustで標準入力を読むCLIを開発してください。README.mdとテストコードも実装してください。"
            ),
            WorkMode::GenericCode
        );
        let rust_library = classify_work_mode_json(
            "文字列スラッグ生成用のRustライブラリを開発してください。README.mdとcargo testで動くテストも実装してください。",
        );
        assert_eq!(rust_library.work_mode, WorkMode::GenericCode);
        assert!(rust_library.requires_tests);
        assert!(rust_library.evidence.contains(&"edit-intent"));
    }

    #[test]
    fn mode_classifier_has_json_safe_shape() {
        let classification = classify_work_mode_json("PythonでCSV集計CLIを作成しテストも追加");
        assert_eq!(classification.work_mode, WorkMode::Python);
        assert!(classification.allows_file_edits);
        assert!(classification.requires_tests);
        assert!(!classification.evidence.is_empty());
        assert!(!classification.alternatives.is_empty());
        assert!(!classification.ambiguity);
        assert!(classification.alternative_gap >= 0.0);
        let json = serde_json::to_string(&classification).expect("json");
        assert!(json.contains("\"work_mode\":\"Python\""));
        assert!(json.contains("\"intent\":\"python-code\""));
        assert!(json.contains("\"ambiguity\""));
        assert!(json.contains("\"alternative_gap\""));
        assert!(json.contains("\"alternatives\""));
    }

    #[test]
    fn mode_classifier_uses_evidence_when_docs_and_ui_conflict() {
        let classification = classify_work_mode_json("READMEにUI設計セクションを追加してください");
        assert_eq!(classification.work_mode, WorkMode::Docs);
        assert!(
            classification
                .alternatives
                .iter()
                .any(|candidate| candidate.work_mode == WorkMode::TypeScriptUi)
        );
    }

    #[test]
    fn mode_classifier_handles_counterexamples_without_keyword_lock_in() {
        assert_eq!(
            infer_work_mode_from_text("Pythonコードの設計を説明して。変更しない"),
            WorkMode::AnswerOnly
        );
        assert_eq!(
            infer_work_mode_from_text("CSV仕様書をdocsに追記してください"),
            WorkMode::Docs
        );
        assert_eq!(
            infer_work_mode_from_text("レビューして問題があれば修正してください"),
            WorkMode::GenericCode
        );
        assert_eq!(
            infer_work_mode_from_text("docs/ui-guidelines.mdを更新してください"),
            WorkMode::Docs
        );
    }

    #[test]
    fn answer_only_policy_disables_repo_edit_recovery() {
        let state = ModeState {
            work_mode: WorkMode::AnswerOnly,
            ..ModeState::default()
        };
        let policy = state.policy();
        assert!(!policy.repo_edit_required);
        assert!(!policy.allow_ui_deterministic_fallback);
        assert!(!policy.include_working_memory);
    }

    fn make_classification(confidence: f32, ambiguity: bool) -> ModeClassification {
        ModeClassification {
            work_mode: WorkMode::GenericCode,
            intent: "code",
            allows_file_edits: true,
            requires_tests: false,
            confidence,
            ambiguity,
            alternative_gap: 0.20,
            reason: "test",
            evidence: vec![],
            alternatives: vec![],
        }
    }

    #[test]
    fn should_request_confirmation_skips_high_confidence_classifications() {
        let c = make_classification(0.95, false);
        assert!(!should_request_confirmation(&c, "irrelevant"));
    }

    #[test]
    fn should_request_confirmation_triggers_low_confidence_classifications() {
        let c = make_classification(0.87, false);
        assert!(should_request_confirmation(&c, "irrelevant"));
    }

    #[test]
    fn should_request_confirmation_triggers_when_ambiguity_is_set() {
        let c = make_classification(0.95, true);
        assert!(should_request_confirmation(&c, "irrelevant"));
    }

    #[test]
    fn should_request_confirmation_boundary_at_threshold_returns_false() {
        // confidence == 0.90 → not below threshold → skip (strict `<`).
        let c = make_classification(WORK_MODE_CONFIRM_CONFIDENCE_THRESHOLD, false);
        assert!(!should_request_confirmation(&c, "irrelevant"));
    }

    /// CB-002: ensure the `explicit_no_edit` needle set covers common English
    /// negated-edit phrases ("do not edit" / "don't edit" / "no edits" /
    /// "no changes please" / "do not change" / "don't change"). For each
    /// phrase the classifier must select `AnswerOnly` regardless of any
    /// edit-flavoured keywords in the input (e.g. "fix", "update").
    #[test]
    fn classifier_detects_english_negated_edit_phrases() {
        for phrase in [
            "Please review the code but do not edit anything.",
            "Read the file, don't edit it.",
            "Summarize the architecture; no edits.",
            "Show me the issue but no changes please.",
            "Explain the bug, do not change the file.",
            "Analyze this code, don't change anything.",
        ] {
            let classification = classify_work_mode_json(phrase);
            assert_eq!(
                classification.work_mode,
                WorkMode::AnswerOnly,
                "phrase did not yield AnswerOnly: {phrase}",
            );
            assert!(
                classification.evidence.contains(&"explicit-no-edit"),
                "phrase missing explicit-no-edit evidence: {phrase}",
            );
        }
    }

    /// CB2-001 regression: edit requests that mention "no changes" inside a
    /// descriptive context (e.g. error/status messages) must NOT acquire the
    /// `explicit-no-edit` evidence, and therefore must NOT be classified as
    /// `AnswerOnly`. The classifier must reserve `explicit-no-edit` for
    /// imperative prohibitions ("no changes please", "make no changes",
    /// "without changes", ...).
    #[test]
    fn classifier_does_not_treat_descriptive_no_changes_as_explicit_no_edit() {
        for phrase in [
            "Fix the bug where the app says no changes detected",
            "When I run git status it shows no changes found, please fix the diff logic",
            "The CI reports no changes but my edits are committed — please debug",
        ] {
            let classification = classify_work_mode_json(phrase);
            assert_ne!(
                classification.work_mode,
                WorkMode::AnswerOnly,
                "descriptive 'no changes' phrase was incorrectly routed to AnswerOnly: {phrase}",
            );
            assert!(
                !classification.evidence.contains(&"explicit-no-edit"),
                "descriptive 'no changes' phrase incorrectly tagged explicit-no-edit: {phrase}",
            );
        }
    }

    /// CB2-001: the new explicit prohibitive phrases must still route to
    /// `AnswerOnly` with `explicit-no-edit` evidence. Locks the replacement
    /// needles in.
    #[test]
    fn classifier_detects_new_explicit_no_changes_phrases() {
        for phrase in [
            "Summarize the design; no changes please.",
            "Please review and make no changes to the source.",
            "Explain the architecture without changes to any file.",
        ] {
            let classification = classify_work_mode_json(phrase);
            assert_eq!(
                classification.work_mode,
                WorkMode::AnswerOnly,
                "phrase did not yield AnswerOnly: {phrase}",
            );
            assert!(
                classification.evidence.contains(&"explicit-no-edit"),
                "phrase missing explicit-no-edit evidence: {phrase}",
            );
        }
    }
}
