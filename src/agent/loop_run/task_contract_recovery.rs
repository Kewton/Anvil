//! TaskContract recovery action / target planners extracted from
//! `turn.rs` (parent #680).
//!
//! Hosts the per-turn `TaskContract` recovery planner:
//!
//! - `task_contract_recovery_action` — production chokepoint. Decides
//!   between `RunVerifier` (verifier-repair ready or completion-probe
//!   says run), `Continue { missing }` (artifact-state derived), or
//!   `Done`. Calls the `artifact_state_projection` SSOT for ledger /
//!   legacy alignment, computes verifier-repair readiness, and
//!   consults `project_probe::probe_completion` to short-circuit
//!   speculative `Continue` decisions when the workspace already
//!   demonstrates done-ness.
//! - `task_contract_recovery_target` — projection from
//!   `CompletionDecision::Continue { missing }` to a
//!   `RecoveryTargetHint` for the first missing role. Tries: scaffold
//!   candidate → scope-internal `Owned` workspace artifact →
//!   synthesised conventional implementation file.
//! - `task_contract_repair_state` (private) — adapter to
//!   `super::repair_job::task_contract_repair_state_from_job` carrying
//!   the per-turn `task_contract_verifier_repair_pending` flag and the
//!   active `repair_job` snapshot.
//!
//! Originally `impl Agent` methods; converted to free functions taking
//! `&mut Agent` / `&Agent`, matching the `actor_loop_flow` /
//! `reply_retry` / earlier vertical-slice precedent. `pub(super)`
//! limited / no facade re-export (DR3-001).

use super::Agent;
use super::verifier_orchestration::synthesized_missing_implementation_target_path_for_request;
use super::workspace_candidates::existing_workspace_candidate_for_role_in_scope;

fn task_contract_repair_state(
    agent: &Agent,
    repair_edit_count: Option<usize>,
    repo_edit_calls_made_this_turn: usize,
) -> super::task_contract::VerifierRepairState {
    super::repair_job::task_contract_repair_state_from_job(
        agent.task_contract_verifier_repair_pending,
        agent.repair_job.as_ref(),
        repair_edit_count,
        repo_edit_calls_made_this_turn,
    )
}

pub(super) fn task_contract_recovery_action(
    agent: &mut Agent,
    contract: &super::task_contract::TaskContract,
    repair_edit_count: Option<usize>,
    repo_edit_calls_made_this_turn: usize,
) -> super::task_contract::ArtifactRecoveryAction {
    super::agent_misc::refresh_artifact_completion_satisfied(agent);
    let artifacts =
        super::artifact_state_projection::task_contract_artifact_states(agent, contract);
    if let Some(action) = satisfied_artifact_job_action(agent, contract, &artifacts) {
        return finalize_recovery_action(agent, contract, action);
    }

    let verifier_repair_ready_to_verify = agent.task_contract_verifier_repair_pending
        && (repair_edit_count
            .is_some_and(|edit_count| repo_edit_calls_made_this_turn > edit_count)
            || agent.repair_job.as_ref().is_some_and(|job| {
                matches!(
                    job.next_action(),
                    super::repair_job::RepairNextAction::RerunVerifier
                        | super::repair_job::RepairNextAction::VerifiedDone
                )
            }));
    if verifier_repair_ready_to_verify {
        return finalize_recovery_action(
            agent,
            contract,
            super::task_contract::ArtifactRecoveryAction::RunVerifier,
        );
    }
    let repair_state =
        task_contract_repair_state(agent, repair_edit_count, repo_edit_calls_made_this_turn);
    let missing_verifier_suppress_retry = agent
        .missing_verifier_job
        .as_ref()
        .is_some_and(|job| job.should_suppress_verifier_retry());
    // Issue #651 Phase 5: feed the SSOT `owned_test_artifacts` slice
    // into the planner so the SafeStop gate (test_execution_required
    // && owned_test_artifacts.is_empty()) can fire.
    let owned_test_artifacts =
        super::owned_test_projection::owned_test_artifacts_for_verifier(agent, contract);
    let action = super::task_contract::plan_artifact_recovery(
        super::task_contract::ArtifactRecoveryInputs {
            contract,
            evidence: &agent.task_contract_evidence_set_this_turn,
            artifacts: &artifacts,
            repair_state: &repair_state,
            artifact_excerpts: &agent.task_contract_excerpts,
            missing_verifier_suppress_retry,
            owned_test_artifacts: &owned_test_artifacts,
        },
    );
    if matches!(
        action,
        super::task_contract::ArtifactRecoveryAction::RunVerifier
    ) && super::behavior_delta_obligation::required_behavior_delta_missing_source_edit(
        contract,
        &agent.task_contract_evidence_set_this_turn,
    ) {
        if let Some(obligation) =
            super::behavior_delta_obligation::project_required_behavior_delta_obligation(contract)
        {
            super::behavior_delta_obligation::log_behavior_delta_required(
                agent.session_store.session_id(),
                agent.current_turn_index,
                &obligation,
            );
        }
        let missing = vec![super::task_contract::ArtifactRole::Implementation];
        let decision = super::task_contract::CompletionDecision::Continue {
            missing: missing.clone(),
        };
        let target_hint =
            super::task_contract_recovery_planning::recovery_target_hint_for_missing_with_contract(
                contract,
                &artifacts,
                &agent.task_contract_excerpts,
                &missing,
            )
            .or_else(|| task_contract_recovery_target(agent, &decision));
        return finalize_recovery_action(
            agent,
            contract,
            super::task_contract::ArtifactRecoveryAction::Continue {
                missing,
                target_hint,
            },
        );
    }
    if !matches!(
        action,
        super::task_contract::ArtifactRecoveryAction::Continue { .. }
            | super::task_contract::ArtifactRecoveryAction::RepairArtifact { .. }
    ) && let Some(fresh_action) =
        super::deliverable_freshness::stale_supporting_deliverable_action(contract, &artifacts)
    {
        return finalize_recovery_action(agent, contract, fresh_action);
    }
    if let Some(probe_action) = super::completion_probe_gate::completion_probe_override(
        agent,
        contract,
        &action,
        &owned_test_artifacts,
    ) {
        return finalize_recovery_action(agent, contract, probe_action);
    }
    finalize_recovery_action(agent, contract, action)
}

