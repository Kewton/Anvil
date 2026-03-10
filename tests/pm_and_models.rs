use anvil::agents::AgentResult;
use anvil::agents::executor::StepExecutor;
use anvil::agents::pm::{AgentRole, PmAgent};
use anvil::agents::planning::{PlanStep, TaskAnalyzer, TaskKind, TurnPlanner};
use anvil::config::repo_instructions::RepoInstructions;
use anvil::models::client::{ModelClient, ModelRequest, ModelResponse};
use anvil::models::routing::ModelRouter;
use anvil::roles::{EffectiveModels, RoleRegistry};
use anvil::runtime::engine::RuntimeEngine;
use anvil::runtime::loop_state::RuntimeLoop;
use anvil::runtime::sandbox::SandboxPolicy;
use anvil::runtime::{NetworkPolicy, PermissionMode};
use anvil::state::session::{AgentModels, SessionState};
use anvil::tools::registry::ToolRegistry;
use clap::Parser;
use std::env;
use std::fs;
use tempfile::tempdir;

struct TestClient {
    prefix: &'static str,
    provider: &'static str,
}

impl ModelClient for TestClient {
    fn provider_name(&self) -> &'static str {
        self.provider
    }

    fn can_handle(&self, model: &str) -> bool {
        model.starts_with(self.prefix)
    }

    fn complete(&self, request: &ModelRequest) -> anyhow::Result<ModelResponse> {
        Ok(ModelResponse {
            provider: self.provider.to_string(),
            model: request.model.clone(),
            output: format!("{} handled {}", self.provider, request.user_prompt),
        })
    }
}

#[test]
fn model_router_routes_to_matching_provider() {
    let router = ModelRouter::new(vec![
        Box::new(TestClient {
            prefix: "lmstudio/",
            provider: "lm-studio-test",
        }),
        Box::new(TestClient {
            prefix: "",
            provider: "ollama-test",
        }),
    ]);

    let response = router
        .complete(&ModelRequest {
            model: "lmstudio/qwen".to_string(),
            system_prompt: "system".to_string(),
            user_prompt: "user".to_string(),
        })
        .expect("router response");

    assert_eq!(response.provider, "lm-studio-test");
}

#[test]
fn pm_agent_uses_fast_path_for_small_clarifications() {
    let registry = RoleRegistry::load_builtin().expect("registry");
    let cli = anvil::cli::Cli::parse_from(["anvil", "--model", "qwen3.5:35b"]);
    let models = EffectiveModels::from_cli(&cli, &registry).expect("models");
    let pm = PmAgent::new(ModelRouter::new(vec![Box::new(TestClient {
        prefix: "",
        provider: "pm-test",
    })]));
    let runtime = runtime(PermissionMode::ReadOnly);

    let outcome = pm
        .run_turn(
            &models,
            "what is the current objective?",
            "[source=user]\nwhat is the current objective?",
            &runtime,
        )
        .expect("pm turn");

    assert!(outcome.delegated_roles.is_empty());
    assert!(outcome
        .results[0]
        .summary
        .contains("PM handled the request directly"));
}

#[test]
fn pm_agent_delegates_editor_work() {
    let registry = RoleRegistry::load_builtin().expect("registry");
    let cli = anvil::cli::Cli::parse_from(["anvil", "--model", "pm-model"]);
    let models = EffectiveModels::from_cli(&cli, &registry).expect("models");
    let pm = PmAgent::default();
    let runtime = runtime(PermissionMode::WorkspaceWrite);

    let outcome = pm
        .run_turn(
            &models,
            "implement the parser fix",
            "[source=user]\nimplement the parser fix",
            &runtime,
        )
        .expect("pm turn");

    assert_eq!(
        outcome.delegated_roles,
        vec![AgentRole::Reader, AgentRole::Editor, AgentRole::Tester]
    );
    assert!(outcome.user_response.contains("その上で"));
    assert!(outcome
        .results
        .iter()
        .any(|result| result.summary.contains("Editor prepared a bounded edit plan")));
}

#[test]
fn editor_can_apply_bounded_mutation_when_explicitly_requested() {
    let registry = RoleRegistry::load_builtin().expect("registry");
    let cli = anvil::cli::Cli::parse_from([
        "anvil",
        "--model",
        "pm-model",
        "--permission-mode",
        "workspace-write",
    ]);
    let models = EffectiveModels::from_cli(&cli, &registry).expect("models");
    let pm = PmAgent::default();
    let temp = tempdir().expect("tempdir");
    let file = temp.path().join("sample.rs");
    fs::write(&file, "fn main() {}\n").expect("write fixture");
    let runtime = runtime_at(temp.path().to_path_buf(), PermissionMode::WorkspaceWrite);

    let outcome = pm
        .run_turn(
            &models,
            "apply update file sample",
            "[source=user]\napply update file sample",
            &runtime,
        )
        .expect("pm turn");

    assert_eq!(outcome.delegated_roles, vec![AgentRole::Reader, AgentRole::Editor]);
    assert!(outcome
        .results
        .iter()
        .any(|result| result.summary.contains("applied a bounded mutation")));
    let updated = fs::read_to_string(&file).expect("read mutated file");
    assert!(updated.contains("anvil-mvp: apply update file sample"));
}

