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
    let explicit_no_edit = contains_any(
        &lower,
        &[
            "do not modify",
            "don't modify",
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
            "python", ".py", "pytest", "pip", "venv", "csv", "pandas", "python3",
        ],
    );
    let explicit_python_artifact = contains_any(
        &lower,
        &[
            "python", ".py", "pytest", "pip", "venv", "pandas", "python3",
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
        if docs && !explicit_python_artifact {
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
        if docs {
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
    let requires_tests =
        selected.work_mode != WorkMode::AnswerOnly && request_requires_tests(&lower, raw);
    ModeClassification {
        work_mode: selected.work_mode,
        intent: selected.intent,
        allows_file_edits: selected.work_mode != WorkMode::AnswerOnly,
        requires_tests: selected.work_mode != WorkMode::Docs && requires_tests,
        confidence: selected.confidence,
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
    }

    #[test]
    fn mode_classifier_has_json_safe_shape() {
        let classification = classify_work_mode_json("PythonでCSV集計CLIを作成しテストも追加");
        assert_eq!(classification.work_mode, WorkMode::Python);
        assert!(classification.allows_file_edits);
        assert!(classification.requires_tests);
        assert!(!classification.evidence.is_empty());
        assert!(!classification.alternatives.is_empty());
        let json = serde_json::to_string(&classification).expect("json");
        assert!(json.contains("\"work_mode\":\"Python\""));
        assert!(json.contains("\"intent\":\"python-code\""));
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
}