fn finalize_recovery_action(
    agent: &Agent,
    contract: &super::task_contract::TaskContract,
    action: super::task_contract::ArtifactRecoveryAction,
) -> super::task_contract::ArtifactRecoveryAction {
    let action = align_recovery_action_with_contract(contract, action);
    super::post_tool_reconciliation::emit_post_tool_reconciliation(agent, contract, &action);
    action
}

fn align_recovery_action_with_contract(
    contract: &super::task_contract::TaskContract,
    action: super::task_contract::ArtifactRecoveryAction,
) -> super::task_contract::ArtifactRecoveryAction {
    match action {
        super::task_contract::ArtifactRecoveryAction::Continue {
            missing,
            target_hint,
        } => {
            let role = target_hint
                .as_ref()
                .map(|hint| hint.role)
                .or_else(|| missing.first().copied());
            super::task_contract::ArtifactRecoveryAction::Continue {
                missing,
                target_hint: align_target_hint_with_contract(contract, role, target_hint),
            }
        }
        super::task_contract::ArtifactRecoveryAction::RepairArtifact { target_hint } => {
            let role = target_hint.as_ref().map(|hint| hint.role);
            super::task_contract::ArtifactRecoveryAction::RepairArtifact {
                target_hint: align_target_hint_with_contract(contract, role, target_hint),
            }
        }
        other => other,
    }
}

fn align_target_hint_with_contract(
    contract: &super::task_contract::TaskContract,
    role: Option<super::task_contract::ArtifactRole>,
    target_hint: Option<super::task_contract::RecoveryTargetHint>,
) -> Option<super::task_contract::RecoveryTargetHint> {
    let role = role?;
    let identities = contract.required_identities_for_role(role);
    let Some(identity) = identities.first() else {
        return target_hint;
    };
    if target_hint
        .as_ref()
        .is_some_and(|hint| hint.role == role && hint.path == identity.path)
    {
        return target_hint;
    }
    Some(super::task_contract::RecoveryTargetHint {
        role,
        path: identity.path.clone(),
        reason: format!(
            "required deliverable obligation is still missing: {}",
            super::task_contract::obligation_report_label(identity)
        ),
    })
}

pub(super) fn record_obligation_diagnostic_attempt_for_action(
    agent: &mut Agent,
    contract: &super::task_contract::TaskContract,
    action: &super::task_contract::ArtifactRecoveryAction,
    repo_edit_calls_made_this_turn: usize,
) -> bool {
    if repo_edit_calls_made_this_turn == 0 {
        return false;
    }
    let super::task_contract::ArtifactRecoveryAction::Continue {
        target_hint: Some(target_hint),
        ..
    } = action
    else {
        return false;
    };
    let Some((job_role, job_path)) = agent
        .artifact_completion_job
        .as_ref()
        .map(|job| (job.role(), job.target_path().to_string()))
    else {
        return false;
    };
    if job_role != target_hint.role || job_path != target_hint.path {
        return false;
    }
    let artifacts =
        super::artifact_state_projection::task_contract_artifact_states(agent, contract);
    let Some(blocking) = super::task_contract::blocking_obligation_diagnostic_for_role(
        contract,
        &artifacts,
        &agent.task_contract_excerpts,
        job_role,
    )
    .filter(|diagnostic| {
        diagnostic.target_hint.path == job_path
            && diagnostic.code != super::verifier::VerifierDiagnosticCode::MissingFile
    }) else {
        return false;
    };
    super::artifact_completion_record::record_artifact_completion_evidence_failure(
        agent,
        &blocking.target_hint.reason,
    )
}

