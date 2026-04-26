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
    if explicit_no_edit || (answer_request && !explicit_edit) {
        return ModeClassification {
            work_mode: WorkMode::AnswerOnly,
            intent: "answer",
            allows_file_edits: false,
            requires_tests: false,
            confidence: if explicit_no_edit { 0.95 } else { 0.82 },
            reason: "request is read-only or asks for analysis without edit intent",
        };
    }

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
    if typescript_ui {
        return ModeClassification {
            work_mode: WorkMode::TypeScriptUi,
            intent: "ui-code",
            allows_file_edits: true,
            requires_tests: request_requires_tests(&lower, raw),
            confidence: 0.86,
            reason: "request mentions UI, frontend, TypeScript, or browser app terms",
        };
    }

    if contains_any(
        &lower,
        &[
            "python", ".py", "pytest", "pip", "venv", "csv", "pandas", "python3",
        ],
    ) {
        return ModeClassification {
            work_mode: WorkMode::Python,
            intent: "python-code",
            allows_file_edits: true,
            requires_tests: request_requires_tests(&lower, raw),
            confidence: 0.88,
            reason: "request mentions Python runtime, files, or Python data terms",
        };
    }

    if contains_any(
        &lower,
        &[
            "readme",
            "markdown",
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
    ) {
        return ModeClassification {
            work_mode: WorkMode::Docs,
            intent: "docs",
            allows_file_edits: true,
            requires_tests: false,
            confidence: 0.84,
            reason: "request mentions documentation artifacts",
        };
    }

    if explicit_edit {
        ModeClassification {
            work_mode: WorkMode::GenericCode,
            intent: "code",
            allows_file_edits: true,
            requires_tests: request_requires_tests(&lower, raw),
            confidence: 0.7,
            reason: "request has edit intent without a specific language or artifact mode",
        }
    } else {
        ModeClassification {
            work_mode: WorkMode::Unknown,
            intent: "unknown",
            allows_file_edits: true,
            requires_tests: request_requires_tests(&lower, raw),
            confidence: 0.35,
            reason: "request lacks enough mode-specific signals",
        }
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
        let json = serde_json::to_string(&classification).expect("json");
        assert!(json.contains("\"work_mode\":\"Python\""));
        assert!(json.contains("\"intent\":\"python-code\""));
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