#[test]
fn runtime_loop_records_delegations_and_results() {
    let registry = RoleRegistry::load_builtin().expect("registry");
    let cli = anvil::cli::Cli::parse_from([
        "anvil",
        "--model",
        "pm-model",
        "--reviewer-model",
        "review-model",
    ]);
    let models = EffectiveModels::from_cli(&cli, &registry).expect("models");
    let pm = PmAgent::default();
    let mut session = sample_session();
    let runtime = runtime(PermissionMode::WorkspaceWrite);

    let summary = RuntimeLoop::run_prompt(
        &mut session,
        &models,
        &pm,
        &runtime,
        "[source=user]\nreview the current diff",
        "review the current diff",
    )
    .expect("runtime loop");

    assert!(summary.contains("差分は"));
    assert_eq!(session.recent_delegations.len(), 1);
    assert_eq!(session.recent_delegations[0].role, "reviewer");
    assert_eq!(session.recent_delegations[0].resolved_model, "review-model");
    assert_eq!(session.recent_results.len(), 1);
    assert!(session.recent_results[0]
        .facts
        .iter()
        .any(|fact| fact.key == "diff.changed_files"));
    assert!(session.active_plan_summary.contains("Review the highest-risk files"));
    assert!(session.latest_evidence_summary.contains("diff.changed_files="));
    assert_eq!(session.completed_steps, vec!["review the current diff"]);
    assert!(session.pending_steps[0].contains("Review the highest-risk files"));
}

#[test]
fn pm_agent_subagents_use_runtime_tools() {
    let registry = RoleRegistry::load_builtin().expect("registry");
    let cli = anvil::cli::Cli::parse_from([
        "anvil",
        "--model",
        "pm-model",
        "--permission-mode",
        "workspace-write",
    ]);
    let models = EffectiveModels::from_cli(&cli, &registry).expect("models");
    let pm = PmAgent::default();
    let runtime = runtime(PermissionMode::WorkspaceWrite);

    let reader = pm
        .run_turn(
            &models,
            "inspect the repository layout",
            "[source=user]\ninspect the repository layout",
            &runtime,
        )
        .expect("reader turn");
    assert_eq!(reader.delegated_roles, vec![AgentRole::Reader]);
    assert!(reader.results[0].summary.contains("Reader inspected"));

    let git_reader = pm
        .run_turn(
            &models,
            "このブランチを解説して",
            "[source=user]\nこのブランチを解説して",
            &runtime,
        )
        .expect("git reader turn");
    assert_eq!(
        git_reader.delegated_roles,
        vec![AgentRole::Reader, AgentRole::Reviewer]
    );
    assert!(git_reader.user_response.contains("現在のブランチは"));
    assert_eq!(git_reader.results.len(), 2);

    let tester = pm
        .run_turn(
            &models,
            "test the current setup",
            "[source=user]\ntest the current setup",
            &runtime,
        )
        .expect("tester turn");
    assert_eq!(tester.delegated_roles, vec![AgentRole::Tester]);
    assert!(tester.results[0].summary.contains("cargo check"));
    assert_eq!(tester.results[0].commands_run, vec!["cargo check"]);
    assert!(!tester.results[0].evidence.is_empty());

    let editor = pm
        .run_turn(
            &models,
            "implement the parser fix",
            "[source=user]\nimplement the parser fix",
            &runtime,
        )
        .expect("editor turn");
    assert_eq!(
        editor.delegated_roles,
        vec![AgentRole::Reader, AgentRole::Editor, AgentRole::Tester]
    );
    assert!(editor
        .results
        .iter()
        .any(|result| result.summary.contains("target")));
    assert!(editor
        .results
        .iter()
        .any(|result| !result.changed_files.is_empty()));

    let reviewer = pm
        .run_turn(
            &models,
            "review the current diff",
            "[source=user]\nreview the current diff",
            &runtime,
        )
        .expect("reviewer turn");
    assert_eq!(reviewer.delegated_roles, vec![AgentRole::Reviewer]);
    assert!(reviewer.results[0].summary.contains("summarized the current diff"));
}