fn satisfied_artifact_job_action(
    agent: &Agent,
    contract: &super::task_contract::TaskContract,
    artifacts: &[super::task_contract::ArtifactState],
) -> Option<super::task_contract::ArtifactRecoveryAction> {
    let job = agent.artifact_completion_job.as_ref()?;
    if !matches!(
        job.status(),
        super::artifact_completion_job::ArtifactCompletionStatus::Satisfied
    ) {
        return None;
    }
    if let Some(target_hint) =
        first_blocking_required_obligation_hint(contract, artifacts, &agent.task_contract_excerpts)
    {
        return Some(super::task_contract::ArtifactRecoveryAction::Continue {
            missing: vec![target_hint.role],
            target_hint: Some(target_hint),
        });
    }
    Some(if contract.objective_contract().requires_evidence() {
        super::task_contract::ArtifactRecoveryAction::RunVerifier
    } else {
        super::task_contract::ArtifactRecoveryAction::Done
    })
}

fn first_blocking_required_obligation_hint(
    contract: &super::task_contract::TaskContract,
    artifacts: &[super::task_contract::ArtifactState],
    artifact_excerpts: &super::task_contract::ArtifactExcerpts,
) -> Option<super::task_contract::RecoveryTargetHint> {
    contract
        .objective_contract()
        .required_deliverables()
        .iter()
        .find_map(|role| {
            super::task_contract::recovery_target_hint_for_blocking_obligation_diagnostic(
                contract,
                artifacts,
                artifact_excerpts,
                *role,
            )
        })
}

