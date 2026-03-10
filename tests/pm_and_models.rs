use anvil::agents::pm::{AgentRole, PmAgent};
use anvil::models::client::{ModelClient, ModelRequest, ModelResponse};
use anvil::models::routing::ModelRouter;
use anvil::roles::{EffectiveModels, RoleRegistry};
use anvil::runtime::loop_state::RuntimeLoop;
use anvil::runtime::{NetworkPolicy, PermissionMode};
use anvil::state::session::{AgentModels, SessionState};
use clap::Parser;

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
    let cli = anvil::cli::Cli::parse_from(["anvil", "--model", "pm-model"]);
    let models = EffectiveModels::from_cli(&cli, &registry).expect("models");
    let pm = PmAgent::new(ModelRouter::new(vec![Box::new(TestClient {
        prefix: "",
        provider: "pm-test",
    })]));

    let outcome = pm
        .run_turn(
            &models,
            "what is the current objective?",
            "[source=user]\nwhat is the current objective?",
        )
        .expect("pm turn");

    assert!(outcome.delegated_role.is_none());
    assert!(outcome
        .result
        .summary
        .contains("PM handled the request directly"));
}

#[test]
fn pm_agent_delegates_editor_work() {
    let registry = RoleRegistry::load_builtin().expect("registry");
    let cli = anvil::cli::Cli::parse_from(["anvil", "--model", "pm-model"]);
    let models = EffectiveModels::from_cli(&cli, &registry).expect("models");
    let pm = PmAgent::default();

    let outcome = pm
        .run_turn(
            &models,
            "implement the parser fix",
            "[source=user]\nimplement the parser fix",
        )
        .expect("pm turn");

    assert_eq!(outcome.delegated_role, Some(AgentRole::Editor));
    assert!(outcome.result.summary.contains("Editor prepared"));
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

    let summary = RuntimeLoop::run_prompt(
        &mut session,
        &models,
        &pm,
        "[source=user]\nreview the current diff",
        "review the current diff",
    )
    .expect("runtime loop");

    assert!(summary.contains("Reviewer prepared"));
    assert_eq!(session.recent_delegations.len(), 1);
    assert_eq!(session.recent_delegations[0].role, "reviewer");
    assert_eq!(session.recent_delegations[0].resolved_model, "review-model");
    assert_eq!(session.recent_results.len(), 1);
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
        user_preferences_summary: String::new(),
        repository_summary: String::new(),
        active_constraints: Vec::new(),
        open_questions: Vec::new(),
        completed_steps: Vec::new(),
        pending_steps: Vec::new(),
        relevant_files: Vec::new(),
        recent_delegations: Vec::new(),
        recent_results: Vec::new(),
    }
}
