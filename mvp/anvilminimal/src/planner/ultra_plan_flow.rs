use super::*;

pub fn generate_ultra_plan(
    client: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
) -> anyhow::Result<UltraPlan> {
    generate_ultra_plan_with_ui(client, goal, config, &NOOP_UI)
}

pub fn generate_ultra_plan_with_ui(
    client: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<UltraPlan> {
    if ui.interrupted() {
        anyhow::bail!("interrupted by user");
    }
    let model = model_for(config, true);
    let intent = detect_intent(goal);
    if let Some(plan) = crate::planner::ultra_preset::maybe_preset_ultra_plan(config, goal, intent)?
    {
        crate::tui::presentation::emit_ultra_plan_card(
            &plan,
            &crate::tui::presentation::PlanProgress::default(),
        );
        return Ok(plan);
    }
    let mut messages = ultra_plan_generation_messages(goal, config);
    let mut last_error = None;
    for attempt in 1..=ULTRA_PLAN_GENERATION_ATTEMPTS {
        emit_ultra_plan_generation_attempt(
            config,
            client.label(),
            model,
            attempt,
            &config.profile,
            &config.style,
            intent,
        );
        let reply = planner_chat_with_request_retry(
            client,
            config,
            ProviderCallScope::PlannerUltra,
            model,
            &messages,
            ui,
        )?;
        ui.publish_status(UiStatus::for_model_reply(
            config,
            model,
            client.label(),
            reply.prompt_tokens,
            reply.completion_tokens,
        ));
        emit_ultra_plan_raw_output_shape(config, client.label(), model, attempt, &reply.content);
        if !reply.tool_calls.is_empty() {
            let message = format!(
                "ultra plan generation must not emit tool calls: {}",
                tool_call_names(&reply.tool_calls)
            );
            last_error = Some(message.clone());
            emit_ultra_plan_generation_tool_call_rejected(
                config,
                client.label(),
                model,
                attempt,
                &message,
            );
            emit_planner_error(
                config,
                client.label(),
                model,
                "schema",
                "planner_schema_error",
                &message,
                attempt,
            );
            if attempt < ULTRA_PLAN_GENERATION_ATTEMPTS {
                emit_ultra_plan_generation_retry(
                    config,
                    client.label(),
                    model,
                    attempt,
                    "planner_schema_error",
                    &message,
                );
                messages.push(crate::state::ConversationMessage::user(
                    build_ultra_plan_tool_call_retry_prompt(goal, attempt),
                ));
                continue;
            }
            break;
        }
        match parse_ultra_plan(&reply.content) {
            Ok(mut plan) => {
                let normalized =
                    normalize_ultra_plan_metadata(&mut plan, goal, &config.profile, &config.style);
                if !normalized.is_empty() {
                    emit_ultra_plan_generation_metadata_normalized(
                        config,
                        client.label(),
                        model,
                        attempt,
                        &normalized,
                    );
                }
                let report = lint_ultra_plan_report(&plan);
                if report.is_pass() {
                    emit_ultra_plan_generation_succeeded(
                        config,
                        client.label(),
                        model,
                        attempt,
                        plan.phases.len(),
                    );
                    crate::tui::presentation::emit_ultra_plan_card(
                        &plan,
                        &crate::tui::presentation::PlanProgress::default(),
                    );
                    return Ok(plan);
                } else {
                    emit_planner_error_for_lint(config, client.label(), model, &report, attempt);
                    let message = report.primary_message();
                    last_error = Some(message.clone());
                    if attempt < ULTRA_PLAN_GENERATION_ATTEMPTS {
                        let (_stage, kind) = planner_stage_and_kind_for_lint(&report);
                        emit_ultra_plan_generation_retry(
                            config,
                            client.label(),
                            model,
                            attempt,
                            kind,
                            &message,
                        );
                        messages.push(crate::state::ConversationMessage::user(
                            build_ultra_plan_lint_retry_prompt(goal, &report, attempt),
                        ));
                        continue;
                    }
                    break;
                }
            }
            Err(err) => {
                let message = err.to_string();
                last_error = Some(message.clone());
                emit_planner_error(
                    config,
                    client.label(),
                    model,
                    "schema",
                    "planner_schema_error",
                    &message,
                    attempt,
                );
                if attempt < ULTRA_PLAN_GENERATION_ATTEMPTS {
                    emit_ultra_plan_generation_retry(
                        config,
                        client.label(),
                        model,
                        attempt,
                        "planner_schema_error",
                        &message,
                    );
                    messages.push(crate::state::ConversationMessage::user(
                        build_ultra_plan_schema_retry_prompt(goal, &message, attempt),
                    ));
                    continue;
                }
                break;
            }
        }
    }
    let message = last_error.unwrap_or_else(|| "unknown UltraPlan generation error".to_string());
    emit_ultra_plan_generation_failed(config, client.label(), model, &message);
    anyhow::bail!("invalid generated UltraPlan after corrective retries: {message}")
}

pub fn save_ultra_plan(root: &Path, plan: &UltraPlan) -> anyhow::Result<PathBuf> {
    let dir = root.join(".anvil").join("plans");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("ultra-plan-{}.yaml", uuid::Uuid::now_v7()));
    std::fs::write(&path, render_ultra_plan(plan))?;
    Ok(path)
}

pub fn run_ultra_plan_file(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    path: &Path,
    config: &Config,
) -> anyhow::Result<String> {
    run_ultra_plan_file_with_ui(planner, execution, path, config, &NOOP_UI)
}

pub fn run_ultra_plan_file_with_ui(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    path: &Path,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    let path = resolve_plan_file_path(&config.workspace_root, path)?;
    let text = std::fs::read_to_string(path)?;
    let plan = parse_ultra_plan(&text)?;
    run_ultra_plan_with_ui(planner, execution, &plan, config, ui)
}

pub fn generate_and_run_ultra_plan(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
) -> anyhow::Result<String> {
    generate_and_run_ultra_plan_with_ui(planner, execution, goal, config, &NOOP_UI)
}

pub fn generate_and_run_ultra_plan_with_ui(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    goal: &str,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    let plan = generate_ultra_plan_with_ui(planner, goal, config, ui)?;
    save_ultra_plan(&config.workspace_root, &plan)?;
    run_ultra_plan_with_ui(planner, execution, &plan, config, ui)
}

pub fn run_ultra_plan(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    plan: &UltraPlan,
    config: &Config,
) -> anyhow::Result<String> {
    run_ultra_plan_with_ui(planner, execution, plan, config, &NOOP_UI)
}