pub(super) fn task_contract_recovery_target(
    agent: &Agent,
    decision: &super::task_contract::CompletionDecision,
) -> Option<super::task_contract::RecoveryTargetHint> {
    let super::task_contract::CompletionDecision::Continue { missing } = decision else {
        return None;
    };
    let role = missing.first().copied()?;
    if let Some(identity) = super::workspace_access::active_request_text(agent)
        .as_deref()
        .and_then(|request| {
            super::task_contract::explicit_artifact_obligations_from_request(request)
                .into_iter()
                .find(|identity| identity.role == role)
        })
    {
        return Some(super::task_contract::RecoveryTargetHint {
            role,
            path: identity.path,
            reason: "required artifact identity is still missing".to_string(),
        });
    }
    if let Some(path) = super::scaffold_pipeline::scaffold_candidate_for_missing_role(agent, role) {
        return Some(super::task_contract::RecoveryTargetHint {
            role,
            path,
            reason: "bootstrap scaffold artifact for the missing role is still unchanged"
                .to_string(),
        });
    }
    // Issue #646 (D1): two-step gate — first scope, then full ownership
    // classifier. A scope-internal but non-`Owned` artifact (e.g. an
    // unchanged scaffold body, a CandidateOnly README the user never
    // mentioned) MUST NOT be surfaced as a recovery target either. The
    // ownership signal — edit / scaffold delta / explicit scope mention
    // — is the same one the planner uses upstream.
    let scope = super::workspace_access::current_workspace_scope(agent);
    if let Some(path) =
        existing_workspace_candidate_for_role_in_scope(&agent.work_root, role, &scope)
    {
        let scaffold_changed =
            super::scaffold_pipeline::repo_edit_has_post_scaffold_delta(agent, &path);
        let edited_this_session = agent.turn_edited_relative_paths.contains(&path);
        let ownership = super::artifact_ownership::classify_ownership(
            super::artifact_ownership::OwnershipInputs {
                work_root: &agent.work_root,
                relative_path: &path,
                scope: &scope,
                edited_this_session,
                scaffold_changed,
                verifier_passed_in_scope: false,
                // Keep target selection conservative. The ledger/verifier
                // projection is the only place that broadens nested-test
                // admission for current-task evidence; generic recovery must
                // not claim pre-existing nested tests without evidence.
                nested_test_admission: super::artifact_ownership::NestedTestAdmission::default(),
            },
        );
        if matches!(
            ownership,
            super::artifact_ownership::ArtifactOwnership::Owned
        ) {
            return Some(super::task_contract::RecoveryTargetHint {
                role,
                path,
                reason: "existing workspace artifact matches the missing role".to_string(),
            });
        }
    }
    if let Some(path) = super::workspace_access::active_request_text(agent)
        .as_deref()
        .and_then(|req| synthesized_missing_implementation_target_path_for_request(role, req))
    {
        return Some(super::task_contract::RecoveryTargetHint {
            role,
            path,
            reason: "no existing implementation artifact for the active request; create a conventional implementation file"
                .to_string(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::loop_run::artifact_completion_job::{
        ArtifactAttemptOutcomeKind, ArtifactCompletionJob, ArtifactCompletionStatus,
    };
    use crate::agent::loop_run::artifact_ledger::LedgerAdmissionContext;
    use crate::agent::loop_run::commands::test_agent_with_config;
    use crate::agent::loop_run::completion_evidence::{CompletionEvidence, RepoEditCategory};
    use crate::agent::loop_run::project_profile::parse_project_profile_confirmation;
    use crate::agent::loop_run::task_contract::{
        ArtifactRecoveryAction, ArtifactRole, RecoveryTargetHint, TaskContract, TaskKind,
    };
    use crate::config::Config;
    use crate::session::store::ConversationMessage;

    #[test]
    fn behavior_delta_blocks_preexisting_verifier_until_source_edit() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = "Improve the existing discounts.py function final_price(price, percent). It should return the price after applying the percentage discount, rounded to 2 decimal places. Keep the existing zero-discount behavior and update tests/test_discounts.py.";
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        super::super::task_classification::populate_task_contract_authority(&mut agent);
        std::fs::create_dir_all(agent.work_root.join("tests")).unwrap();
        std::fs::write(
            agent.work_root.join("discounts.py"),
            "def final_price(price, percent):\n    return price\n",
        )
        .unwrap();
        std::fs::write(
            agent.work_root.join("tests/test_discounts.py"),
            "from discounts import final_price\n\n\ndef test_zero_discount():\n    assert final_price(10, 0) == 10\n",
        )
        .unwrap();
        let contract = TaskContract::from_request(request);

        let action = task_contract_recovery_action(&mut agent, &contract, None, 0);

        assert!(
            matches!(
                action,
                ArtifactRecoveryAction::Continue {
                    ref missing,
                    target_hint: Some(RecoveryTargetHint {
                        role: ArtifactRole::Implementation,
                        ref path,
                        ..
                    }),
                } if missing == &vec![ArtifactRole::Implementation] && path == "discounts.py"
            ),
            "pre-existing verifier evidence must not satisfy a current behavior delta before source edit: {action:?}"
        );
    }

    #[test]
    fn completion_probe_does_not_override_missing_owned_test_repair() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = concat!(
            "# CSV to JSON CLI Tool\n\n",
            "A Node.js CLI tool that converts CSV input to JSON array output.\n\n",
            "## Features\n",
            "- Support stdin or file path as input\n",
            "- Parse quoted commas and escaped quotes correctly\n",
            "- Fail clearly on malformed rows\n",
            "- Output JSON array to stdout\n\n",
            "## Usage\n",
            "From file path: node index.js data.csv\n",
            "From stdin: cat data.csv | node index.js\n\n",
            "## Installation\nnpm install\n\n",
            "## Testing\nnpm test"
        );
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        super::super::task_classification::populate_task_contract_authority(&mut agent);

        let contract = TaskContract::from_request(request);
        assert_eq!(contract.task_kind, TaskKind::Coding);
        assert!(contract.completion_policy.test_execution_required());

        std::fs::create_dir_all(agent.work_root.join("tests")).unwrap();
        std::fs::write(
            agent.work_root.join("index.js"),
            "function parseCsv(input) { return []; }\nmodule.exports = { parseCsv };\n",
        )
        .unwrap();
        std::fs::write(
            agent.work_root.join("tests/index.test.js"),
            "const { test } = require('node:test');\ntest('smoke', () => {});\n",
        )
        .unwrap();
        std::fs::write(
            agent.work_root.join("README.md"),
            "# CSV to JSON CLI Tool\n\n## Installation\nnpm install\n\n## Testing\nnpm test\n",
        )
        .unwrap();
        std::fs::write(
            agent.work_root.join("package.json"),
            r#"{"name":"csv-to-json-cli","scripts":{"test":"node --test tests/index.test.js"}}"#,
        )
        .unwrap();

        let scope = super::super::workspace_access::current_workspace_scope(&agent);
        for (path, role, category) in [
            (
                "index.js",
                ArtifactRole::Implementation,
                RepoEditCategory::Impl,
            ),
            (
                "tests/index.test.js",
                ArtifactRole::Test,
                RepoEditCategory::Test,
            ),
            ("README.md", ArtifactRole::UsageDocs, RepoEditCategory::Docs),
            ("package.json", ArtifactRole::Setup, RepoEditCategory::Setup),
        ] {
            agent.turn_edited_relative_paths.insert(path.to_string());
            agent
                .task_contract_evidence_set_this_turn
                .push(CompletionEvidence::RepoEdit {
                    category,
                    count: 1,
                    path: Some(path.to_string()),
                });
            let recorded = agent.artifact_ledger.record_repo_edit_event(
                &LedgerAdmissionContext::new(&agent.work_root, &scope),
                path.to_string(),
                role,
                true,
            );
            assert!(recorded.is_some(), "repo edit must be recorded: {path}");
        }

        let action = task_contract_recovery_action(&mut agent, &contract, None, 4);
        assert!(
            matches!(
                action,
                ArtifactRecoveryAction::Continue {
                    ref missing,
                    target_hint: Some(RecoveryTargetHint {
                        role: ArtifactRole::Test,
                        ref path,
                        ..
                    }),
                } if missing == &vec![ArtifactRole::Test] && path == "tests/index.test.js"
            ),
            "preflight-rejected tests must stay in Test repair instead of completion-probe RunVerifier: {action:?}"
        );
    }

    #[test]
    fn post_tool_recovery_action_keeps_profile_test_identity_over_rust_family_hint() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = "Coding TDD task: create math_utils.py and tests/test_math_utils.py only. Implement clamp(value, minimum, maximum): return minimum when value is below minimum, maximum when value is above maximum, otherwise value. Use Python unittest and verify with python -m unittest discover -s tests. Do not create README, package.json, Cargo.toml, or setup files.";
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        std::fs::write(
            agent.work_root.join("Cargo.toml"),
            "[package]\nname = \"ambient-rust-manifest\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(
            agent.work_root.join("math_utils.py"),
            "def clamp(value, minimum, maximum):\n    return max(minimum, min(value, maximum))\n",
        )
        .unwrap();
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"python",
                "shape":"library",
                "deliverable_kind":"code",
                "primary_artifacts":["math_utils.py","tests/test_math_utils.py"],
                "forbidden_artifacts":["setup","docs"],
                "evidence_kind":"test_run",
                "needs_environment_setup":false,
                "preferred_runner":null,
                "confidence":1.0,
                "reason":"explicit Python code and unittest files"
            }"#,
        )
        .expect("profile");
        let contract =
            TaskContract::from_request_with_kind_and_project_profile(request, None, Some(&profile));
        assert_eq!(
            contract
                .required_identities_for_role(ArtifactRole::Test)
                .first()
                .map(|identity| identity.path.as_str()),
            Some("tests/test_math_utils.py")
        );

        let scope = super::super::workspace_access::current_workspace_scope(&agent);
        agent
            .turn_edited_relative_paths
            .insert("math_utils.py".to_string());
        agent
            .task_contract_evidence_set_this_turn
            .push(CompletionEvidence::RepoEdit {
                category: RepoEditCategory::Impl,
                count: 1,
                path: Some("math_utils.py".to_string()),
            });
        let recorded = agent.artifact_ledger.record_repo_edit_event(
            &LedgerAdmissionContext::new(&agent.work_root, &scope),
            "math_utils.py".to_string(),
            ArtifactRole::Implementation,
            true,
        );
        assert!(recorded.is_some(), "implementation edit must be recorded");

        let action = task_contract_recovery_action(&mut agent, &contract, None, 1);
        let ArtifactRecoveryAction::Continue {
            missing,
            target_hint: Some(target_hint),
        } = action
        else {
            panic!("expected missing test recovery action, got {action:?}");
        };
        assert_eq!(missing, vec![ArtifactRole::Test]);
        assert_eq!(target_hint.role, ArtifactRole::Test);
        assert_eq!(target_hint.path, "tests/test_math_utils.py");
        assert_eq!(
            target_hint.reason,
            "required deliverable obligation is still missing: role=test, kind=file, path=tests/test_math_utils.py"
        );
    }

    #[test]
    fn post_tool_recovery_ignores_stale_owned_test_when_contract_test_identity_is_missing() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = "Coding TDD task: create math_utils.py and tests/test_math_utils.py only. Implement clamp(value, minimum, maximum): return minimum when value is below minimum, maximum when value is above maximum, otherwise value. Use Python unittest and verify with python -m unittest discover -s tests. Do not create README, package.json, Cargo.toml, or setup files.";
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        std::fs::create_dir_all(agent.work_root.join("tests")).unwrap();
        std::fs::write(
            agent.work_root.join("Cargo.toml"),
            "[package]\nname = \"ambient-rust-manifest\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        std::fs::write(
            agent.work_root.join("math_utils.py"),
            "def clamp(value, minimum, maximum):\n    return max(minimum, min(value, maximum))\n",
        )
        .unwrap();
        std::fs::write(agent.work_root.join("tests/cli.rs"), "# stale rust test\n").unwrap();
        let profile = parse_project_profile_confirmation(
            r#"{
                "language":"python",
                "shape":"library",
                "deliverable_kind":"code",
                "primary_artifacts":["math_utils.py","tests/test_math_utils.py"],
                "forbidden_artifacts":["setup","docs"],
                "evidence_kind":"test_run",
                "needs_environment_setup":false,
                "preferred_runner":null,
                "confidence":1.0,
                "reason":"explicit Python code and unittest files"
            }"#,
        )
        .expect("profile");
        let contract =
            TaskContract::from_request_with_kind_and_project_profile(request, None, Some(&profile));

        let scope = super::super::workspace_access::current_workspace_scope(&agent);
        for (path, role, category) in [
            (
                "math_utils.py",
                ArtifactRole::Implementation,
                RepoEditCategory::Impl,
            ),
            ("tests/cli.rs", ArtifactRole::Test, RepoEditCategory::Test),
        ] {
            agent.turn_edited_relative_paths.insert(path.to_string());
            agent
                .task_contract_evidence_set_this_turn
                .push(CompletionEvidence::RepoEdit {
                    category,
                    count: 1,
                    path: Some(path.to_string()),
                });
            let recorded = agent.artifact_ledger.record_repo_edit_event(
                &LedgerAdmissionContext::new(&agent.work_root, &scope),
                path.to_string(),
                role,
                true,
            );
            assert!(recorded.is_some(), "repo edit must be recorded: {path}");
        }

        let action = task_contract_recovery_action(&mut agent, &contract, None, 2);
        let ArtifactRecoveryAction::Continue {
            missing,
            target_hint: Some(target_hint),
        } = action
        else {
            panic!("expected missing contract test recovery action, got {action:?}");
        };
        assert_eq!(missing, vec![ArtifactRole::Test]);
        assert_eq!(target_hint.role, ArtifactRole::Test);
        assert_eq!(target_hint.path, "tests/test_math_utils.py");
        assert_eq!(
            target_hint.reason,
            "required deliverable obligation is still missing: role=test, kind=file, path=tests/test_math_utils.py"
        );
    }

    #[test]
    fn docs_artifact_satisfied_without_verification_returns_done() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = concat!(
            "STATE_CONTROL_PACKET\n",
            r#"{"objective":"Create README.md documentation with Setup and Usage sections.","next_required_action":"artifact","required_artifacts":[{"path":"README.md","role":"docs"}]}"#
        );
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        super::super::task_classification::populate_task_contract_authority(&mut agent);

        let contract = TaskContract::from_request(request);
        assert_eq!(contract.task_kind, TaskKind::Docs);
        assert!(
            contract
                .required_artifacts
                .contains(&ArtifactRole::UsageDocs)
        );
        assert!(
            !contract.verification_required,
            "plain docs artifact completion should not require a coding verifier"
        );

        let scope = super::super::workspace_access::current_workspace_scope(&agent);
        agent.artifact_completion_job = Some(
            ArtifactCompletionJob::new(
                &agent.work_root,
                &scope,
                RecoveryTargetHint {
                    role: ArtifactRole::UsageDocs,
                    path: "README.md".to_string(),
                    reason: "missing docs artifact".to_string(),
                },
                true,
                false,
            )
            .expect("docs artifact completion job"),
        );
        std::fs::write(
            agent.work_root.join("README.md"),
            "# Local Notes CLI\n\n## Setup\n\nInstall the binary.\n\n## Usage\n\nRun notes from the shell.\n",
        )
        .unwrap();
        agent
            .turn_edited_relative_paths
            .insert("README.md".to_string());
        agent
            .task_contract_excerpts
            .insert(ArtifactRole::UsageDocs, "# Local Notes CLI\n\n## Setup\n\nInstall the binary.\n\n## Usage\n\nRun notes from the shell.\n".to_string());
        agent
            .task_contract_evidence_set_this_turn
            .push(CompletionEvidence::RepoEdit {
                category: RepoEditCategory::Docs,
                count: 1,
                path: Some("README.md".to_string()),
            });
        let recorded = agent.artifact_ledger.record_repo_edit_event(
            &LedgerAdmissionContext::new(&agent.work_root, &scope),
            "README.md".to_string(),
            ArtifactRole::UsageDocs,
            true,
        );
        assert!(
            recorded.is_some(),
            "README.md repo edit must be admitted as UsageDocs evidence"
        );
        let completed = agent
            .artifact_ledger
            .required_artifacts_completed(&contract);
        assert_eq!(completed.get(&ArtifactRole::UsageDocs), Some(&true));
        let states =
            super::super::artifact_state_projection::task_contract_artifact_states_for_test(
                &mut agent, &contract,
            );
        assert!(
            states
                .iter()
                .any(|state| state.role == ArtifactRole::UsageDocs
                    && state.path.as_deref() == Some("README.md")),
            "states={states:?}"
        );

        let action = task_contract_recovery_action(&mut agent, &contract, None, 1);
        assert_eq!(action, ArtifactRecoveryAction::Done);
        assert!(
            agent
                .artifact_completion_job
                .as_ref()
                .is_some_and(|job| { matches!(job.status(), ArtifactCompletionStatus::Satisfied) })
        );
    }

    #[test]
    fn satisfied_impl_job_still_blocks_on_missing_manifest_deliverable() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = concat!(
            "STATE_CONTROL_PACKET\n",
            r#"{"objective":"Create Rust slugify library with passing evidence","next_required_action":"artifact","required_artifacts":[{"path":"Cargo.toml","role":"manifest"},{"path":"src/lib.rs","role":"source"}],"evidence_command":"cargo test --manifest-path Cargo.toml"}"#,
            "\nCreate the Rust library."
        );
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        super::super::task_classification::populate_task_contract_authority(&mut agent);

        let contract = TaskContract::from_request(request);
        assert!(contract.verification_required);
        assert_eq!(contract.task_kind, TaskKind::Coding);
        let scope = super::super::workspace_access::current_workspace_scope(&agent);
        agent.artifact_completion_job = Some(
            ArtifactCompletionJob::new(
                &agent.work_root,
                &scope,
                RecoveryTargetHint {
                    role: ArtifactRole::Implementation,
                    path: "src/lib.rs".to_string(),
                    reason: "missing implementation artifact".to_string(),
                },
                true,
                false,
            )
            .expect("implementation artifact completion job"),
        );
        std::fs::create_dir_all(agent.work_root.join("src")).unwrap();
        std::fs::write(
            agent.work_root.join("src/lib.rs"),
            "pub fn slugify(input: &str) -> String { input.to_ascii_lowercase() }\n",
        )
        .unwrap();
        agent
            .turn_edited_relative_paths
            .insert("src/lib.rs".to_string());
        agent.task_contract_excerpts.insert(
            ArtifactRole::Implementation,
            "pub fn slugify(input: &str) -> String { input.to_ascii_lowercase() }\n".to_string(),
        );
        agent
            .task_contract_evidence_set_this_turn
            .push(CompletionEvidence::RepoEdit {
                category: RepoEditCategory::Impl,
                count: 1,
                path: Some("src/lib.rs".to_string()),
            });
        let recorded = agent.artifact_ledger.record_repo_edit_event(
            &LedgerAdmissionContext::new(&agent.work_root, &scope),
            "src/lib.rs".to_string(),
            ArtifactRole::Implementation,
            true,
        );
        assert!(
            recorded.is_some(),
            "src/lib.rs repo edit must be admitted as implementation evidence"
        );

        let action = task_contract_recovery_action(&mut agent, &contract, None, 1);
        assert!(
            matches!(
                action,
                ArtifactRecoveryAction::Continue {
                    ref missing,
                    target_hint: Some(RecoveryTargetHint {
                        role: ArtifactRole::Setup,
                        ref path,
                        ..
                    }),
                } if missing == &vec![ArtifactRole::Setup] && path == "Cargo.toml"
            ),
            "satisfied impl job must not advance to verifier while Cargo.toml is missing: {action:?}"
        );
        assert!(
            agent
                .artifact_completion_job
                .as_ref()
                .is_some_and(|job| { matches!(job.status(), ArtifactCompletionStatus::Satisfied) }),
            "implementation job should remain satisfied while the next missing deliverable is selected"
        );
    }

    #[test]
    fn satisfied_data_artifact_job_still_blocks_on_schema_diagnostic() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = "Generate output.csv with columns Category and Total from the input CSV.";
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        super::super::task_classification::populate_task_contract_authority(&mut agent);

        let contract = TaskContract::from_request(request);
        assert_eq!(contract.task_kind, TaskKind::Data);
        let scope = super::super::workspace_access::current_workspace_scope(&agent);
        agent.artifact_completion_job = Some(
            ArtifactCompletionJob::new(
                &agent.work_root,
                &scope,
                RecoveryTargetHint {
                    role: ArtifactRole::DataOutput,
                    path: "output.csv".to_string(),
                    reason: "missing data artifact".to_string(),
                },
                true,
                false,
            )
            .expect("data artifact completion job"),
        );
        std::fs::write(agent.work_root.join("output.csv"), "x,y\n1").unwrap();
        agent
            .turn_edited_relative_paths
            .insert("output.csv".to_string());
        agent
            .task_contract_excerpts
            .insert(ArtifactRole::DataOutput, "x,y\n1".to_string());
        agent
            .task_contract_evidence_set_this_turn
            .push(CompletionEvidence::RepoEdit {
                category: RepoEditCategory::Data,
                count: 1,
                path: Some("output.csv".to_string()),
            });
        let recorded = agent.artifact_ledger.record_repo_edit_event(
            &LedgerAdmissionContext::new(&agent.work_root, &scope),
            "output.csv".to_string(),
            ArtifactRole::DataOutput,
            true,
        );
        assert!(
            recorded.is_some(),
            "output.csv repo edit must be admitted as DataOutput evidence"
        );

        let action = task_contract_recovery_action(&mut agent, &contract, None, 1);
        match action {
            ArtifactRecoveryAction::Continue {
                ref missing,
                target_hint: Some(ref target_hint),
            } => {
                assert_eq!(missing, &vec![ArtifactRole::DataOutput]);
                assert_eq!(target_hint.path, "output.csv");
                assert!(
                    target_hint.reason.contains("schema_mismatch"),
                    "reason={}",
                    target_hint.reason
                );
            }
            other => panic!("expected schema repair Continue, got {other:?}"),
        }
        assert!(
            agent.artifact_completion_job.as_ref().is_some_and(|job| {
                !matches!(job.status(), ArtifactCompletionStatus::Satisfied)
            }),
            "schema-mismatched artifact must not be reported as a satisfied completion job"
        );
        assert!(
            !record_obligation_diagnostic_attempt_for_action(&mut agent, &contract, &action, 0),
            "no repo edit means no semantic evidence failure attempt is recorded"
        );
        let exhausted =
            record_obligation_diagnostic_attempt_for_action(&mut agent, &contract, &action, 1);
        assert!(!exhausted, "first evidence failure should not exhaust");
        let job = agent
            .artifact_completion_job
            .as_ref()
            .expect("artifact completion job");
        assert_eq!(job.attempts().len(), 1);
        assert_eq!(job.remaining_budget(), 3);
        assert_eq!(
            job.attempts()[0].kind(),
            ArtifactAttemptOutcomeKind::EvidenceFailed
        );
        assert!(job.attempts()[0].cluster_key().is_some());
    }

    #[test]
    fn docs_required_section_failure_records_generic_evidence_failed_attempt() {
        let (mut agent, _temp) = test_agent_with_config(Config::default());
        let request = concat!(
            "STATE_CONTROL_PACKET\n",
            r#"{"objective":"Create README.md documentation.","next_required_action":"artifact","required_artifacts":[{"path":"README.md","role":"docs","schema":{"required_sections":["Setup","Usage"]}}]}"#
        );
        agent
            .session
            .messages
            .push(ConversationMessage::user(request.to_string()));
        agent
            .session
            .working_memory
            .set_active_task(Some(request.to_string()));
        super::super::task_classification::populate_task_contract_authority(&mut agent);

        let contract = TaskContract::from_request(request);
        assert_eq!(contract.task_kind, TaskKind::Docs);
        let scope = super::super::workspace_access::current_workspace_scope(&agent);
        agent.artifact_completion_job = Some(
            ArtifactCompletionJob::new(
                &agent.work_root,
                &scope,
                RecoveryTargetHint {
                    role: ArtifactRole::UsageDocs,
                    path: "README.md".to_string(),
                    reason: "missing docs artifact".to_string(),
                },
                true,
                false,
            )
            .expect("docs artifact completion job"),
        );
        std::fs::write(
            agent.work_root.join("README.md"),
            "# Local Notes\n\n## Setup\n\nInstall the binary.\n",
        )
        .unwrap();
        agent
            .turn_edited_relative_paths
            .insert("README.md".to_string());
        agent.task_contract_excerpts.insert(
            ArtifactRole::UsageDocs,
            "# Local Notes\n\n## Setup\n\nInstall the binary.\n".to_string(),
        );
        agent
            .task_contract_evidence_set_this_turn
            .push(CompletionEvidence::RepoEdit {
                category: RepoEditCategory::Docs,
                count: 1,
                path: Some("README.md".to_string()),
            });
        let recorded = agent.artifact_ledger.record_repo_edit_event(
            &LedgerAdmissionContext::new(&agent.work_root, &scope),
            "README.md".to_string(),
            ArtifactRole::UsageDocs,
            true,
        );
        assert!(recorded.is_some());

        let action = task_contract_recovery_action(&mut agent, &contract, None, 1);
        assert!(matches!(
            action,
            ArtifactRecoveryAction::Continue {
                ref missing,
                target_hint: Some(RecoveryTargetHint { ref path, .. }),
            } if missing == &vec![ArtifactRole::UsageDocs] && path == "README.md"
        ));
        let exhausted =
            record_obligation_diagnostic_attempt_for_action(&mut agent, &contract, &action, 1);
        assert!(!exhausted, "first evidence failure should not exhaust");
        let job = agent
            .artifact_completion_job
            .as_ref()
            .expect("artifact completion job");
        assert_eq!(job.attempts().len(), 1);
        assert_eq!(
            job.attempts()[0].kind(),
            ArtifactAttemptOutcomeKind::EvidenceFailed
        );
        assert!(job.attempts()[0].cluster_key().is_some());
    }
}