#[test]
fn task_analyzer_and_turn_planner_classify_repo_grounded_work() {
    let analyzer = TaskAnalyzer;
    let planner = TurnPlanner;

    let repo_analysis = analyzer.analyze("このリポジトリを解析して");
    assert_eq!(repo_analysis.kind, TaskKind::RepositoryAnalysis);
    assert!(repo_analysis.needs_repo_grounding);

    let repo_plan = planner.build("このリポジトリを解析して", &repo_analysis);
    assert!(!repo_plan.allow_fast_path);
    assert_eq!(repo_plan.steps.len(), 2);
    assert_eq!(repo_plan.steps[0].role, AgentRole::Reader);
    assert_eq!(repo_plan.steps[1].role, AgentRole::Reviewer);

    let change = analyzer.analyze("implement the parser fix");
    assert_eq!(change.kind, TaskKind::Change);
    let change_plan = planner.build("implement the parser fix", &change);
    assert_eq!(
        change_plan
            .steps
            .iter()
            .map(|step| step.role)
            .collect::<Vec<_>>(),
        vec![AgentRole::Reader, AgentRole::Editor, AgentRole::Tester]
    );

    let conversational = analyzer.analyze("what is the current objective?");
    assert_eq!(conversational.kind, TaskKind::Conversational);
    let conversational_plan = planner.build("what is the current objective?", &conversational);
    assert!(conversational_plan.allow_fast_path);
    assert!(conversational_plan.steps.is_empty());
}

#[test]
fn step_executor_halts_after_confirmation_or_blocked_step() {
    let executor = StepExecutor;
    let steps = vec![
        PlanStep {
            role: AgentRole::Reader,
            objective: "read".to_string(),
        },
        PlanStep {
            role: AgentRole::Tester,
            objective: "test".to_string(),
        },
        PlanStep {
            role: AgentRole::Reviewer,
            objective: "review".to_string(),
        },
    ];

    let trace = executor.execute(&steps, "[source=user]\nrun checks", |step, _| match step.role {
        AgentRole::Reader => AgentResult::new("reader", "Reader inspected the repo"),
        AgentRole::Tester => AgentResult::awaiting_confirmation(
            "tester",
            "Tester awaiting confirmation: networked commands require explicit approval",
        ),
        AgentRole::Reviewer => AgentResult::new("reviewer", "Reviewer summarized the diff"),
        AgentRole::Editor => AgentResult::new("editor", "Editor prepared a patch"),
    });

    assert_eq!(trace.delegated_roles, vec![AgentRole::Reader, AgentRole::Tester]);
    assert_eq!(trace.results.len(), 2);
    assert!(trace.results[1].needs_confirmation());
}

#[test]
#[ignore = "requires a running LM Studio server and a loaded local model"]
fn lm_studio_live_smoke_test() {
    let model =
        env::var("ANVIL_LM_STUDIO_MODEL").unwrap_or_else(|_| "lmstudio/qwen2.5-coder".to_string());
    let router = ModelRouter::default();

    let response = router
        .complete(&ModelRequest {
            model: model.clone(),
            system_prompt: "Reply with exactly: OK".to_string(),
            user_prompt: "Reply with exactly: OK".to_string(),
        })
        .expect("lm studio response");

    assert_eq!(response.provider, "lm_studio");
    assert_eq!(response.model, model);
    assert!(response.output.contains("OK"));
}

fn sample_session() -> SessionState {
    SessionState {
        session_id: "session-1".to_string(),
        pm_model: "pm-model".to_string(),
        permission_mode: PermissionMode::ReadOnly,
        network_policy: NetworkPolicy::Disabled,
        agent_models: AgentModels::default(),
        objective: "objective".to_string(),
        working_summary: "summary".to_string(),
        active_plan_summary: String::new(),
        latest_evidence_summary: String::new(),
        user_preferences_summary: String::new(),
        repository_summary: String::new(),
        active_constraints: Vec::new(),
        open_questions: Vec::new(),
        completed_steps: Vec::new(),
        pending_steps: Vec::new(),
        relevant_files: Vec::new(),
        recent_delegations: Vec::new(),
        recent_results: Vec::new(),
        pending_confirmation: None,
    }
}

fn runtime(permission_mode: PermissionMode) -> RuntimeEngine {
    let root = std::env::current_dir().expect("cwd");
    runtime_at(root, permission_mode)
}

fn runtime_at(root: std::path::PathBuf, permission_mode: PermissionMode) -> RuntimeEngine {
    RuntimeEngine::new(
        SandboxPolicy::new(permission_mode, NetworkPolicy::Disabled, root, vec![]),
        ToolRegistry::default(),
        RepoInstructions::default(),
    )
}