pub fn run_ultra_plan_with_ui(
    planner: &mut dyn ChatClient,
    execution: &mut dyn ChatClient,
    plan: &UltraPlan,
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<String> {
    let preset_plan = crate::planner::ultra_preset::is_profile_preset_plan(config, plan);
    let mut active_plan = plan.clone();
    let mut active_config = config.clone();
    let plan = &mut active_plan;
    let config = &mut active_config;
    let report = lint_ultra_plan_report(plan);
    if !report.is_pass() {
        emit_planner_error_for_lint(config, "ultra-plan-file", &config.planner_model, &report, 0);
        anyhow::bail!("{}", report.primary_message());
    }
    let mut final_expected_paths =
        profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    let mut ultra_context = UltraRunContext::new(missing_final_artifacts(
        &config.workspace_root,
        &final_expected_paths,
    ));
    let mut ultra_session = SessionSnapshot::new();
    let mut promotion_state = ProfilePromotionState::for_run(plan, config);
    let mut setup_authority_state = UltraRunSetupAuthorityState::default();
    emit_ultra_context_initialized(config, plan, &ultra_context, ultra_session.messages.len());
    let phases = plan.phases.clone();
    for (index, phase) in phases.iter().enumerate() {
        if ui.interrupted() {
            anyhow::bail!("interrupted by user");
        }
        emit_ultra_phase_event(
            config,
            "ultra_phase_start",
            plan,
            phase,
            index,
            "start",
            None,
            None,
            None,
        );
        let profile_snapshot = profile_before_phase(&config.workspace_root, &plan.profile)?;
        emit_ultra_phase_context_attached(
            config,
            plan,
            phase,
            index,
            &ultra_context,
            ultra_session.messages.len(),
        );
        let phase_prompt = ultra_phase_prompt(plan, phase, config, &ultra_context);
        let step_plan = generate_step_plan_with_ui_for_phase(
            planner,
            &phase_prompt,
            config,
            ui,
            Some(&phase.id),
            preset_plan,
        )
        .map_err(|err| {
            let message = err.to_string();
            emit_ultra_phase_event(
                config,
                "ultra_phase_failed",
                plan,
                phase,
                index,
                "scaffold",
                Some(false),
                Some(&message),
                None,
            );
            emit_planner_error(
                config,
                planner.label(),
                &config.planner_model,
                "scaffold",
                "phase_scaffold_error",
                &format!("phase scaffold failed: {}", message),
                index + 1,
            );
            let handoff = save_ultra_phase_recovery_handoff(
                config,
                plan,
                phase,
                UltraPhaseRecoveryRequest {
                    failure_kind: "phase_scaffold_error",
                    reason: &message,
                    missing_paths: &missing_final_artifacts(
                        &config.workspace_root,
                        &final_expected_paths,
                    ),
                    missing_signals: &[],
                    repair_targets: &["phase_scaffold".to_string()],
                },
            );
            anyhow::anyhow!(
                "{}",
                render_failure_stop_reason(format!("phase scaffold failed: {message}"), handoff,)
            )
        })?;
        emit_ultra_phase_event(
            config,
            "ultra_phase_scaffold_complete",
            plan,
            phase,
            index,
            "scaffold",
            Some(true),
            None,
            Some(step_plan.steps.len()),
        );
        emit_ultra_phase_event(
            config,
            "ultra_phase_plan_validated",
            plan,
            phase,
            index,
            "lint",
            Some(true),
            None,
            Some(step_plan.steps.len()),
        );
        save_step_plan(&config.workspace_root, &step_plan)?;
        let final_phase = index + 1 == plan.phases.len();
        let step_outcome = match run_step_plan_with_session_with_ui_and_run_authority(
            execution,
            &mut ultra_session,
            &step_plan,
            config,
            ui,
            false,
            "ultra-plan-run",
            Some(&phase.id),
            Some(&plan.goal),
            Some(&mut setup_authority_state),
        ) {
            Ok(outcome) => outcome,
            Err(err) => {
                let mut pending_signals = ultra_context.pending_capability_evidence.clone();
                merge_unique_strings(
                    &mut pending_signals,
                    &err.partial_outcome.observed_contract_keys(),
                );
                let message =
                    exhaustion_reason_with_pending_contract_state(&err.message, &pending_signals);
                ultra_context.update_after_failure(
                    phase,
                    &err.partial_outcome,
                    missing_final_artifacts(&config.workspace_root, &final_expected_paths),
                );
                emit_ultra_phase_context_updated(
                    config,
                    plan,
                    phase,
                    index,
                    &ultra_context,
                    ultra_session.messages.len(),
                    true,
                );
                emit_ultra_phase_event(
                    config,
                    "ultra_phase_failed",
                    plan,
                    phase,
                    index,
                    "execute",
                    Some(false),
                    Some(&message),
                    None,
                );
                merge_unique_strings(
                    &mut pending_signals,
                    &ultra_context.pending_capability_evidence,
                );
                let failure_evidence = capability_evidence_failure_evidence(
                    &config.workspace_root,
                    &plan.profile,
                    &pending_signals,
                    &message,
                );
                let handoff = save_ultra_phase_recovery_handoff_with_evidence(
                    config,
                    plan,
                    phase,
                    UltraPhaseRecoveryRequest {
                        failure_kind: "phase_execute_error",
                        reason: &message,
                        missing_paths: &missing_final_artifacts(
                            &config.workspace_root,
                            &final_expected_paths,
                        ),
                        missing_signals: &pending_signals,
                        repair_targets: &err.partial_outcome.repair_targets,
                    },
                    &failure_evidence,
                );
                return Err(anyhow::anyhow!(
                    "{}",
                    render_failure_stop_reason(
                        format!("phase {} failed: {message}", phase.id),
                        handoff
                    )
                ));
            }
        };
        ultra_context.update_after_phase(
            phase,
            &step_outcome,
            missing_final_artifacts(&config.workspace_root, &final_expected_paths),
        );
        for rollback in &step_outcome.compile_rollbacks {
            emit_compile_rollback_context_carried(config, rollback);
        }
        let acceptance = ultra_contract_runtime_acceptance_report(plan, config)?;
        ultra_context.refresh_pending_capability_evidence(&acceptance);
        emit_ultra_phase_context_updated(
            config,
            plan,
            phase,
            index,
            &ultra_context,
            ultra_session.messages.len(),
            step_outcome.partial,
        );
        emit_ultra_phase_event(
            config,
            "ultra_phase_execute_complete",
            plan,
            phase,
            index,
            "execute",
            Some(true),
            None,
            None,
        );
        let mut invariant_report =
            verify_profile_invariant_with_hook_snapshot(config, plan, &profile_snapshot);
        if !final_phase && !invariant_report.is_pass() {
            invariant_report = repair_intermediate_profile_invariant(
                execution,
                &mut ultra_session,
                config,
                plan,
                phase,
                index,
                &profile_snapshot,
                &step_plan,
                &mut ultra_context,
                &final_expected_paths,
                ui,
                invariant_report,
                &mut setup_authority_state,
            )?;
        }
        if !final_phase
            && !invariant_report.is_pass()
            && !invariant_report.compile_errors.is_empty()
            && let Some(rollback) = try_compile_rollback_after_repair_exhaustion(
                config,
                &plan.profile,
                &plan.goal,
                &phase.id,
                &phase.prompt,
                &invariant_report,
                "profile_invariant_repair_exhausted",
            )?
        {
            push_context_items_capped(
                &mut ultra_context.carry_forward_guidance,
                &rollback.carry_forward_guidance,
                ULTRA_CONTEXT_MAX_MESSAGES,
                &mut ultra_context.truncated,
            );
            emit_compile_rollback_context_carried(config, &rollback);
            invariant_report =
                verify_profile_invariant_with_hook_snapshot(config, plan, &profile_snapshot);
        }
        if !invariant_report.is_pass() {
            let fresh_evidence = fresh_profile_invariant_failure_evidence(
                config,
                plan,
                &profile_snapshot,
                &final_expected_paths,
            );
            invariant_report = fresh_evidence.report.clone();
            let invariant_reason = invariant_report.primary_reason();
            let missing_paths = fresh_evidence.missing_paths.clone();
            let failure_evidence = fresh_evidence.failure_evidence.clone();
            if !final_phase {
                ultra_context.update_after_profile_failure(
                    phase,
                    &invariant_reason,
                    missing_paths.clone(),
                );
                emit_ultra_phase_context_updated(
                    config,
                    plan,
                    phase,
                    index,
                    &ultra_context,
                    ultra_session.messages.len(),
                    true,
                );
            }
            emit_ultra_phase_event(
                config,
                if final_phase {
                    "ultra_phase_profile_check"
                } else {
                    "ultra_phase_failed"
                },
                plan,
                phase,
                index,
                "profile_invariant",
                Some(false),
                Some(&invariant_reason),
                None,
            );
            emit_phase_verification_event(
                config,
                plan,
                phase,
                index,
                PhaseVerificationMode::IntermediateInvariant,
                false,
                Some(&invariant_reason),
            );
            if !final_phase {
                let handoff = save_ultra_phase_recovery_handoff_with_evidence(
                    config,
                    plan,
                    phase,
                    UltraPhaseRecoveryRequest {
                        failure_kind: "profile_invariant_failure",
                        reason: &invariant_reason,
                        missing_paths: &missing_paths,
                        missing_signals: &verification_missing_signals(&invariant_report),
                        repair_targets: &["profile_contract".to_string()],
                    },
                    &failure_evidence,
                );
                return Err(anyhow::anyhow!(
                    "{}",
                    render_failure_stop_reason(
                        format!(
                            "phase {} profile invariant verification failed: {}",
                            phase.id, invariant_reason
                        ),
                        handoff,
                    )
                ));
            }
        } else {
            hook_snapshot::save_phase_snapshots(config, &plan.profile, &plan.goal, &phase.id);
            emit_phase_verification_event(
                config,
                plan,
                phase,
                index,
                PhaseVerificationMode::IntermediateInvariant,
                true,
                None,
            );
        }
        if !final_phase {
            emit_ultra_phase_event(
                config,
                "ultra_phase_profile_check",
                plan,
                phase,
                index,
                "profile_invariant",
                Some(true),
                None,
                None,
            );
            emit_ultra_phase_event(
                config,
                "ultra_phase_complete",
                plan,
                phase,
                index,
                "complete",
                Some(true),
                None,
                None,
            );
            bounded_process::reap_registered_server_children_for_workspace(
                config.eval_events_path.as_deref(),
                "phase_transition",
                &config.workspace_root,
            );
            reconcile_manifest_changed_dependencies_if_needed(
                config,
                &plan.profile,
                &mut setup_authority_state,
            )?;
            if try_promote_profile_at_phase_boundary(
                config,
                plan,
                &mut ultra_context,
                &mut final_expected_paths,
                &mut promotion_state,
                phase,
                index,
            )?
            .is_some()
            {
                setup_authority_state.grant("profile_promotion");
                reconcile_run_dependency_setup(
                    config,
                    &plan.profile,
                    DependencyReconciliationTrigger::Promotion,
                    &setup_authority_state,
                )?;
            }
            continue;
        }
        let final_invariant_reason =
            (!invariant_report.is_pass()).then(|| invariant_report.primary_reason());
        if invariant_report.is_pass() {
            hook_snapshot::save_phase_snapshots(config, &plan.profile, &plan.goal, &phase.id);
        }
        emit_phase_verification_event(
            config,
            plan,
            phase,
            index,
            PhaseVerificationMode::FinalAcceptance,
            invariant_report.is_pass(),
            final_invariant_reason.as_deref(),
        );
        emit_ultra_phase_event(
            config,
            "ultra_phase_profile_check",
            plan,
            phase,
            index,
            "profile_observed",
            Some(invariant_report.is_pass()),
            final_invariant_reason.as_deref(),
            None,
        );
        emit_ultra_phase_event(
            config,
            "ultra_phase_complete",
            plan,
            phase,
            index,
            "complete",
            Some(true),
            None,
            None,
        );
        bounded_process::reap_registered_server_children_for_workspace(
            config.eval_events_path.as_deref(),
            "phase_transition",
            &config.workspace_root,
        );
        reconcile_manifest_changed_dependencies_if_needed(
            config,
            &plan.profile,
            &mut setup_authority_state,
        )?;
        if try_promote_profile_at_phase_boundary(
            config,
            plan,
            &mut ultra_context,
            &mut final_expected_paths,
            &mut promotion_state,
            phase,
            index,
        )?
        .is_some()
        {
            setup_authority_state.grant("profile_promotion");
            reconcile_run_dependency_setup(
                config,
                &plan.profile,
                DependencyReconciliationTrigger::Promotion,
                &setup_authority_state,
            )?;
        }
    }
    let mut final_acceptance_cycle_deltas = Vec::new();
    let (mut acceptance_report, mut deterministic_remedies_applied) =
        ultra_final_acceptance_report_with_deterministic_remedies(
            plan,
            config,
            0,
            &mut setup_authority_state,
        )?;
    if !acceptance_report.is_pass() {
        let initial_reason = acceptance_report.primary_reason();
        let initial_target = classify_repair_target(&acceptance_report);
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "ultra_final_acceptance_failed",
                "cycle_index": 0,
                "lifecycle_stage": "final_acceptance",
                "primary_reason": eval_events::body_snippet(&initial_reason),
                "repair_target": initial_target.as_str(),
                "missing_paths": acceptance_report.missing_paths.clone(),
                "compile_errors": acceptance_report.compile_errors.clone(),
                "profile_failures": acceptance_report.profile_failures.clone(),
                "deterministic_remedies_applied": deterministic_remedies_applied.clone(),
                "bounded_repair_available": true,
                "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
            }),
        );
        let fallback_phase = plan.phases.last().cloned().unwrap_or_else(|| UltraPhase {
            id: "final".to_string(),
            prompt: "Final acceptance".to_string(),
        });
        let repair_config = capped_config(config, STEP_REPAIR_MAX_ITERATIONS);
        let escalation_carryover = EscalationCarryoverHandle::new();
        let repair_started = Instant::now();
        let mut attempts_run = 0;
        let mut exhausted_reason = "bounded_repair_exhausted".to_string();
        let mut compile_no_source_change_count = 0usize;
        let mut evidence_no_source_change_count = 0usize;
        let mut evidence_regeneration_decision_emitted = false;
        let mut compile_no_snapshot_narrow_retry_used = false;
        let mut max_repair_attempts = FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS;
        let mut hook_snapshot_feedback_given = false;
        let mut hook_snapshot_restore_used = false;
        for attempt in 1..=FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS
            + FINAL_ACCEPTANCE_COMPILE_NO_SNAPSHOT_EXTRA_ATTEMPTS
                .max(FINAL_ACCEPTANCE_EVIDENCE_NO_CHANGE_EXTRA_ATTEMPTS)
        {
            if attempt > max_repair_attempts {
                break;
            }
            if repair_started.elapsed() > FINAL_ACCEPTANCE_REPAIR_WALL_CLOCK_CAP {
                exhausted_reason = "bounded_repair_wall_clock_cap".to_string();
                break;
            }
            attempts_run = attempt;
            let repair_target = classify_repair_target(&acceptance_report);
            let mut expected_paths =
                final_acceptance_repair_expected_paths(plan, config, &acceptance_report)?;
            let pending_repair_evidence = escalation_carryover.carry_pending_evidence(
                final_acceptance_repair_signals(&acceptance_report),
                &ultra_context.pending_capability_evidence,
            );
            let contract_attribute_paths = contract_attribute_repair_target_paths(
                &config.workspace_root,
                &plan.profile,
                &acceptance_report,
            );
            let diagnosis =
                crate::planner::state_binding_scan::final_acceptance_actionable_diagnosis(
                    &config.workspace_root,
                    &plan.profile,
                    &acceptance_report,
                );
            let target_selection =
                crate::planner::repair_targeting::resolve_final_acceptance_repair_targets(
                    crate::planner::repair_targeting::FinalAcceptanceRepairTargetInput {
                        root: &config.workspace_root,
                        profile: &plan.profile,
                        pending_evidence: &pending_repair_evidence,
                        contract_attribute_paths: &contract_attribute_paths,
                        repair_changed_paths: &ultra_context.last_repair_changed_paths,
                        required_paths: &expected_paths,
                        diagnosis_path: diagnosis.as_ref().map(|diagnosis| diagnosis.path.as_str()),
                    },
                );
            merge_unique_strings(&mut expected_paths, &target_selection.selected_targets);
            let repair_required_paths = if target_selection.selected_targets.is_empty() {
                expected_paths.clone()
            } else {
                target_selection.selected_targets.clone()
            };
            let evidence_zero_edit_eligible =
                evidence_repair_zero_edit_eligible(&acceptance_report, repair_target);
            if evidence_zero_edit_eligible
                && evidence_no_source_change_count >= 3
                && let Some(target_path) = final_acceptance_evidence_regeneration_target(
                    &config.workspace_root,
                    &plan.profile,
                    &acceptance_report,
                    &expected_paths,
                )
            {
                let Some(target_abs) =
                    writable_workspace_source_path(&config.workspace_root, &target_path)
                else {
                    emit_evidence_regeneration_event(
                        config,
                        false,
                        false,
                        Some(&target_path),
                        &verification_missing_signals(&acceptance_report),
                        &verification_missing_signals(&acceptance_report),
                        &[],
                        "target_path_rejected",
                    );
                    evidence_regeneration_decision_emitted = true;
                    break;
                };
                let before_content = match std::fs::read(&target_abs) {
                    Ok(content) => content,
                    Err(err) => {
                        emit_evidence_regeneration_event(
                            config,
                            false,
                            false,
                            Some(&target_path),
                            &verification_missing_signals(&acceptance_report),
                            &verification_missing_signals(&acceptance_report),
                            &[],
                            &format!("snapshot_read_error:{err}"),
                        );
                        evidence_regeneration_decision_emitted = true;
                        break;
                    }
                };
                let before_keys = verification_missing_signals(&acceptance_report);
                let regeneration_prompt = build_final_acceptance_evidence_regeneration_prompt(
                    &config.workspace_root,
                    plan,
                    &acceptance_report,
                    &target_path,
                );
                let mut regeneration_session = SessionSnapshot::new();
                let regeneration = run_final_acceptance_repair_with_carryover(
                    execution,
                    &mut regeneration_session,
                    &regeneration_prompt,
                    std::slice::from_ref(&target_path),
                    &repair_config,
                    ui,
                    escalation_carryover.clone(),
                );
                let regeneration = match regeneration {
                    Ok(regeneration) => regeneration,
                    Err(err) => {
                        let _ = std::fs::write(&target_abs, &before_content);
                        emit_evidence_regeneration_event(
                            config,
                            true,
                            false,
                            Some(&target_path),
                            &before_keys,
                            &before_keys,
                            &[],
                            &format!(
                                "regeneration_turn_error:{}",
                                eval_events::body_snippet(&err.to_string())
                            ),
                        );
                        evidence_regeneration_decision_emitted = true;
                        exhausted_reason = "evidence_repair_no_source_change".to_string();
                        break;
                    }
                };
                clear_final_acceptance_browser_probe_evidence(config);
                let (regenerated_report, regenerated_deterministic_remedies) =
                    ultra_final_acceptance_report_with_deterministic_remedies(
                        plan,
                        config,
                        attempt,
                        &mut setup_authority_state,
                    )?;
                let after_keys = verification_missing_signals(&regenerated_report);
                let resolved = resolved_missing_signals(&before_keys, &after_keys);
                let mut regeneration_changed_paths = regeneration.changed_paths.clone();
                regeneration_changed_paths.sort();
                regeneration_changed_paths.dedup();
                let accepted = !resolved.is_empty()
                    && regenerated_report.compile_errors.is_empty()
                    && regenerated_report.dependency_missing.is_empty();
                emit_evidence_regeneration_event(
                    config,
                    true,
                    accepted,
                    Some(&target_path),
                    &before_keys,
                    &after_keys,
                    &regeneration_changed_paths,
                    if accepted {
                        "accepted"
                    } else if resolved.is_empty() {
                        "missing_evidence_not_reduced"
                    } else {
                        "build_or_dependency_regressed"
                    },
                );
                evidence_regeneration_decision_emitted = true;
                if accepted {
                    push_context_items_capped(
                        &mut ultra_context.created_or_changed_paths,
                        &regeneration_changed_paths,
                        ULTRA_CONTEXT_MAX_PATHS,
                        &mut ultra_context.truncated,
                    );
                    push_context_items_capped(
                        &mut ultra_context.last_repair_changed_paths,
                        &regeneration_changed_paths,
                        ULTRA_CONTEXT_MAX_PATHS,
                        &mut ultra_context.truncated,
                    );
                    deterministic_remedies_applied = regenerated_deterministic_remedies;
                    acceptance_report = regenerated_report;
                    if acceptance_report.is_pass() {
                        break;
                    }
                    continue;
                }
                let _ = std::fs::write(&target_abs, &before_content);
                exhausted_reason = "evidence_repair_no_source_change".to_string();
                break;
            }
            let compile_reanchored_retry =
                compile_no_source_change_count > 0 && !acceptance_report.compile_errors.is_empty();
            let (repair_session_mode, evidence_reanchored_retry, evidence_compact_retry) =
                evidence_repair_retry_mode(
                    evidence_zero_edit_eligible,
                    evidence_no_source_change_count,
                );
            let compile_narrow_no_snapshot_retry = compile_no_snapshot_narrow_retry_used
                && compile_no_source_change_count >= FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS
                && !acceptance_report.compile_errors.is_empty();
            let before_missing_keys = verification_missing_signals(&acceptance_report);
            let before_route_bound_paths =
                route_bound_source_paths(&config.workspace_root, &plan.profile);
            let before_source_snapshot = final_acceptance_source_snapshot(
                &config.workspace_root,
                &plan.profile,
                &plan.goal,
                &expected_paths,
                &[],
            );
            let mut repair_prompt = final_acceptance_repair_prompt_with_events(
                &config.workspace_root,
                config.prompt_layout,
                plan,
                &acceptance_report,
                &ultra_context,
                repair_target.as_str(),
                &expected_paths,
                &plan_adherence_report(plan, &config.workspace_root).missing,
                (attempt, FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS),
                compile_reanchored_retry,
                compile_narrow_no_snapshot_retry,
                config.eval_events_path.as_deref(),
            );
            if evidence_reanchored_retry {
                repair_prompt = format!(
                    "Evidence repair re-anchor mandate: the previous repair turn made no source changes. Inspection is complete; make a concrete Write/Edit change for the pending evidence keys now.\n\n{repair_prompt}"
                );
            } else if evidence_compact_retry {
                repair_prompt = format!(
                    "Repair session mode: compact.\nEvidence repair compact mandate: use a fresh, minimal context and make the concrete Write/Edit change for the pending evidence keys. Do not inspect only.\n\n{repair_prompt}"
                );
            }
            repair_prompt = hook_snapshot::prefix_feedback_if_missing(
                config,
                &plan.profile,
                &plan.goal,
                "final_acceptance_repair",
                Some(&fallback_phase.id),
                &mut hook_snapshot_feedback_given,
                repair_prompt,
            );
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "final_acceptance_repair_start",
                    "cycle_index": attempt,
                    "lifecycle_stage": "final_acceptance_repair",
                    "attempt": attempt,
                    "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                    "repair_target": repair_target.as_str(),
                    "missing_paths": acceptance_report.missing_paths.clone(),
                    "compile_errors": acceptance_report.compile_errors.clone(),
                    "profile_failures": acceptance_report.profile_failures.clone(),
                    "deterministic_remedies_applied": deterministic_remedies_applied.clone(),
                    "selected_evidence_keys": pending_repair_evidence.clone(),
                    "selected_target": target_selection.primary_target().unwrap_or_default(),
                    "selected_targets": target_selection.selected_targets.clone(),
                    "selection_reason": target_selection.selection_reason.clone(),
                    "selected_interaction_failure": final_acceptance_app_behavior_failure_kind(&acceptance_report)
                        .unwrap_or_default(),
                    "bounded_repair": true,
                    "max_iterations": repair_config.max_iterations,
                    "shared_execution_session": true,
                    "compile_reanchored_retry": compile_reanchored_retry,
                    "evidence_reanchored_retry": evidence_reanchored_retry,
                    "repair_session_mode": repair_session_mode,
                    "compile_narrow_no_snapshot_retry": compile_narrow_no_snapshot_retry,
                    "session_message_count": ultra_session.messages.len(),
                }),
            );
            let repair_outcome = match if evidence_compact_retry {
                let mut compact_session = SessionSnapshot::new();
                run_final_acceptance_repair_with_carryover(
                    execution,
                    &mut compact_session,
                    &repair_prompt,
                    &repair_required_paths,
                    &repair_config,
                    ui,
                    escalation_carryover.clone(),
                )
            } else {
                run_final_acceptance_repair_with_carryover(
                    execution,
                    &mut ultra_session,
                    &repair_prompt,
                    &repair_required_paths,
                    &repair_config,
                    ui,
                    escalation_carryover.clone(),
                )
            } {
                Ok(outcome) => outcome,
                Err(err) => {
                    let err_text = err.to_string();
                    if !acceptance_report.compile_errors.is_empty()
                        && err_text.contains("missing tool call for action prompt")
                    {
                        compile_no_source_change_count += 1;
                        exhausted_reason = "compile_repair_no_source_change".to_string();
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "final_acceptance_repair_no_source_change",
                                "cycle_index": attempt,
                                "lifecycle_stage": "final_acceptance_repair",
                                "attempt": attempt,
                                "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                                "failure_kind": "compile_repair_no_source_change",
                                "repair_target": repair_target.as_str(),
                                "compile_errors": acceptance_report.compile_errors.clone(),
                                "repair_error": eval_events::body_snippet(&err_text),
                                "reanchored_retry": compile_no_source_change_count == 1,
                                "narrow_no_snapshot_retry": compile_narrow_no_snapshot_retry,
                                "proceed_to_rollback": compile_no_source_change_count >= 2,
                            }),
                        );
                        if compile_no_source_change_count >= 2 {
                            if should_run_compile_no_snapshot_narrow_retry(
                                config,
                                &acceptance_report,
                                compile_no_snapshot_narrow_retry_used,
                            ) {
                                compile_no_snapshot_narrow_retry_used = true;
                                max_repair_attempts = FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS
                                    + FINAL_ACCEPTANCE_COMPILE_NO_SNAPSHOT_EXTRA_ATTEMPTS;
                                exhausted_reason =
                                    "compile_repair_no_snapshot_narrow_retry".to_string();
                                emit_compile_no_snapshot_narrow_retry(
                                    config,
                                    attempt,
                                    &acceptance_report,
                                    "repair_error",
                                );
                                continue;
                            }
                            break;
                        }
                        continue;
                    }
                    if acceptance_report.compile_errors.is_empty()
                        && evidence_zero_edit_eligible
                        && err_text.contains("missing tool call for action prompt")
                    {
                        evidence_no_source_change_count += 1;
                        exhausted_reason = "evidence_repair_no_source_change".to_string();
                        max_repair_attempts = FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS
                            + FINAL_ACCEPTANCE_EVIDENCE_NO_CHANGE_EXTRA_ATTEMPTS;
                        eval_events::emit(
                            config.eval_events_path.as_deref(),
                            json!({
                                "event": "final_acceptance_repair_no_source_change",
                                "cycle_index": attempt,
                                "lifecycle_stage": "final_acceptance_repair",
                                "attempt": attempt,
                                "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                                "failure_kind": "evidence_repair_no_source_change",
                                "repair_target": repair_target.as_str(),
                                "repair_error": eval_events::body_snippet(&err_text),
                                "repair_session_mode": repair_session_mode,
                                "evidence_no_source_change_count": evidence_no_source_change_count,
                                "reanchored_retry": evidence_reanchored_retry,
                                "compact_retry": evidence_compact_retry,
                                "proceed_to_regeneration": evidence_no_source_change_count >= 3,
                            }),
                        );
                        continue;
                    }
                    eval_events::emit(
                        config.eval_events_path.as_deref(),
                        json!({
                            "event": "final_acceptance_repair_failed",
                            "cycle_index": attempt,
                            "lifecycle_stage": "final_acceptance_repair",
                            "attempt": attempt,
                            "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                            "repair_target": repair_target.as_str(),
                            "reason": eval_events::body_snippet(&err_text),
                            "bounded_repair_exhausted": true,
                        }),
                    );
                    let repair_targets =
                        final_acceptance_recovery_repair_targets(&acceptance_report, repair_target);
                    let missing_signals = verification_missing_signals(&acceptance_report);
                    let failure_evidence = final_acceptance_recovery_failure_evidence(
                        &plan.profile,
                        &plan.goal,
                        &acceptance_report,
                        &err_text,
                    );
                    let handoff = save_ultra_phase_recovery_handoff_with_evidence(
                        config,
                        plan,
                        &fallback_phase,
                        UltraPhaseRecoveryRequest {
                            failure_kind: "final_acceptance_repair_failed",
                            reason: &err_text,
                            missing_paths: &acceptance_report.missing_paths,
                            missing_signals: &missing_signals,
                            repair_targets: &repair_targets,
                        },
                        &failure_evidence,
                    );
                    anyhow::bail!(
                        "{}",
                        render_failure_stop_reason(
                            format!("ultra final acceptance repair failed: {err_text}"),
                            handoff,
                        )
                    );
                }
            };
            push_context_items_capped(
                &mut ultra_context.created_or_changed_paths,
                &repair_outcome.changed_paths,
                ULTRA_CONTEXT_MAX_PATHS,
                &mut ultra_context.truncated,
            );
            push_context_items_capped(
                &mut ultra_context.last_repair_changed_paths,
                &repair_outcome.changed_paths,
                ULTRA_CONTEXT_MAX_PATHS,
                &mut ultra_context.truncated,
            );
            let after_route_bound_paths =
                route_bound_source_paths(&config.workspace_root, &plan.profile);
            let after_source_snapshot = final_acceptance_source_snapshot(
                &config.workspace_root,
                &plan.profile,
                &plan.goal,
                &expected_paths,
                &repair_outcome.changed_paths,
            );
            let actual_changed_paths =
                changed_snapshot_paths(&before_source_snapshot, &after_source_snapshot);
            let route_bound_changed_paths = route_bound_changed_paths(
                &before_source_snapshot,
                &after_source_snapshot,
                &before_route_bound_paths,
                &after_route_bound_paths,
            );
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "final_acceptance_repair_complete",
                    "cycle_index": attempt,
                    "lifecycle_stage": "final_acceptance_repair",
                    "attempt": attempt,
                    "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                    "repair_target": repair_target.as_str(),
                    "changed_path_count": actual_changed_paths.len(),
                    "reported_changed_path_count": repair_outcome.changed_paths.len(),
                    "changed_paths": actual_changed_paths.clone(),
                    "reported_changed_paths": repair_outcome.changed_paths.clone(),
                    "route_bound_changed_paths": route_bound_changed_paths.clone(),
                    "route_bound_source_changed": !route_bound_changed_paths.is_empty(),
                    "iterations": repair_outcome.iterations,
                    "tool_calls": repair_outcome.tool_calls,
                    "shared_execution_session": true,
                    "repair_session_mode": repair_session_mode,
                    "session_message_count": ultra_session.messages.len(),
                }),
            );
            if !acceptance_report.compile_errors.is_empty() && actual_changed_paths.is_empty() {
                compile_no_source_change_count += 1;
                exhausted_reason = "compile_repair_no_source_change".to_string();
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "final_acceptance_repair_no_source_change",
                        "cycle_index": attempt,
                        "lifecycle_stage": "final_acceptance_repair",
                        "attempt": attempt,
                        "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                        "failure_kind": "compile_repair_no_source_change",
                        "repair_target": repair_target.as_str(),
                        "compile_errors": acceptance_report.compile_errors.clone(),
                        "reanchored_retry": compile_no_source_change_count == 1,
                        "narrow_no_snapshot_retry": compile_narrow_no_snapshot_retry,
                        "proceed_to_rollback": compile_no_source_change_count >= 2,
                    }),
                );
                if compile_no_source_change_count >= 2 {
                    if should_run_compile_no_snapshot_narrow_retry(
                        config,
                        &acceptance_report,
                        compile_no_snapshot_narrow_retry_used,
                    ) {
                        compile_no_snapshot_narrow_retry_used = true;
                        max_repair_attempts = FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS
                            + FINAL_ACCEPTANCE_COMPILE_NO_SNAPSHOT_EXTRA_ATTEMPTS;
                        exhausted_reason = "compile_repair_no_snapshot_narrow_retry".to_string();
                        emit_compile_no_snapshot_narrow_retry(
                            config,
                            attempt,
                            &acceptance_report,
                            "no_source_change",
                        );
                        continue;
                    }
                    break;
                }
                continue;
            }
            if hook_snapshot_feedback_given && !hook_snapshot_restore_used {
                match hook_snapshot::restore_first_missing(config, &plan.profile, &plan.goal) {
                    Ok(Some(restored)) => {
                        hook_snapshot_restore_used = true;
                        push_context_items_capped(
                            &mut ultra_context.created_or_changed_paths,
                            std::slice::from_ref(&restored.restored_path),
                            ULTRA_CONTEXT_MAX_PATHS,
                            &mut ultra_context.truncated,
                        );
                        push_context_items_capped(
                            &mut ultra_context.last_repair_changed_paths,
                            std::slice::from_ref(&restored.restored_path),
                            ULTRA_CONTEXT_MAX_PATHS,
                            &mut ultra_context.truncated,
                        );
                        clear_final_acceptance_browser_probe_evidence(config);
                        (acceptance_report, deterministic_remedies_applied) =
                            ultra_final_acceptance_report_with_deterministic_remedies(
                                plan,
                                config,
                                attempt,
                                &mut setup_authority_state,
                            )?;
                        let remaining_keys = verification_missing_signals(&acceptance_report);
                        let mut restored_changed_paths = actual_changed_paths.clone();
                        merge_unique_strings(
                            &mut restored_changed_paths,
                            std::slice::from_ref(&restored.restored_path),
                        );
                        let mut restored_route_bound_paths = route_bound_changed_paths.clone();
                        merge_unique_strings(
                            &mut restored_route_bound_paths,
                            std::slice::from_ref(&restored.restored_path),
                        );
                        let delta = FinalAcceptanceCycleDelta {
                            cycle_index: attempt,
                            resolved_keys: resolved_missing_signals(
                                &before_missing_keys,
                                &remaining_keys,
                            ),
                            remaining_keys,
                            changed_paths: restored_changed_paths,
                            route_bound_changed_paths: restored_route_bound_paths,
                        };
                        emit_final_acceptance_cycle_delta(
                            config,
                            &delta,
                            acceptance_report.is_pass(),
                        );
                        final_acceptance_cycle_deltas.push(delta);
                        if acceptance_report.is_pass() {
                            break;
                        }
                        continue;
                    }
                    Ok(None) => {}
                    Err(err) => {
                        exhausted_reason = format!("hook_snapshot_restore_error:{err}");
                    }
                }
            }
            compile_no_source_change_count = 0;
            if actual_changed_paths.is_empty() {
                if evidence_zero_edit_eligible {
                    evidence_no_source_change_count += 1;
                    exhausted_reason = "evidence_repair_no_source_change".to_string();
                    max_repair_attempts = FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS
                        + FINAL_ACCEPTANCE_EVIDENCE_NO_CHANGE_EXTRA_ATTEMPTS;
                } else {
                    exhausted_reason = "final_acceptance_repair_no_source_change".to_string();
                }
                eval_events::emit(
                    config.eval_events_path.as_deref(),
                    json!({
                        "event": "final_acceptance_repair_no_source_change",
                        "cycle_index": attempt,
                        "lifecycle_stage": "final_acceptance_repair",
                        "attempt": attempt,
                        "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                        "failure_kind": if evidence_zero_edit_eligible { "evidence_repair_no_source_change" } else { "final_acceptance_repair_no_source_change" },
                        "repair_target": repair_target.as_str(),
                        "reported_changed_paths": repair_outcome.changed_paths.clone(),
                        "route_bound_changed_paths": route_bound_changed_paths.clone(),
                        "repair_session_mode": repair_session_mode,
                        "evidence_no_source_change_count": evidence_no_source_change_count,
                        "reprobe_skipped": true,
                    }),
                );
                if evidence_zero_edit_eligible {
                    continue;
                }
                break;
            }
            evidence_no_source_change_count = 0;
            clear_final_acceptance_browser_probe_evidence(config);
            (acceptance_report, deterministic_remedies_applied) =
                ultra_final_acceptance_report_with_deterministic_remedies(
                    plan,
                    config,
                    attempt,
                    &mut setup_authority_state,
                )?;
            let remaining_keys = verification_missing_signals(&acceptance_report);
            let delta = FinalAcceptanceCycleDelta {
                cycle_index: attempt,
                resolved_keys: resolved_missing_signals(&before_missing_keys, &remaining_keys),
                remaining_keys,
                changed_paths: actual_changed_paths,
                route_bound_changed_paths,
            };
            emit_final_acceptance_cycle_delta(config, &delta, acceptance_report.is_pass());
            final_acceptance_cycle_deltas.push(delta);
            if acceptance_report.is_pass() {
                break;
            }
        }
        if !acceptance_report.is_pass() && !acceptance_report.compile_errors.is_empty() {
            let expected_paths =
                final_acceptance_repair_expected_paths(plan, config, &acceptance_report)?;
            if exhausted_reason == "bounded_repair_wall_clock_cap" {
                emit_compile_regeneration_event(
                    config,
                    None,
                    "final_acceptance_repair",
                    false,
                    false,
                    0,
                    None,
                    "bounded_repair_wall_clock_cap",
                    acceptance_report.compile_errors.len(),
                    acceptance_report.compile_errors.len(),
                    &[],
                );
            } else {
                let _accepted_compile_regeneration = try_final_acceptance_compile_regeneration(
                    execution,
                    config,
                    plan,
                    &mut ultra_context,
                    &mut acceptance_report,
                    &mut deterministic_remedies_applied,
                    &mut setup_authority_state,
                    attempts_run,
                    &expected_paths,
                    &repair_config,
                    ui,
                )?;
            }
        }
        if !acceptance_report.is_pass()
            && !acceptance_report.compile_errors.is_empty()
            && let Some(rollback) = try_compile_rollback_after_repair_exhaustion(
                config,
                &plan.profile,
                &plan.goal,
                &fallback_phase.id,
                &fallback_phase.prompt,
                &acceptance_report,
                &exhausted_reason,
            )?
        {
            push_context_items_capped(
                &mut ultra_context.carry_forward_guidance,
                &rollback.carry_forward_guidance,
                ULTRA_CONTEXT_MAX_MESSAGES,
                &mut ultra_context.truncated,
            );
            emit_compile_rollback_context_carried(config, &rollback);
            exhausted_reason = "compile_rollback_applied".to_string();
            (acceptance_report, deterministic_remedies_applied) =
                ultra_final_acceptance_report_with_deterministic_remedies(
                    plan,
                    config,
                    final_acceptance_cycle_deltas
                        .last()
                        .map(|delta| delta.cycle_index)
                        .unwrap_or(0),
                    &mut setup_authority_state,
                )?;
        }
        if !acceptance_report.is_pass() {
            let target = classify_repair_target(&acceptance_report);
            let behavior_failure = final_acceptance_app_behavior_failure_kind(&acceptance_report);
            let report_reason = acceptance_report.primary_reason();
            let base_reason = behavior_failure
                .clone()
                .unwrap_or_else(|| report_reason.clone());
            let missing_signals = verification_missing_signals(&acceptance_report);
            let pending_reason = capability_evidence_unresolved_reason(&missing_signals);
            let exhausted_reason_for_event = pending_reason
                .clone()
                .unwrap_or_else(|| exhausted_reason.clone());
            if acceptance_report.compile_errors.is_empty()
                && !missing_signals.is_empty()
                && !evidence_regeneration_decision_emitted
            {
                let target_path =
                    final_acceptance_repair_expected_paths(plan, config, &acceptance_report)
                        .ok()
                        .and_then(|paths| {
                            final_acceptance_evidence_regeneration_target(
                                &config.workspace_root,
                                &plan.profile,
                                &acceptance_report,
                                &paths,
                            )
                        });
                emit_evidence_regeneration_event(
                    config,
                    false,
                    false,
                    target_path.as_deref(),
                    &missing_signals,
                    &missing_signals,
                    &[],
                    &exhausted_reason_for_event,
                );
            }
            let reason = pending_reason
                .as_ref()
                .map(|pending| format!("{pending}; {base_reason}"))
                .unwrap_or(base_reason);
            let failure_kind = if let Some(pending) = pending_reason.clone() {
                pending
            } else if !acceptance_report.compile_errors.is_empty() {
                "implementation_compile_error".to_string()
            } else {
                behavior_failure
                    .clone()
                    .unwrap_or_else(|| "final_acceptance_repair_exhausted".to_string())
            };
            let handoff_reason = final_acceptance_recovery_reason(
                &plan.profile,
                &plan.goal,
                &acceptance_report,
                &reason,
                &exhausted_reason_for_event,
            );
            let repair_targets =
                final_acceptance_recovery_repair_targets(&acceptance_report, target);
            let failure_evidence = final_acceptance_recovery_failure_evidence_with_context(
                &config.workspace_root,
                &plan.profile,
                &plan.goal,
                &acceptance_report,
                &handoff_reason,
            );
            eval_events::emit(
                config.eval_events_path.as_deref(),
                json!({
                    "event": "final_acceptance_repair_exhausted",
                    "cycle_index": attempts_run,
                    "lifecycle_stage": "final_acceptance_repair",
                    "attempt": attempts_run,
                    "max_attempts": FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS,
                    "repair_target": target.as_str(),
                    "primary_reason": eval_events::body_snippet(&reason),
                    "failure_kind": failure_kind.clone(),
                    "missing_paths": acceptance_report.missing_paths.clone(),
                    "compile_errors": acceptance_report.compile_errors.clone(),
                    "profile_failures": acceptance_report.profile_failures.clone(),
                    "pending_capability_evidence": missing_signals.clone(),
                    "pending_capability_evidence_count": missing_signals.len(),
                    "capability_evidence_remedies": capability_evidence_remedy_lines(&missing_signals),
                    "restart_hook_attachment_guidance": restart_hook_attachment_guidance(&config.workspace_root, &plan.profile),
                    "deterministic_remedies_applied": deterministic_remedies_applied.clone(),
                    "bounded_repair_exhausted": true,
                    "exhausted_reason": exhausted_reason_for_event.clone(),
                }),
            );
            append_final_acceptance_cycle_summary(config, &final_acceptance_cycle_deltas);
            let handoff = save_ultra_phase_recovery_handoff_with_evidence(
                config,
                plan,
                &fallback_phase,
                UltraPhaseRecoveryRequest {
                    failure_kind: &failure_kind,
                    reason: &handoff_reason,
                    missing_paths: &acceptance_report.missing_paths,
                    missing_signals: &missing_signals,
                    repair_targets: &repair_targets,
                },
                &failure_evidence,
            );
            anyhow::bail!(
                "{}",
                render_failure_stop_reason(
                    format!("ultra final acceptance failed after bounded repair: {reason}"),
                    handoff,
                )
            );
        }
    }
    append_final_acceptance_cycle_summary(config, &final_acceptance_cycle_deltas);
    let terminal_capabilities = inferred_required_capabilities(&plan.profile, &plan.goal);
    let (assurance_level, assurance_reason) =
        assurance_for_completion(&plan.profile, &terminal_capabilities);
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_plan_complete",
            "total_phases": plan.phases.len(),
            "profile": plan.profile,
            "assurance_level": assurance_level,
            "assurance_reason": assurance_reason,
            "ok": true,
        }),
    );
    Ok(format!(
        "ultra-plan-run complete: {} phases",
        plan.phases.len()
    ))
}
