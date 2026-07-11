use super::*;

pub(super) const FINAL_ACCEPTANCE_REPAIR_MAX_ATTEMPTS: usize = 2;

pub(super) const FINAL_ACCEPTANCE_COMPILE_NO_SNAPSHOT_EXTRA_ATTEMPTS: usize = 1;

pub(super) const FINAL_ACCEPTANCE_EVIDENCE_NO_CHANGE_EXTRA_ATTEMPTS: usize = 2;

pub(super) const FINAL_ACCEPTANCE_REPAIR_WALL_CLOCK_CAP: Duration = Duration::from_secs(240);

thread_local! {
    static FINAL_ACCEPTANCE_CYCLE_INDEX: Cell<usize> = const { Cell::new(0) };
}

pub(super) fn current_final_acceptance_cycle_index() -> usize {
    FINAL_ACCEPTANCE_CYCLE_INDEX.with(Cell::get)
}

pub(super) fn with_final_acceptance_cycle<T>(cycle_index: usize, f: impl FnOnce() -> T) -> T {
    FINAL_ACCEPTANCE_CYCLE_INDEX.with(|cell| {
        let previous = cell.replace(cycle_index);
        let result = f();
        cell.set(previous);
        result
    })
}

#[derive(Debug, Clone, Default)]
pub(super) struct FinalAcceptanceCycleDelta {
    pub(super) cycle_index: usize,
    pub(super) resolved_keys: Vec<String>,
    pub(super) remaining_keys: Vec<String>,
    pub(super) changed_paths: Vec<String>,
    pub(super) route_bound_changed_paths: Vec<String>,
}

pub(super) fn contract_origin_for_acceptance(config: &Config) -> &'static str {
    if config
        .eval_events_path
        .as_deref()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|text| {
            text.lines()
                .any(|line| line.contains(r#""event":"profile_reinferred""#))
        })
    {
        "promoted_union"
    } else {
        "initial"
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn bind_completion_contract_for_acceptance(
    config: &Config,
    scope: &str,
    profile: &str,
    goal: &str,
    required_paths: &[String],
    required_capabilities: &[String],
    required_evidence: &[String],
    required_obligations: &[String],
) -> anyhow::Result<Option<BoundCompletionContract>> {
    let required = completion_contract_required(profile, goal, required_capabilities);
    if let Some(contract) = CompletionContract::load_for_config(config)? {
        let mut contract = contract;
        contract.merge_evidence_hint_tokens_from_goal(goal);
        let path = explicit_completion_contract_path(config)
            .map(|path| display_path_for_event(&config.workspace_root, &path))
            .unwrap_or_else(|| "<inline-config>".to_string());
        let fs_path = explicit_completion_contract_path(config);
        let bound = BoundCompletionContract {
            contract,
            path,
            fs_path,
            generated: false,
            required,
        };
        emit_completion_contract_bound(config, scope, profile, goal, &bound);
        return Ok(Some(bound));
    }
    if !required {
        return Ok(None);
    }
    let contract = CompletionContract {
        required_paths: required_paths.to_vec(),
        verify_commands: Vec::new(),
        profile: None,
        goal: Some(goal.to_string()),
        required_capabilities: required_capabilities.to_vec(),
        deterministic_oracles: Vec::new(),
        required_evidence: required_evidence.to_vec(),
        evidence_hint_tokens: evidence_hint_tokens_for_goal(goal),
        required_obligations: required_obligations.to_vec(),
        deferred_verify_requirements: Vec::new(),
        verify_repair_cap: 2,
    }
    .validate(&config.workspace_root)?;
    let path = generated_completion_contract_path(config, scope);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&contract)?;
    std::fs::write(&path, format!("{text}\n"))?;
    let bound = BoundCompletionContract {
        contract,
        path: display_path_for_event(&config.workspace_root, &path),
        fs_path: Some(path),
        generated: true,
        required,
    };
    emit_completion_contract_bound(config, scope, profile, goal, &bound);
    Ok(Some(bound))
}

pub(super) fn runtime_missing_signals(report: &RuntimeAcceptanceReport) -> Vec<String> {
    let mut out = Vec::new();
    merge_unique_strings(&mut out, &report.missing_capabilities);
    merge_unique_strings(&mut out, &report.missing_evidence);
    merge_unique_strings(&mut out, &report.missing_obligations);
    merge_unique_strings(&mut out, &report.diagnostics);
    out
}

pub(super) fn resolved_missing_signals(before: &[String], after: &[String]) -> Vec<String> {
    let after = after.iter().collect::<BTreeSet<_>>();
    before
        .iter()
        .filter(|key| !after.contains(*key))
        .cloned()
        .collect()
}

pub(super) fn emit_final_acceptance_cycle_delta(
    config: &Config,
    delta: &FinalAcceptanceCycleDelta,
    passed: bool,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "final_acceptance_cycle_complete",
            "cycle_index": delta.cycle_index,
            "ok": passed,
            "resolved_keys": delta.resolved_keys.clone(),
            "remaining_keys": delta.remaining_keys.clone(),
            "changed_paths": delta.changed_paths.clone(),
            "route_bound_changed_paths": delta.route_bound_changed_paths.clone(),
            "route_bound_source_changed": !delta.route_bound_changed_paths.is_empty(),
        }),
    );
}

pub(super) fn append_final_acceptance_cycle_summary(
    config: &Config,
    deltas: &[FinalAcceptanceCycleDelta],
) {
    if deltas.is_empty() {
        return;
    }
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "final_acceptance_cycle_summary",
            "cycles": deltas
                .iter()
                .map(|delta| {
                    json!({
                        "cycle_index": delta.cycle_index,
                        "resolved_keys": delta.resolved_keys.clone(),
                        "remaining_keys": delta.remaining_keys.clone(),
                        "changed_paths": delta.changed_paths.clone(),
                        "route_bound_changed_paths": delta.route_bound_changed_paths.clone(),
                    })
                })
                .collect::<Vec<_>>(),
        }),
    );
    let mut lines = vec!["Final acceptance repair cycles:".to_string()];
    for delta in deltas {
        lines.push(format!(
            "- cycle {}: resolved={} remaining={} changed={} route_bound_changed={}",
            delta.cycle_index,
            missing_if_empty(&delta.resolved_keys.join(", ")),
            missing_if_empty(&delta.remaining_keys.join(", ")),
            missing_if_empty(&delta.changed_paths.join(", ")),
            missing_if_empty(&delta.route_bound_changed_paths.join(", "))
        ));
    }
    eval_events::append_run_summary(config.eval_events_path.as_deref(), &lines.join("\n"));
}

pub(super) fn snapshot_last_known_good_sources(
    config: &Config,
    origin_kind: &str,
    origin_id: Option<&str>,
    profile: &str,
    goal: &str,
    extra_paths: &[String],
) {
    let Some(latest_dir) = snapshot_latest_dir(config) else {
        return;
    };
    let paths = snapshot_source_candidates(&config.workspace_root, profile, goal, extra_paths);
    let mut saved_paths = Vec::new();
    let mut snapshot_origins = Vec::new();
    let mut failures = Vec::new();
    for rel in paths {
        let Some(source) = readable_workspace_source_path(&config.workspace_root, &rel) else {
            continue;
        };
        let destination = latest_dir.join(&rel);
        if let Some(parent) = destination.parent()
            && let Err(err) = std::fs::create_dir_all(parent)
        {
            failures.push(format!("{rel}: create snapshot dir failed: {err}"));
            continue;
        }
        match std::fs::copy(&source, &destination) {
            Ok(_) => {
                saved_paths.push(rel.clone());
                snapshot_origins.push(workspace_relative_handoff_path(&destination));
            }
            Err(err) => failures.push(format!("{rel}: snapshot copy failed: {err}")),
        }
    }
    if !saved_paths.is_empty() || !failures.is_empty() {
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "compile_snapshot_saved",
                "origin_kind": origin_kind,
                "origin_id": origin_id.unwrap_or_default(),
                "snapshot_paths": saved_paths,
                "snapshot_origins": snapshot_origins,
                "snapshot_failures": failures,
            }),
        );
    }
}

pub(super) fn snapshot_source_candidates(
    root: &Path,
    profile: &str,
    goal: &str,
    extra_paths: &[String],
) -> Vec<String> {
    let mut paths = Vec::new();
    merge_source_candidates(&mut paths, extra_paths.iter().map(String::as_str));
    let profile_paths = profile_expected_paths(root, profile, goal);
    merge_source_candidates(&mut paths, profile_paths.iter().map(String::as_str));
    merge_source_candidates(
        &mut paths,
        crate::planner::repair_targeting::default_repair_target_candidates(root, profile)
            .iter()
            .map(String::as_str),
    );
    for dir in ["src/app", "app", "pages", "src/components", "components"] {
        collect_source_files_under(root, dir, &mut paths, 0);
    }
    paths.truncate(128);
    paths
}

pub(super) fn final_acceptance_source_snapshot(
    root: &Path,
    profile: &str,
    goal: &str,
    expected_paths: &[String],
    extra_paths: &[String],
) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut paths = snapshot_source_candidates(root, profile, goal, expected_paths);
    merge_final_acceptance_snapshot_candidates(
        &mut paths,
        expected_paths.iter().map(String::as_str),
    );
    merge_final_acceptance_snapshot_candidates(&mut paths, extra_paths.iter().map(String::as_str));
    merge_final_acceptance_snapshot_candidates(
        &mut paths,
        crate::planner::repair_targeting::final_acceptance_snapshot_candidates(root, profile)
            .iter()
            .map(String::as_str),
    );
    for path in route_bound_source_paths(root, profile) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    snapshot_paths(root, &paths)
}

pub(super) fn snapshot_paths(root: &Path, paths: &[String]) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut snapshot = BTreeMap::new();
    for path in paths {
        let Some(rel) = safe_final_acceptance_snapshot_rel_path(path) else {
            continue;
        };
        snapshot.insert(rel.clone(), std::fs::read(root.join(rel)).ok());
    }
    snapshot
}

pub(super) fn changed_snapshot_paths(
    before: &BTreeMap<String, Option<Vec<u8>>>,
    after: &BTreeMap<String, Option<Vec<u8>>>,
) -> Vec<String> {
    before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| before.get(path) != after.get(path))
        .collect()
}

pub(super) fn route_bound_changed_paths(
    before: &BTreeMap<String, Option<Vec<u8>>>,
    after: &BTreeMap<String, Option<Vec<u8>>>,
    before_route_bound: &[String],
    after_route_bound: &[String],
) -> Vec<String> {
    let route_bound = before_route_bound
        .iter()
        .chain(after_route_bound.iter())
        .cloned()
        .collect::<BTreeSet<_>>();
    changed_snapshot_paths(before, after)
        .into_iter()
        .filter(|path| route_bound.contains(path))
        .collect()
}

pub(super) fn merge_source_candidates<'a>(
    out: &mut Vec<String>,
    candidates: impl IntoIterator<Item = &'a str>,
) {
    for candidate in candidates {
        if let Some(rel) = safe_source_rel_path(candidate)
            && !out.contains(&rel)
        {
            out.push(rel);
        }
    }
}

pub(super) fn merge_final_acceptance_snapshot_candidates<'a>(
    out: &mut Vec<String>,
    candidates: impl IntoIterator<Item = &'a str>,
) {
    for candidate in candidates {
        if let Some(rel) = safe_final_acceptance_snapshot_rel_path(candidate)
            && !out.contains(&rel)
        {
            out.push(rel);
        }
    }
}

pub(super) fn collect_source_files_under(
    root: &Path,
    rel_dir: &str,
    out: &mut Vec<String>,
    depth: usize,
) {
    if depth > 4 || out.len() >= 128 {
        return;
    }
    let Some(rel_dir) = safe_dir_rel_path(rel_dir) else {
        return;
    };
    let dir = root.join(&rel_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name().to_string_lossy().to_string();
        if name == "node_modules" || name == ".anvil" || name.starts_with('.') {
            continue;
        }
        let rel = format!("{rel_dir}/{name}").replace('\\', "/");
        if path.is_dir() {
            collect_source_files_under(root, &rel, out, depth + 1);
        } else if let Some(rel) = safe_source_rel_path(&rel)
            && !out.contains(&rel)
        {
            out.push(rel);
        }
        if out.len() >= 128 {
            break;
        }
    }
}

pub(super) fn snapshot_latest_dir(config: &Config) -> Option<PathBuf> {
    config
        .eval_events_path
        .as_ref()
        .and_then(|path| path.parent())
        .map(|run_dir| run_dir.join("snapshots").join("latest"))
}

pub(super) fn readable_workspace_source_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = safe_source_rel_path(rel)?;
    let path = root.join(rel);
    if !path.is_file() {
        return None;
    }
    let root = root.canonicalize().ok()?;
    let canonical = path.canonicalize().ok()?;
    canonical.starts_with(root).then_some(path)
}

pub(super) fn writable_workspace_source_path(root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = safe_source_rel_path(rel)?;
    let path = root.join(rel);
    let parent = path.parent()?;
    let root = root.canonicalize().ok()?;
    let parent = parent.canonicalize().ok()?;
    parent.starts_with(root).then_some(path)
}

pub(super) fn safe_dir_rel_path(raw: &str) -> Option<String> {
    let rel = raw.trim().trim_start_matches("./").replace('\\', "/");
    if rel.is_empty() || rel.starts_with('/') || rel.contains('\0') {
        return None;
    }
    let path = Path::new(&rel);
    if !path
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        return None;
    }
    let lower = rel.to_ascii_lowercase();
    if lower == ".anvil"
        || lower.starts_with(".anvil/")
        || lower.contains("/.anvil/")
        || lower == "node_modules"
        || lower.starts_with("node_modules/")
        || lower.contains("/node_modules/")
    {
        return None;
    }
    Some(rel)
}

pub(super) fn safe_source_rel_path(raw: &str) -> Option<String> {
    let rel = safe_dir_rel_path(raw)?;
    let lower = rel.to_ascii_lowercase();
    if matches!(
        lower.as_str(),
        "package-lock.json" | "pnpm-lock.yaml" | "yarn.lock" | "bun.lockb" | "cargo.lock"
    ) {
        return None;
    }
    let path = Path::new(&rel);
    let ext = path.extension().and_then(|ext| ext.to_str())?;
    matches!(ext, "tsx" | "ts" | "jsx" | "js" | "mjs" | "cjs" | "css").then_some(rel)
}

pub(super) fn safe_final_acceptance_snapshot_rel_path(raw: &str) -> Option<String> {
    if let Some(rel) = safe_source_rel_path(raw) {
        return Some(rel);
    }
    let rel = safe_dir_rel_path(raw)?;
    let lower = rel.to_ascii_lowercase();
    matches!(
        lower.as_str(),
        "package.json"
            | "tsconfig.json"
            | "next.config.js"
            | "next.config.mjs"
            | "next.config.ts"
            | "postcss.config.js"
            | "postcss.config.mjs"
            | "tailwind.config.js"
            | "tailwind.config.ts"
            | "vite.config.js"
            | "vite.config.ts"
            | "cargo.toml"
            | "pyproject.toml"
    )
    .then_some(rel)
}

pub(super) fn profile_invariant_offending_file_excerpts(
    root: &Path,
    profile: &str,
    reason: &str,
) -> String {
    let paths = profile_invariant_relevant_paths(root, profile, reason);
    if paths.is_empty() {
        return "- no matching profile files found".to_string();
    }
    paths
        .into_iter()
        .filter_map(|path| {
            let content = std::fs::read_to_string(&path).ok()?;
            let label = path
                .strip_prefix(root)
                .map(|path| path.display().to_string())
                .unwrap_or_else(|_| path.display().to_string());
            Some(format!(
                "--- {label} ---\n{}",
                bounded_file_excerpt(&content, PROFILE_REPAIR_FILE_EXCERPT_MAX_CHARS)
            ))
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

pub(super) fn profile_invariant_relevant_paths(
    root: &Path,
    profile: &str,
    reason: &str,
) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if !matches!(profile, "nextjs" | "next-js" | "next.js") {
        return out;
    }
    let tailwind_failure = reason.contains("tailwind_contract_failure");
    let rels = crate::planner::repair_targeting::profile_invariant_excerpt_candidates(
        root,
        profile,
        tailwind_failure,
    );
    for project_root in profile_excerpt_project_roots(root) {
        for rel in &rels {
            let path = project_root.join(rel);
            if path.is_file() && !out.contains(&path) {
                out.push(path);
            }
        }
    }
    out
}

pub(super) fn profile_excerpt_project_roots(root: &Path) -> Vec<PathBuf> {
    let mut roots = vec![root.to_path_buf()];
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() && path.join("package.json").is_file() && !roots.contains(&path) {
                roots.push(path);
            }
        }
    }
    roots
}

pub(super) fn bounded_file_excerpt(content: &str, max_chars: usize) -> String {
    if content.chars().count() <= max_chars {
        return content.to_string();
    }
    let mut excerpt = content.chars().take(max_chars).collect::<String>();
    excerpt.push_str("\n[truncated]");
    excerpt
}

#[cfg(test)]
pub(super) fn ultra_final_acceptance_report(
    plan: &UltraPlan,
    config: &Config,
) -> anyhow::Result<VerificationReport> {
    ultra_final_acceptance_report_with_cycle(plan, config, 0)
}

pub(super) fn ultra_final_acceptance_report_with_cycle(
    plan: &UltraPlan,
    config: &Config,
    cycle_index: usize,
) -> anyhow::Result<VerificationReport> {
    with_final_acceptance_cycle(cycle_index, || {
        ultra_final_acceptance_report_inner(plan, config, cycle_index)
    })
}

pub(super) fn ultra_final_acceptance_report_with_deterministic_remedies(
    plan: &UltraPlan,
    config: &Config,
    cycle_index: usize,
    setup_authority: &mut UltraRunSetupAuthorityState,
) -> anyhow::Result<(VerificationReport, Vec<String>)> {
    let mut deterministic_remedies_applied = Vec::new();
    if reconcile_manifest_changed_dependencies_if_needed(config, &plan.profile, setup_authority)?
        .is_some()
    {
        push_unique_label(
            &mut deterministic_remedies_applied,
            "manifest_changed_dependency_reconciliation",
        );
        clear_final_acceptance_browser_probe_evidence(config);
    }
    let mut report = ultra_final_acceptance_report_with_cycle(plan, config, cycle_index)?;
    if acceptance_dependency_deterministic_reconcile_needed(config, &report) {
        setup_authority.grant("declared_dependencies_not_ready");
        if reconcile_run_dependency_setup(
            config,
            &plan.profile,
            DependencyReconciliationTrigger::DeclaredDependenciesNotReady,
            setup_authority,
        )?
        .is_some()
        {
            push_unique_label(
                &mut deterministic_remedies_applied,
                "declared_dependencies_not_ready_install",
            );
            clear_final_acceptance_browser_probe_evidence(config);
            report = ultra_final_acceptance_report_with_cycle(plan, config, cycle_index)?;
        }
    }
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "final_acceptance_deterministic_remedies",
            "cycle_index": cycle_index,
            "lifecycle_stage": "final_acceptance",
            "deterministic_remedies_applied": deterministic_remedies_applied.clone(),
        }),
    );
    Ok((report, deterministic_remedies_applied))
}

pub(super) fn ultra_final_acceptance_report_inner(
    plan: &UltraPlan,
    config: &Config,
    cycle_index: usize,
) -> anyhow::Result<VerificationReport> {
    let effective_profile = canonical_profile_name(&plan.profile);
    let mut required_paths =
        profile_expected_paths(&config.workspace_root, &effective_profile, &plan.goal);
    let mut required_capabilities = inferred_required_capabilities(&effective_profile, &plan.goal);
    let mut required_obligations =
        inferred_required_obligations(&effective_profile, &plan.goal, &required_capabilities);
    let mut required_evidence =
        inferred_required_evidence(&effective_profile, &plan.goal, &required_capabilities);
    let mut requirements = ContractRequirements {
        capabilities: required_capabilities,
        evidence: required_evidence,
        obligations: required_obligations,
    };
    carry_recorded_promotion_contract_requirements(
        config,
        &effective_profile,
        &plan.goal,
        &mut requirements,
    );
    required_capabilities = requirements.capabilities;
    required_evidence = requirements.evidence;
    required_obligations = requirements.obligations;
    let mut evidence_hint_tokens = evidence_hint_tokens_for_goal(&plan.goal);
    let bound_contract = bind_completion_contract_for_acceptance(
        config,
        "ultra-plan-run",
        &effective_profile,
        &plan.goal,
        &required_paths,
        &required_capabilities,
        &required_evidence,
        &required_obligations,
    )?;
    let mut deferred_commands = Vec::new();
    let mut verify_commands = Vec::new();
    if let Some(contract) = bound_contract.as_ref().map(|bound| &bound.contract) {
        merge_unique_strings(&mut required_paths, &contract.required_paths);
        merge_unique_strings(&mut required_capabilities, &contract.required_capabilities);
        merge_unique_strings(&mut required_evidence, &contract.required_evidence);
        merge_unique_strings(&mut required_obligations, &contract.required_obligations);
        merge_unique_strings(&mut verify_commands, &contract.verify_commands);
        merge_unique_strings(&mut evidence_hint_tokens, &contract.evidence_hint_tokens);
        deferred_commands.extend(
            contract
                .deferred_verify_requirements
                .iter()
                .map(|requirement| requirement.command.clone()),
        );
    }
    merge_unique_strings(
        &mut required_evidence,
        &inferred_required_evidence(&effective_profile, &plan.goal, &required_capabilities),
    );
    let missing = missing_final_artifacts(&config.workspace_root, &required_paths);
    let mut acceptance = verify_runtime_acceptance_with_browser_dirs_and_hints(
        &config.workspace_root,
        &required_paths,
        &verify_commands,
        &required_capabilities,
        &required_evidence,
        &required_obligations,
        &deferred_commands,
        &release_evidence_extra_dirs(config),
        &evidence_hint_tokens,
    );
    let profile_invariant_report = verify_profile_invariant(
        &config.workspace_root,
        &effective_profile,
        &plan.goal,
        &ProfileSnapshot::None,
    );
    let profile_invariant_report = hook_snapshot::report_missing_as_profile_failure(
        config,
        &effective_profile,
        &plan.goal,
        profile_invariant_report,
    );
    let profile_report = if !profile_invariant_report.is_pass() {
        profile_invariant_report
    } else {
        verify_profile_final(&config.workspace_root, &effective_profile, &plan.goal)
    };
    let external_report = bound_contract.as_ref().map(|bound| {
        bound
            .contract
            .verify_with_goal(&config.workspace_root, &plan.goal)
    });
    let contract_required =
        completion_contract_required(&effective_profile, &plan.goal, &required_capabilities)
            || bound_contract.as_ref().is_some_and(|bound| bound.required);
    let external_contract_checked = bound_contract.is_some();
    let contract_binding_missing = contract_required && !external_contract_checked;
    let external_ok = !contract_binding_missing
        && external_contract_ok_after_runtime_arbitration(
            external_report.as_ref(),
            Some(&acceptance),
        );
    let production_build_failed = report_has_production_build_failure(&profile_report)
        || external_report
            .as_ref()
            .is_some_and(report_has_production_build_failure);
    let browser_probe = if production_build_failed {
        None
    } else {
        run_ultra_final_browser_checks_before_arbitration(
            config,
            plan,
            &effective_profile,
            &required_capabilities,
            &required_evidence,
        )
    };
    let evidence_arbitration = final_acceptance_evidence_arbitration(
        config,
        &mut acceptance,
        &required_capabilities,
        &required_evidence,
        &required_obligations,
    );
    let mut release_gate = if production_build_failed {
        production_build_failed_release_gate()
    } else {
        let signal_text = ultra_plan_signal_text(plan);
        final_acceptance_release_gate(
            config,
            &effective_profile,
            &signal_text,
            &required_capabilities,
            Some(&acceptance),
            true,
        )
    };
    let signal_text = ultra_plan_signal_text(plan);
    let gate_telemetry = acceptance_gate_telemetry(
        &effective_profile,
        &signal_text,
        &required_capabilities,
        &required_evidence,
        &release_gate,
    );
    if acceptance.passed
        && external_ok
        && let Some(reason) = acceptance_gates_disconnected_reason(&gate_telemetry, &release_gate)
    {
        release_gate.status = "failed".to_string();
        release_gate.reasons = dedup_strings(
            release_gate
                .reasons
                .iter()
                .cloned()
                .chain(std::iter::once(reason))
                .collect(),
        );
    }
    let profile_behavior_probe = run_profile_behavior_probe(
        config,
        &effective_profile,
        &plan.goal,
        &required_capabilities,
        &profile_report,
    );
    let profile_behavior_failed = profile_behavior_probe.status == "failed";
    if profile_behavior_failed {
        mark_release_gate_profile_behavior_failed(&mut release_gate, &profile_behavior_probe);
    }
    let final_acceptance_status = release_gate_final_acceptance_status(&release_gate);
    let runtime_acceptance_status = if profile_behavior_failed {
        "failed"
    } else {
        runtime_acceptance_status(acceptance.passed, Some(&acceptance))
    };
    let (assurance_level, assurance_reason) = earned_assurance_for_completion(
        &effective_profile,
        &required_capabilities,
        !contract_required || (external_contract_checked && !contract_binding_missing),
        final_acceptance_status,
        &release_gate,
        &gate_telemetry,
    );
    let release_quality_completion =
        release_quality_completion_status(&release_gate, final_acceptance_status);
    let next_action = release_gate_next_action(&release_gate, final_acceptance_status);
    let state_dimensions_changed =
        interaction_state_dimensions_changed_from_path(&release_gate.interaction_evidence_path);
    let action_hooks = interaction_action_hooks_from_path(&release_gate.interaction_evidence_path);
    let surface_fit = interaction_surface_fit_from_path(&release_gate.interaction_evidence_path);
    let text_telemetry =
        interaction_text_telemetry_from_path(&release_gate.interaction_evidence_path);
    let depth_profile = depth_profile(
        &config.workspace_root,
        &effective_profile,
        &state_dimensions_changed,
        &action_hooks,
        &release_gate.interaction_evidence_path,
        &text_telemetry,
    );
    let plan_adherence = plan_adherence_report(plan, &config.workspace_root);
    let phase_signal_text = ultra_plan_phase_signal_text(plan);
    let requested_port =
        effective_requested_port(&effective_profile, &plan.goal, Some(&phase_signal_text));
    let mut compile_errors = profile_report.compile_errors.clone();
    if let Some(report) = external_report.as_ref() {
        for error in &report.compile_errors {
            if !compile_errors.contains(error) {
                compile_errors.push(error.clone());
            }
        }
    }
    let primary_reason = if !missing.is_empty() {
        format!("missing final artifacts: {}", missing.join(", "))
    } else if contract_binding_missing {
        "completion contract binding required but missing".to_string()
    } else if !profile_report.is_pass() {
        profile_report.primary_reason()
    } else if let Some(report) = external_report.as_ref().filter(|report| {
        !external_contract_ok_after_runtime_arbitration(Some(*report), Some(&acceptance))
    }) {
        report.primary_reason()
    } else if profile_behavior_failed {
        profile_behavior_probe.reasons.join("; ")
    } else if !acceptance.passed {
        acceptance.primary_reason.clone()
    } else if matches!(release_gate.status.as_str(), "partial" | "failed") {
        release_gate.reasons.join("; ")
    } else {
        acceptance.primary_reason.clone()
    };
    let recovery_handoff = if acceptance.passed
        && external_ok
        && (matches!(release_gate.status.as_str(), "partial" | "failed")
            || final_acceptance_status == "partial")
    {
        let acceptance_layer =
            release_recovery_acceptance_layer(&release_gate, final_acceptance_status);
        let failure_kind =
            release_recovery_failure_kind(&release_gate, final_acceptance_status, &primary_reason);
        let scope = format!("release-{}", recovery_scope_token(acceptance_layer));
        save_release_recovery_handoff(
            config,
            &effective_profile,
            &plan.goal,
            &scope,
            acceptance_layer,
            &failure_kind,
            release_recovery_failure_evidence(
                &release_gate,
                final_acceptance_status,
                &primary_reason,
                Some(&acceptance),
            ),
            missing.clone(),
            release_recovery_missing_capabilities(Some(&acceptance)),
            release_recovery_repair_targets(&release_gate, Some(&acceptance)),
            release_recovery_verify_commands(&effective_profile, &release_gate),
        )
    } else {
        None
    };
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "ultra_final_acceptance",
            "cycle_index": cycle_index,
            "profile": effective_profile.clone(),
            "effective_profile": effective_profile.clone(),
            "contract_origin": contract_origin_for_acceptance(config),
            "profile_inferred": config
                .profile_inference
                .map(|inference| inference.profile)
                .unwrap_or(""),
            "profile_inference_source": config
                .profile_inference
                .map(|inference| inference.source.as_str())
                .unwrap_or(""),
            "requested_port": requested_port.as_ref().map(|requested| requested.telemetry.clone()),
            "required_paths": required_paths.clone(),
            "missing_paths": missing.clone(),
            "completion_contract_verification_enabled": external_contract_checked,
            "completion_contract_path_merge_enabled": external_contract_checked,
            "completion_contract_path": bound_contract
                .as_ref()
                .map(|bound| bound.path.clone())
                .unwrap_or_default(),
            "completion_contract_generated": bound_contract
                .as_ref()
                .map(|bound| bound.generated)
                .unwrap_or(false),
            "external_contract_checked": external_contract_checked,
            "external_contract_required": contract_required,
            "external_contract_ok": external_ok,
            "required_capabilities": required_capabilities.clone(),
            "required_evidence": required_evidence.clone(),
            "required_obligations": required_obligations.clone(),
            "runtime_acceptance_passed": acceptance.passed && !profile_behavior_failed,
            "runtime_acceptance_status": runtime_acceptance_status,
            "runtime_acceptance_inconclusive": acceptance.inconclusive,
            "compile_errors": compile_errors.clone(),
            "compile_error_failure_kind": if compile_errors.is_empty() { "" } else { "implementation_compile_error" },
            "final_acceptance_status": final_acceptance_status,
            "assurance_level": assurance_level,
            "assurance_reason": assurance_reason,
            "release_quality_completion": release_quality_completion,
            "missing_capabilities": acceptance.missing_capabilities.clone(),
            "missing_evidence": acceptance.missing_evidence.clone(),
            "missing_obligations": acceptance.missing_obligations.clone(),
            "weak_evidence": acceptance.weak_evidence.clone(),
            "runtime_acceptance_diagnostics": acceptance.diagnostics.clone(),
            "unverified_evidence": acceptance.unverified_evidence.clone(),
            "evidence_tiers": acceptance.evidence_tiers.clone(),
            "evidence_arbitration": evidence_arbitration.records.clone(),
            "evidence_arbitration_summary": evidence_arbitration.summary.clone(),
            "artifact_obligations": acceptance.artifact_obligations.clone(),
            "capability_evidence_bindings": acceptance.capability_evidence_bindings.clone(),
            "obligation_repair_targets": acceptance.obligation_repair_targets.clone(),
            "inconclusive_reasons": acceptance.inconclusive_reasons.clone(),
            "release_gate_status": release_gate.status.clone(),
            "release_gate_reasons": release_gate.reasons.clone(),
            "profile_behavior_probe_status": profile_behavior_probe.status,
            "profile_behavior_probe_reasons": profile_behavior_probe.reasons.clone(),
            "profile_behavior_probe_evidence_path": profile_behavior_probe.evidence_path.clone().unwrap_or_default(),
            "browser_readiness_applicable": gate_telemetry.browser_readiness_applicable,
            "browser_readiness_execution_status": gate_telemetry.browser_readiness_execution_status.clone(),
            "browser_readiness_status": release_gate.browser_readiness_status.clone(),
            "browser_readiness_evidence_path": release_gate.browser_readiness_evidence_path.clone(),
            "interaction_evidence_applicable": gate_telemetry.interaction_evidence_applicable,
            "interaction_evidence_execution_status": gate_telemetry.interaction_evidence_execution_status.clone(),
            "interaction_evidence_status": release_gate.interaction_evidence_status.clone(),
            "interaction_evidence_path": release_gate.interaction_evidence_path.clone(),
            "state_dimensions_changed": state_dimensions_changed,
            "action_hooks": action_hooks,
            "surface_fit": surface_fit.raw,
            "surface_fit_summary": surface_fit.summary,
            "surface_fit_guidance": surface_fit.guidance,
            "text_entry": text_telemetry.text_entry,
            "text_entry_target": text_telemetry.text_entry_target,
            "typed_token": text_telemetry.typed_token,
            "token_echoed": text_telemetry.token_echoed,
            "echo_latency_ms": text_telemetry.echo_latency_ms,
            "token_echoed_after_reload": text_telemetry.token_echoed_after_reload,
            "token_echo_after_reload_latency_ms": text_telemetry.token_echo_after_reload_latency_ms,
            "text_input_state_change": text_telemetry.text_input_state_change,
            "next_action": next_action,
            "recovery_handoff_kind": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.recovery_handoff_kind.as_str())
                .unwrap_or_default(),
            "acceptance_layer": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.acceptance_layer.as_str())
                .unwrap_or_default(),
            "recovery_prompt_path": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.recovery_prompt_path.as_str())
                .unwrap_or_default(),
            "recovery_ultra_plan_path": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.recovery_ultra_plan_path.as_str())
                .unwrap_or_default(),
            "suggested_recovery_command": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.suggested_recovery_command.as_str())
                .unwrap_or_default(),
            "suggested_recovery_yaml_command": recovery_handoff
                .as_ref()
                .map(|handoff| handoff.suggested_recovery_yaml_command.as_str())
                .unwrap_or_default(),
            "plan_adherence_present": plan_adherence.present.clone(),
            "plan_adherence_missing": plan_adherence.missing.clone(),
            "recovery_handoff_saved": recovery_handoff
                .as_ref()
                .is_some_and(ReleaseRecoveryHandoffSummary::has_artifact),
            "handoff_saved_not_success": recovery_handoff.is_some(),
            "primary_reason": eval_events::body_snippet(&primary_reason),
        }),
    );
    emit_depth_profile(
        config.eval_events_path.as_deref(),
        "ultra_final_acceptance",
        &depth_profile,
    );
    let mut report = VerificationReport::pass();
    for path in missing {
        report.push_missing_path(path);
    }
    merge_verification_report(&mut report, profile_report);
    if !acceptance.passed {
        report.push_profile_failure(acceptance.primary_reason.clone());
        for guidance in runtime_acceptance_repair_guidance(&acceptance) {
            report.push_profile_failure(format!("repair guidance: {guidance}"));
        }
        for target in &acceptance.obligation_repair_targets {
            report.push_profile_failure(format!(
                "missing_required_obligation_target:{}:{}",
                target.obligation, target.target_path
            ));
        }
    }
    if contract_binding_missing {
        report.push_profile_failure("completion contract binding required but missing".to_string());
    }
    if profile_behavior_failed {
        for reason in &profile_behavior_probe.reasons {
            report.push_profile_failure(reason.clone());
        }
        if let Some(path) = &profile_behavior_probe.evidence_path {
            report.push_profile_failure(format!("profile behavior evidence: {path}"));
        }
    }
    if let Some(external_report) = external_report.filter(|report| {
        !external_contract_ok_after_runtime_arbitration(Some(report), Some(&acceptance))
    }) {
        let reason = external_report.primary_reason();
        merge_verification_report(&mut report, external_report);
        report.push_profile_failure(format!("external contract failed: {}", reason));
    }
    let append_release_observations = !report.is_pass() || release_gate.status == "failed";
    if release_gate.status == "failed" {
        report.push_profile_failure(format!(
            "release gate failed: {}",
            release_gate.reasons.join("; ")
        ));
    }
    if append_release_observations {
        append_release_gate_observation_failures(&mut report, &release_gate);
    }
    if let Some(observation) = &browser_probe
        && let Some(reason) = observation.failure_reason()
    {
        report.push_profile_failure(format!("browser_readiness_failed:{reason}"));
        if !observation.output_excerpt.trim().is_empty() {
            report.push_profile_failure(format!(
                "browser_probe_output: {}",
                eval_events::body_snippet(&observation.output_excerpt)
            ));
        }
        if observation.failure_kind == "build_verifier_failed"
            && !observation.compile_errors.is_empty()
        {
            report.push_compile_errors(
                "browser readiness build verifier",
                observation.compile_errors.clone(),
            );
        }
    }
    Ok(report)
}

pub(super) fn clear_final_acceptance_browser_probe_evidence(config: &Config) {
    for path in release_evidence_candidate_paths(
        config,
        &[
            "browser-readiness.json",
            "browser.json",
            "browser-readiness-evidence.json",
            "browser-interaction.json",
        ],
    ) {
        let _ = std::fs::remove_file(path);
    }
}

pub(super) fn final_acceptance_evidence_arbitration(
    config: &Config,
    report: &mut RuntimeAcceptanceReport,
    required_capabilities: &[String],
    required_evidence: &[String],
    required_obligations: &[String],
) -> EvidenceArbitrationReport {
    behavior_evidence::arbitrate_final_acceptance(
        report,
        &config.workspace_root,
        &release_evidence_extra_dirs(config),
        required_capabilities,
        required_evidence,
        required_obligations,
    )
}

pub(super) fn final_acceptance_app_behavior_failure_kind(
    report: &VerificationReport,
) -> Option<String> {
    let primary_reason = report.primary_reason();
    report
        .profile_failures
        .iter()
        .map(String::as_str)
        .chain(std::iter::once(primary_reason.as_str()))
        .find_map(app_behavior_probe_failure_kind)
}

pub(super) fn final_acceptance_repair_expected_paths(
    plan: &UltraPlan,
    config: &Config,
    report: &VerificationReport,
) -> anyhow::Result<Vec<String>> {
    let mut expected = profile_expected_paths(&config.workspace_root, &plan.profile, &plan.goal);
    if let Some(contract) = CompletionContract::load_for_config(config)? {
        merge_unique_strings(&mut expected, &contract.required_paths);
    }
    merge_unique_strings(&mut expected, &compile_error_paths(&report.compile_errors));
    merge_unique_strings(&mut expected, &report.missing_paths);
    merge_unique_strings(&mut expected, &obligation_repair_target_paths(report));
    Ok(expected)
}

pub(super) fn contract_attribute_repair_target_paths(report: &VerificationReport) -> Vec<String> {
    crate::planner::contract_attribute_repair::detect(report)
        .map(|issue| vec![issue.path])
        .unwrap_or_default()
}

pub(super) fn compile_error_paths(errors: &[CompileError]) -> Vec<String> {
    errors
        .iter()
        .map(|error| error.path.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(super) fn single_compile_regeneration_target(
    report: &VerificationReport,
) -> Result<String, String> {
    if report.compile_errors.is_empty() {
        return Err("no_compile_errors".to_string());
    }
    let paths = report
        .compile_errors
        .iter()
        .filter_map(|error| safe_source_rel_path(&error.path))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    match paths.as_slice() {
        [path] => Ok(path.clone()),
        [] => Err("no_safe_source_target".to_string()),
        _ => Err("multi_file_compile_failure".to_string()),
    }
}

pub(super) fn changed_paths_only_target(changed_paths: &[String], target_path: &str) -> bool {
    let changed = changed_paths
        .iter()
        .filter_map(|path| safe_source_rel_path(path))
        .collect::<BTreeSet<_>>();
    changed.len() == 1 && changed.contains(target_path)
}

pub(super) fn evidence_repair_zero_edit_eligible(
    report: &VerificationReport,
    repair_target: RepairTarget,
) -> bool {
    report.compile_errors.is_empty()
        && repair_target == RepairTarget::Implementation
        && !verification_missing_signals(report).is_empty()
}

pub(super) fn evidence_repair_retry_mode(
    evidence_zero_edit_eligible: bool,
    evidence_no_source_change_count: usize,
) -> (&'static str, bool, bool) {
    let reanchored_retry = evidence_zero_edit_eligible && evidence_no_source_change_count == 1;
    let compact_retry = evidence_zero_edit_eligible && evidence_no_source_change_count >= 2;
    (
        if compact_retry { "compact" } else { "appended" },
        reanchored_retry,
        compact_retry,
    )
}

pub(super) fn final_acceptance_evidence_regeneration_target(
    root: &Path,
    profile: &str,
    report: &VerificationReport,
    expected_paths: &[String],
) -> Option<String> {
    let pending_evidence = verification_missing_signals(report);
    let contract_attribute_paths = contract_attribute_repair_target_paths(report);
    crate::planner::repair_targeting::resolve_repair_targets(
        crate::planner::repair_targeting::RepairTargetResolutionInput {
            root,
            profile,
            pending_evidence: &pending_evidence,
            missing_capabilities: &[],
            contract_attribute_paths: &contract_attribute_paths,
            repair_changed_paths: &[],
            required_paths: expected_paths,
            fallback_paths: &[],
        },
    )
    .and_then(|selection| {
        selection
            .selected_targets
            .into_iter()
            .find_map(|path| safe_source_rel_path(&path))
    })
}

pub(super) fn build_final_acceptance_evidence_regeneration_prompt(
    root: &Path,
    plan: &UltraPlan,
    report: &VerificationReport,
    target_path: &str,
) -> String {
    let current_content = std::fs::read_to_string(root.join(target_path)).unwrap_or_default();
    let pending_keys = verification_missing_signals(report);
    format!(
        "Repair session mode: compact regeneration.\n\
Evidence-target regeneration for final acceptance.\n\n\
Original ultra goal:\n{goal}\n\n\
Pending evidence keys:\n{keys}\n\n\
Current content of {target_path}:\n\
```tsx\n\
{current_content}\n\
```\n\n\
Regeneration mandate:\n\
- This is generation, not incremental editing.\n\
- Write the complete corrected file via the Write tool (full content, one file only): {target_path}.\n\
- Add concrete route-bound implementation evidence for the pending keys without weakening build or verification.\n\
- Do not modify any other file.\n\
- Stop immediately after the Write tool call.",
        goal = plan.goal,
        keys = render_prompt_bullets(&pending_keys),
        current_content = current_content,
        target_path = target_path,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn try_final_acceptance_compile_regeneration(
    execution: &mut dyn ChatClient,
    config: &Config,
    plan: &UltraPlan,
    ultra_context: &mut UltraRunContext,
    acceptance_report: &mut VerificationReport,
    deterministic_remedies_applied: &mut Vec<String>,
    setup_authority_state: &mut UltraRunSetupAuthorityState,
    cycle_index: usize,
    expected_paths: &[String],
    repair_config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<bool> {
    if acceptance_report.compile_errors.is_empty() {
        return Ok(false);
    }
    let before_error_count = acceptance_report.compile_errors.len();
    let target_path = match single_compile_regeneration_target(acceptance_report) {
        Ok(path) => path,
        Err(reason) => {
            emit_compile_regeneration_event(
                config,
                None,
                "final_acceptance_repair",
                false,
                false,
                0,
                None,
                &reason,
                before_error_count,
                before_error_count,
                &[],
            );
            return Ok(false);
        }
    };
    let Some(target_abs) = writable_workspace_source_path(&config.workspace_root, &target_path)
    else {
        emit_compile_regeneration_event(
            config,
            None,
            "final_acceptance_repair",
            false,
            false,
            0,
            Some(&target_path),
            "target_path_rejected",
            before_error_count,
            before_error_count,
            &[],
        );
        return Ok(false);
    };
    let before_content = match std::fs::read(&target_abs) {
        Ok(content) => content,
        Err(err) => {
            emit_compile_regeneration_event(
                config,
                None,
                "final_acceptance_repair",
                false,
                false,
                0,
                Some(&target_path),
                &format!("snapshot_read_error:{err}"),
                before_error_count,
                before_error_count,
                &[],
            );
            return Ok(false);
        }
    };
    let repair_context = RepairContext {
        profile: Some(plan.profile.clone()),
        overall_goal: Some(plan.goal.clone()),
        required_final_artifacts: expected_paths.to_vec(),
        expected_paths: expected_paths.to_vec(),
        missing_paths: acceptance_report.missing_paths.clone(),
        workspace_root: Some(config.workspace_root.clone()),
        prompt_layout: config.prompt_layout,
        compile_reanchored_retry: true,
        ..RepairContext::default()
    };
    let regeneration_prompt = build_compile_regeneration_prompt_with_context(
        "final-acceptance",
        acceptance_report,
        &repair_context,
        &target_path,
    );
    let mut regeneration_session = SessionSnapshot::new();
    let regeneration = run_final_acceptance_repair_with_ultra_session(
        execution,
        &mut regeneration_session,
        &regeneration_prompt,
        std::slice::from_ref(&target_path),
        repair_config,
        ui,
    );
    let regeneration = match regeneration {
        Ok(outcome) => outcome,
        Err(err) => {
            let _ = std::fs::write(&target_abs, &before_content);
            emit_compile_regeneration_event(
                config,
                None,
                "final_acceptance_repair",
                true,
                false,
                0,
                Some(&target_path),
                &format!(
                    "regeneration_turn_error:{}",
                    eval_events::body_snippet(&err.to_string())
                ),
                before_error_count,
                before_error_count,
                &[],
            );
            return Ok(false);
        }
    };
    let mut regeneration_changed_paths = regeneration.changed_paths.clone();
    regeneration_changed_paths.sort();
    regeneration_changed_paths.dedup();
    let one_file_write = changed_paths_only_target(&regeneration_changed_paths, &target_path);
    clear_final_acceptance_browser_probe_evidence(config);
    let (regenerated_report, regenerated_deterministic_remedies) =
        ultra_final_acceptance_report_with_deterministic_remedies(
            plan,
            config,
            cycle_index,
            setup_authority_state,
        )?;
    let after_error_count = regenerated_report.compile_errors.len();
    let error_delta = before_error_count as i64 - after_error_count as i64;
    if one_file_write && error_delta > 0 {
        emit_compile_regeneration_event(
            config,
            None,
            "final_acceptance_repair",
            true,
            true,
            error_delta,
            Some(&target_path),
            "accepted",
            before_error_count,
            after_error_count,
            &regeneration_changed_paths,
        );
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
        *deterministic_remedies_applied = regenerated_deterministic_remedies;
        *acceptance_report = regenerated_report;
        return Ok(true);
    }
    let _ = std::fs::write(&target_abs, &before_content);
    emit_compile_regeneration_event(
        config,
        None,
        "final_acceptance_repair",
        true,
        false,
        error_delta,
        Some(&target_path),
        if one_file_write {
            "compile_error_count_not_decreased"
        } else {
            "changed_paths_not_single_target"
        },
        before_error_count,
        after_error_count,
        &regeneration_changed_paths,
    );
    Ok(false)
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_evidence_regeneration_event(
    config: &Config,
    fired: bool,
    accepted: bool,
    target_path: Option<&str>,
    before_keys: &[String],
    after_keys: &[String],
    changed_paths: &[String],
    reason: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "repair_regeneration",
            "lifecycle_stage": "final_acceptance_repair",
            "fired": fired,
            "accepted": accepted,
            "target_path": target_path.unwrap_or_default(),
            "reason": reason,
            "before_missing_evidence": before_keys,
            "after_missing_evidence": after_keys,
            "resolved_missing_evidence": resolved_missing_signals(before_keys, after_keys),
            "changed_paths": changed_paths,
            "repair_session_mode": if fired { "compact_regeneration" } else { "" },
            "regeneration_gate": "evidence_static_present_and_build_passes",
        }),
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn emit_compile_regeneration_event(
    config: &Config,
    step_id: Option<&str>,
    lifecycle_stage: &str,
    fired: bool,
    accepted: bool,
    error_delta: i64,
    target_path: Option<&str>,
    reason: &str,
    before_errors: usize,
    after_errors: usize,
    changed_paths: &[String],
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "repair_regeneration",
            "step_id": step_id.unwrap_or_default(),
            "lifecycle_stage": lifecycle_stage,
            "fired": fired,
            "accepted": accepted,
            "error_delta": error_delta,
            "target_path": target_path.unwrap_or_default(),
            "reason": reason,
            "before_compile_error_count": before_errors,
            "after_compile_error_count": after_errors,
            "changed_paths": changed_paths,
            "repair_session_mode": if fired { "compact_regeneration" } else { "" },
            "regeneration_skipped_reason": if fired { "" } else { reason },
        }),
    );
}

#[derive(Debug, Clone, Default)]
pub(super) struct CompileRollbackOutcome {
    pub(super) paths: Vec<String>,
    pub(super) snapshot_origins: Vec<String>,
    pub(super) carry_forward_guidance: Vec<String>,
}

pub(super) fn try_compile_rollback_after_repair_exhaustion(
    config: &Config,
    profile: &str,
    goal: &str,
    phase_id: &str,
    phase_prompt: &str,
    report: &VerificationReport,
    exhausted_reason: &str,
) -> anyhow::Result<Option<CompileRollbackOutcome>> {
    let failing_paths = compile_error_paths(&report.compile_errors)
        .into_iter()
        .filter_map(|path| safe_source_rel_path(&path))
        .collect::<Vec<_>>();
    if failing_paths.is_empty() {
        return Ok(None);
    }
    let Some(latest_dir) = snapshot_latest_dir(config) else {
        emit_compile_rollback_skipped(
            config,
            &failing_paths,
            exhausted_reason,
            "snapshot_store_missing",
        );
        return Ok(None);
    };
    let mut snapshot_paths = Vec::new();
    for rel in &failing_paths {
        let snapshot = latest_dir.join(rel);
        if !snapshot.is_file() {
            emit_compile_rollback_skipped(
                config,
                &failing_paths,
                exhausted_reason,
                &format!("snapshot_missing:{rel}"),
            );
            return Ok(None);
        }
        snapshot_paths.push(snapshot);
    }
    let mut restored_paths = Vec::new();
    let mut origins = Vec::new();
    for (rel, snapshot) in failing_paths.iter().zip(snapshot_paths.iter()) {
        let Some(destination) = writable_workspace_source_path(&config.workspace_root, rel) else {
            emit_compile_rollback_skipped(
                config,
                &failing_paths,
                exhausted_reason,
                &format!("restore_path_rejected:{rel}"),
            );
            return Ok(None);
        };
        let content = std::fs::read(snapshot)?;
        std::fs::write(&destination, content)?;
        restored_paths.push(rel.clone());
        origins.push(workspace_relative_handoff_path(snapshot));
    }
    let rebuild_report = verify_profile_final(&config.workspace_root, profile, goal);
    if report_has_production_build_failure(&rebuild_report) {
        eval_events::emit(
            config.eval_events_path.as_deref(),
            json!({
                "event": "compile_rollback_failed",
                "phase_id": phase_id,
                "paths": restored_paths,
                "snapshot_origins": origins,
                "exhausted_reason": exhausted_reason,
                "rebuild_reason": eval_events::body_snippet(&rebuild_report.primary_reason()),
            }),
        );
        return Ok(None);
    }
    let phase_goal = phase_goal_one_liner(phase_prompt);
    let carry_forward_guidance = restored_paths
        .iter()
        .map(|path| {
            format!(
                "phase {} changes to {} were rolled back; re-apply: {}",
                phase_id, path, phase_goal
            )
        })
        .collect::<Vec<_>>();
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "compile_rollback_applied",
            "phase_id": phase_id,
            "paths": restored_paths.clone(),
            "snapshot_origins": origins.clone(),
            "exhausted_reason": exhausted_reason,
            "carry_forward_guidance": carry_forward_guidance.clone(),
        }),
    );
    eval_events::append_run_summary(
        config.eval_events_path.as_deref(),
        &format!(
            "Compile rollback applied:\n- paths: {}\n- snapshot origin: {}\n- carry-forward: {}",
            missing_if_empty(&restored_paths.join(", ")),
            missing_if_empty(&origins.join(", ")),
            missing_if_empty(&carry_forward_guidance.join("; "))
        ),
    );
    Ok(Some(CompileRollbackOutcome {
        paths: restored_paths,
        snapshot_origins: origins,
        carry_forward_guidance,
    }))
}

pub(super) fn emit_compile_rollback_context_carried(
    config: &Config,
    rollback: &CompileRollbackOutcome,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "compile_rollback_context_carried",
            "paths": &rollback.paths,
            "snapshot_origins": &rollback.snapshot_origins,
            "carry_forward_guidance": &rollback.carry_forward_guidance,
        }),
    );
}

pub(super) fn emit_compile_rollback_skipped(
    config: &Config,
    failing_paths: &[String],
    exhausted_reason: &str,
    reason: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "compile_rollback_skipped",
            "paths": failing_paths,
            "exhausted_reason": exhausted_reason,
            "reason": reason,
        }),
    );
}

pub(super) fn should_run_compile_no_snapshot_narrow_retry(
    config: &Config,
    report: &VerificationReport,
    already_used: bool,
) -> bool {
    !already_used
        && !report.compile_errors.is_empty()
        && !compile_rollback_snapshot_available(config, report)
}

pub(super) fn compile_rollback_snapshot_available(
    config: &Config,
    report: &VerificationReport,
) -> bool {
    let failing_paths = compile_error_paths(&report.compile_errors)
        .into_iter()
        .filter_map(|path| safe_source_rel_path(&path))
        .collect::<Vec<_>>();
    if failing_paths.is_empty() {
        return false;
    }
    let Some(latest_dir) = snapshot_latest_dir(config) else {
        return false;
    };
    failing_paths
        .iter()
        .all(|rel| latest_dir.join(rel).is_file())
}

pub(super) fn emit_compile_no_snapshot_narrow_retry(
    config: &Config,
    attempt: usize,
    report: &VerificationReport,
    trigger: &str,
) {
    eval_events::emit(
        config.eval_events_path.as_deref(),
        json!({
            "event": "compile_no_snapshot_narrow_retry",
            "attempt": attempt,
            "next_attempt": attempt + 1,
            "trigger": trigger,
            "compile_errors": report.compile_errors.clone(),
            "primary_reason": eval_events::body_snippet(&report.primary_reason()),
        }),
    );
}

pub(super) fn obligation_repair_target_paths(report: &VerificationReport) -> Vec<String> {
    report
        .profile_failures
        .iter()
        .filter_map(|failure| {
            failure
                .strip_prefix("missing_required_obligation_target:")
                .and_then(|rest| rest.split_once(':'))
                .map(|(_, path)| path.trim().to_string())
                .filter(|path| !path.is_empty())
        })
        .collect()
}

pub(super) fn final_acceptance_pending_evidence_guidance(
    root: &Path,
    profile: &str,
    report: &VerificationReport,
) -> String {
    let pending_keys = verification_missing_signals(report);
    if pending_keys.is_empty() {
        return "- none".to_string();
    }
    let mut lines = capability_evidence_failure_evidence(
        root,
        profile,
        &pending_keys,
        &capability_evidence_unresolved_reason(&pending_keys).unwrap_or_default(),
    );
    if lines.is_empty() {
        lines = capability_evidence_remedy_lines(&pending_keys);
    }
    render_prompt_bullets(&lines)
}

#[cfg(test)]
#[allow(clippy::too_many_arguments)]
pub(super) fn final_acceptance_repair_prompt(
    root: &Path,
    layout: PromptLayout,
    plan: &UltraPlan,
    report: &VerificationReport,
    context: &UltraRunContext,
    repair_target: &str,
    expected_paths: &[String],
    adherence_missing: &[String],
    repair_budget: (usize, usize),
    compile_reanchored_retry: bool,
    compile_narrow_no_snapshot_retry: bool,
) -> String {
    final_acceptance_repair_prompt_with_events(
        root,
        layout,
        plan,
        report,
        context,
        repair_target,
        expected_paths,
        adherence_missing,
        repair_budget,
        compile_reanchored_retry,
        compile_narrow_no_snapshot_retry,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn final_acceptance_repair_prompt_with_events(
    root: &Path,
    layout: PromptLayout,
    plan: &UltraPlan,
    report: &VerificationReport,
    context: &UltraRunContext,
    repair_target: &str,
    expected_paths: &[String],
    adherence_missing: &[String],
    repair_budget: (usize, usize),
    compile_reanchored_retry: bool,
    compile_narrow_no_snapshot_retry: bool,
    eval_events_path: Option<&Path>,
) -> String {
    match layout {
        PromptLayout::Stable => final_acceptance_repair_prompt_stable(
            root,
            plan,
            report,
            context,
            repair_target,
            expected_paths,
            adherence_missing,
            repair_budget,
            compile_reanchored_retry,
            compile_narrow_no_snapshot_retry,
            eval_events_path,
        ),
        PromptLayout::Legacy => final_acceptance_repair_prompt_legacy(
            root,
            plan,
            report,
            context,
            repair_target,
            expected_paths,
            adherence_missing,
            repair_budget,
            compile_reanchored_retry,
            compile_narrow_no_snapshot_retry,
            eval_events_path,
        ),
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn final_acceptance_repair_prompt_stable(
    root: &Path,
    plan: &UltraPlan,
    report: &VerificationReport,
    context: &UltraRunContext,
    repair_target: &str,
    expected_paths: &[String],
    adherence_missing: &[String],
    repair_budget: (usize, usize),
    compile_reanchored_retry: bool,
    compile_narrow_no_snapshot_retry: bool,
    eval_events_path: Option<&Path>,
) -> String {
    let (attempt, max_attempts) = repair_budget;
    let expected = render_prompt_bullets(expected_paths);
    let missing = render_prompt_bullets(&report.missing_paths);
    let dependencies = render_prompt_bullets(&report.dependency_missing);
    let profile_failures = final_acceptance_model_fixable_profile_failures(report);
    let profile_failures = render_prompt_bullets(&profile_failures);
    let adherence_guidance =
        final_acceptance_adherence_guidance(report, repair_target, adherence_missing);
    let behavioral_probe_context =
        final_acceptance_behavioral_probe_context(report, expected_paths);
    let restart_attachment_guidance =
        final_acceptance_restart_attachment_guidance(root, plan, report);
    let pending_evidence_guidance =
        final_acceptance_pending_evidence_guidance(root, &plan.profile, report);
    let state_binding_guidance = crate::planner::state_binding_scan::final_acceptance_feedback(
        root,
        &plan.profile,
        report,
        eval_events_path,
    );
    let state_binding_guidance = if state_binding_guidance.is_empty() {
        String::new()
    } else {
        format!("State binding repair guidance:\n{state_binding_guidance}\n\n")
    };
    let command_failures = command_failure_summaries(report);
    let command_failures = render_prompt_bullets(&command_failures);
    let compile_errors = compile_repair_prompt_section_with_root(
        Some(root),
        &report.compile_errors,
        CompileRepairPromptProtection {
            reanchored_retry: compile_reanchored_retry,
            narrow_no_snapshot_retry: compile_narrow_no_snapshot_retry,
        },
    );
    format!(
        "Repair the final acceptance failure for the current ultra run.\n\n\
Bounded repair rules:\n\
- This is a bounded final acceptance repair, not a new planning cycle.\n\
- Repair the concrete missing or failed acceptance obligations without weakening verification, package scripts, or profile contracts.\n\
- If a scaffold exists, continue task-specific implementation instead of treating scaffold/build-only output as complete.\n\
- Prefer the smallest necessary file changes, then stop.\n\n\
Original ultra goal:\n{goal}\n\n\
Profile: {profile}\nIntent: {intent}\n\n\
Pending capability evidence remedies:\n{pending_evidence_guidance}\n\n\
{state_binding_guidance}\
Missing paths:\n{missing}\n\n\
Dependency failures:\n{dependencies}\n\n\
Compile errors:\n{compile_errors}\n\n\
Command failures:\n{command_failures}\n\n\
Profile failures:\n{profile_failures}\n\n\
{behavioral_probe_context}\
{restart_attachment_guidance}\
{adherence_guidance}\
Expected paths to preserve or create:\n{expected}\n\n\
{prior_context}\n\n\
Current objective: repair final acceptance for {goal}; primary reason {primary_reason}; target {repair_target}.\n\n\
Final acceptance failure:\n\
- primary reason: {primary_reason}\n\
- repair target: {repair_target}\n\
- attempt: {attempt}/{max_attempts}",
        goal = plan.goal,
        profile = plan.profile,
        intent = plan.intent,
        primary_reason = report.primary_reason(),
        repair_target = repair_target,
        attempt = attempt,
        max_attempts = max_attempts,
        pending_evidence_guidance = pending_evidence_guidance,
        state_binding_guidance = state_binding_guidance,
        missing = missing,
        dependencies = dependencies,
        compile_errors = compile_errors,
        command_failures = command_failures,
        profile_failures = profile_failures,
        behavioral_probe_context = behavioral_probe_context,
        restart_attachment_guidance = restart_attachment_guidance,
        adherence_guidance = adherence_guidance,
        expected = expected,
        prior_context = context.render_prompt_section(),
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn final_acceptance_repair_prompt_legacy(
    root: &Path,
    plan: &UltraPlan,
    report: &VerificationReport,
    context: &UltraRunContext,
    repair_target: &str,
    expected_paths: &[String],
    adherence_missing: &[String],
    repair_budget: (usize, usize),
    compile_reanchored_retry: bool,
    compile_narrow_no_snapshot_retry: bool,
    eval_events_path: Option<&Path>,
) -> String {
    let (attempt, max_attempts) = repair_budget;
    let expected = render_prompt_bullets(expected_paths);
    let missing = render_prompt_bullets(&report.missing_paths);
    let dependencies = render_prompt_bullets(&report.dependency_missing);
    let profile_failures = final_acceptance_model_fixable_profile_failures(report);
    let profile_failures = render_prompt_bullets(&profile_failures);
    let adherence_guidance =
        final_acceptance_adherence_guidance(report, repair_target, adherence_missing);
    let behavioral_probe_context =
        final_acceptance_behavioral_probe_context(report, expected_paths);
    let restart_attachment_guidance =
        final_acceptance_restart_attachment_guidance(root, plan, report);
    let pending_evidence_guidance =
        final_acceptance_pending_evidence_guidance(root, &plan.profile, report);
    let state_binding_guidance = crate::planner::state_binding_scan::final_acceptance_feedback(
        root,
        &plan.profile,
        report,
        eval_events_path,
    );
    let state_binding_guidance = if state_binding_guidance.is_empty() {
        String::new()
    } else {
        format!("State binding repair guidance:\n{state_binding_guidance}\n\n")
    };
    let command_failures = command_failure_summaries(report);
    let command_failures = render_prompt_bullets(&command_failures);
    let compile_errors = compile_repair_prompt_section_with_root(
        Some(root),
        &report.compile_errors,
        CompileRepairPromptProtection {
            reanchored_retry: compile_reanchored_retry,
            narrow_no_snapshot_retry: compile_narrow_no_snapshot_retry,
        },
    );
    format!(
        "Repair the final acceptance failure for the current ultra run.\n\n\
Original ultra goal:\n{goal}\n\n\
Profile: {profile}\nIntent: {intent}\n\n\
Final acceptance failure:\n\
- primary reason: {primary_reason}\n\
- repair target: {repair_target}\n\
- attempt: {attempt}/{max_attempts}\n\n\
Pending capability evidence remedies:\n{pending_evidence_guidance}\n\n\
{state_binding_guidance}\
Missing paths:\n{missing}\n\n\
Dependency failures:\n{dependencies}\n\n\
Compile errors:\n{compile_errors}\n\n\
Command failures:\n{command_failures}\n\n\
Profile failures:\n{profile_failures}\n\n\
{behavioral_probe_context}\
{restart_attachment_guidance}\
{adherence_guidance}\
Expected paths to preserve or create:\n{expected}\n\n\
{prior_context}\n\n\
Bounded repair rules:\n\
- This is a bounded final acceptance repair, not a new planning cycle.\n\
- Repair the concrete missing or failed acceptance obligations without weakening verification, package scripts, or profile contracts.\n\
- If a scaffold exists, continue task-specific implementation instead of treating scaffold/build-only output as complete.\n\
- Prefer the smallest necessary file changes, then stop.",
        goal = plan.goal,
        profile = plan.profile,
        intent = plan.intent,
        primary_reason = report.primary_reason(),
        repair_target = repair_target,
        attempt = attempt,
        max_attempts = max_attempts,
        pending_evidence_guidance = pending_evidence_guidance,
        state_binding_guidance = state_binding_guidance,
        missing = missing,
        dependencies = dependencies,
        compile_errors = compile_errors,
        command_failures = command_failures,
        profile_failures = profile_failures,
        behavioral_probe_context = behavioral_probe_context,
        restart_attachment_guidance = restart_attachment_guidance,
        adherence_guidance = adherence_guidance,
        expected = expected,
        prior_context = context.render_prompt_section(),
    )
}

pub(super) fn final_acceptance_restart_attachment_guidance(
    root: &Path,
    plan: &UltraPlan,
    report: &VerificationReport,
) -> String {
    if !verification_missing_signals(report)
        .iter()
        .any(|key| key == "restart_or_recoverable_state_evidence")
    {
        return String::new();
    }
    let guidance = restart_hook_attachment_guidance(root, &plan.profile);
    if guidance.is_empty() {
        return String::new();
    }
    format!(
        "Restart hook attachment guidance:\n{}\n\n",
        render_prompt_bullets(&guidance)
    )
}

pub(super) fn interaction_root_cause_repair_guidance(
    failure_kind: &str,
    evidence: Option<&Value>,
) -> Vec<String> {
    if !interaction_render_loop_failure(failure_kind) {
        return Vec::new();
    }
    let mut lines = evidence
        .map(unattached_ref_guidance_lines)
        .unwrap_or_default();
    lines.push(
        "render-loop checklist: ref attached -> effect runs -> rAF loop starts -> draw calls"
            .to_string(),
    );
    lines.push(format!(
        "input-wiring checklist: {INTERACTION_STATE_CHANGE_REPAIR_REQUIREMENT}"
    ));
    dedup_strings(lines)
}

pub(super) fn interaction_render_loop_failure(failure_kind: &str) -> bool {
    let lower = failure_kind.to_ascii_lowercase();
    lower.contains("input_state_change_missing_after_start") || lower.contains("canvas_blank")
}

pub(super) fn unattached_ref_guidance_lines(value: &Value) -> Vec<String> {
    raw_value_scopes(value)
        .into_iter()
        .find_map(|scope| {
            scope
                .get("unattached_ref_diagnostics")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(unattached_ref_guidance_line)
                        .collect::<Vec<_>>()
                })
                .filter(|items| !items.is_empty())
        })
        .unwrap_or_default()
}

pub(super) fn unattached_ref_guidance_line(value: &Value) -> Option<String> {
    if let Some(guidance) = value
        .get("guidance")
        .and_then(Value::as_str)
        .filter(|guidance| !guidance.trim().is_empty())
    {
        return Some(guidance.trim().to_string());
    }
    let name = value.get("name").and_then(Value::as_str)?;
    let candidate = value
        .get("candidate_elements")
        .and_then(Value::as_array)?
        .first()?;
    let tag = candidate.get("tag").and_then(Value::as_str)?;
    let source = candidate.get("source").and_then(Value::as_str)?;
    let line = candidate.get("line").and_then(Value::as_u64)?;
    let file = Path::new(source)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(source);
    Some(format!(
        "attach ref={{{name}}} to the <{tag}> at {file}:{line}"
    ))
}

pub(super) fn final_acceptance_behavioral_probe_context(
    report: &VerificationReport,
    expected_paths: &[String],
) -> String {
    let Some(failure_kind) = final_acceptance_app_behavior_failure_kind(report) else {
        return String::new();
    };
    let evidence = interaction_probe_json_from_report(report);
    let dispatched_inputs = evidence
        .as_ref()
        .map(|value| raw_string_array_field_deep(value, "input_dispatches"))
        .filter(|values| !values.is_empty())
        .unwrap_or_else(|| {
            vec![
                "ArrowLeft keydown".to_string(),
                "ArrowRight keydown".to_string(),
                "Space keydown".to_string(),
                "canvas/center click".to_string(),
            ]
        });
    let mut lines = interaction_root_cause_repair_guidance(&failure_kind, evidence.as_ref());
    lines.extend([
        "Interaction probe context:".to_string(),
        format!("- failure kind: {failure_kind}"),
        format!("- dispatched inputs: {}", dispatched_inputs.join(", ")),
    ]);
    if let Some(value) = evidence.as_ref() {
        if let Some(blank) = raw_bool_field_deep(value, "canvas_blank_after_start") {
            lines.push(format!("- canvas blank after start: {blank}"));
        }
        if let Some(blank) = raw_bool_field_deep(value, "canvas_blank_after_inputs") {
            lines.push(format!("- canvas blank after inputs: {blank}"));
        }
        if let Some(mode) =
            raw_text_field_deep(value, &["probe_mode"]).filter(|mode| !mode.is_empty())
        {
            lines.push(format!("- probe mode: {mode}"));
        }
        if let Some(status) = raw_text_field_deep(value, &["contract_hook_status"])
            .filter(|status| !status.is_empty())
        {
            lines.push(format!("- contract hook status: {status}"));
        }
        if let Some(restart_present) = raw_contract_hook_bool(value, "restart_present") {
            lines.push(format!("- restart hook present: {restart_present}"));
        }
        if let Some(restart_reachable) =
            raw_bool_field_deep(value, "restart_hook_reachable_after_start")
        {
            lines.push(format!(
                "- restart hook reachable after start: {restart_reachable}"
            ));
        }
        let action_hooks = raw_string_array_field_deep(value, "action_hooks");
        if !action_hooks.is_empty() {
            lines.push(format!("- action hooks: {}", action_hooks.join(", ")));
        }
        if let Some(status) =
            raw_text_field_deep(value, &["text_entry"]).filter(|status| !status.is_empty())
        {
            lines.push(format!("- text entry: {status}"));
        }
        if let Some(target) =
            raw_text_field_deep(value, &["text_entry_target"]).filter(|target| !target.is_empty())
        {
            lines.push(format!("- text entry target: {target}"));
        }
        if let Some(token) =
            raw_text_field_deep(value, &["typed_token"]).filter(|token| !token.is_empty())
        {
            lines.push(format!("- typed token: {token}"));
        }
        if let Some(echoed) = raw_bool_field_deep(value, "token_echoed") {
            lines.push(format!("- token echoed outside input: {echoed}"));
        }
        if let Some(latency) = raw_u64_field_deep(value, "echo_latency_ms") {
            lines.push(format!("- token echo latency ms: {latency}"));
        }
        if let Some(after_reload) = raw_bool_field_deep(value, "token_echoed_after_reload") {
            lines.push(format!("- token echoed after reload: {after_reload}"));
        }
        if let Some(latency) = raw_u64_field_deep(value, "token_echo_after_reload_latency_ms") {
            lines.push(format!("- token echo after reload latency ms: {latency}"));
        }
        if let Some(changed) = raw_bool_field_deep(value, "text_input_state_change") {
            lines.push(format!("- text input state change: {changed}"));
        }
        for line in surface_fit_guidance_lines_from_value(value) {
            lines.push(format!("- surface fit guidance: {line}"));
        }
        let state_dimensions = raw_string_array_field_deep(value, "state_dimensions_changed");
        if !state_dimensions.is_empty() {
            lines.push(format!(
                "- state dimensions changed: {}",
                state_dimensions.join(", ")
            ));
        }
        if let Some(status) = raw_text_field_deep(value, &["persistence_after_reload"])
            .filter(|status| !status.is_empty())
        {
            lines.push(format!("- persistence after reload: {status}"));
        }
        if let Some(reason) = raw_text_field_deep(value, &["persistence_after_reload_reason"])
            .filter(|reason| !reason.is_empty())
        {
            lines.push(format!("- persistence after reload reason: {reason}"));
        }
        let persisted_dimensions =
            raw_string_array_field_deep(value, "persistence_changed_dimensions");
        if !persisted_dimensions.is_empty() {
            lines.push(format!(
                "- persistence dimensions checked: {}",
                persisted_dimensions.join(", ")
            ));
        }
        let info = raw_string_array_field_deep(value, "informational_failure_kinds");
        if !info.is_empty() {
            lines.push(format!(
                "- informational probe findings: {}",
                info.join(", ")
            ));
        }
        let candidates = interaction_candidate_prompt_lines(value);
        if !candidates.is_empty() {
            lines.push("- candidate table:".to_string());
            lines.extend(candidates);
        }
        for key in [
            "before_marker",
            "after_marker",
            "input_before_marker",
            "input_after_marker",
            "recovery_before_marker",
            "recovery_after_marker",
        ] {
            if let Some(marker) =
                raw_text_field_deep(value, &[key]).filter(|marker| !marker.is_empty())
            {
                lines.push(format!("- {key}: {}", prompt_marker_excerpt(&marker)));
            }
        }
    }
    if failure_kind.contains("start_transition_missing") {
        lines.push(format!(
            "- concrete requirement: {INTERACTION_START_REPAIR_REQUIREMENT}"
        ));
    }
    if failure_kind.contains("interaction_state_change_missing")
        || failure_kind.contains("input_state_change_missing_after_start")
        || failure_kind.contains("input_state_change_not_evaluated_after_start")
        || failure_kind.contains("text_input_state_change_missing")
    {
        lines.push(format!(
            "- concrete requirement: {INTERACTION_STATE_CHANGE_REPAIR_REQUIREMENT}"
        ));
    }
    if failure_kind.contains("persistence_after_reload_reset") {
        lines.push(format!(
            "- concrete requirement: {PERSISTENCE_RELOAD_REPAIR_REQUIREMENT}"
        ));
    }
    if failure_kind.contains("token_echo_after_reload_only") {
        lines.push(format!(
            "- concrete requirement: {TEXT_ECHO_AFTER_RELOAD_REPAIR_REQUIREMENT}"
        ));
    } else if failure_kind.contains("token_echo_missing") {
        lines.push(format!(
            "- concrete requirement: {TEXT_ECHO_REPAIR_REQUIREMENT}"
        ));
    }
    let route_paths = route_bound_implementation_paths(expected_paths);
    if !route_paths.is_empty() {
        lines.push("Route-bound implementation targets:".to_string());
        lines.extend(route_paths.into_iter().map(|path| format!("- {path}")));
    }
    format!("{}\n\n", lines.join("\n"))
}

pub(super) fn final_acceptance_concrete_interaction_requirement(
    failure_kind: &str,
) -> Option<&'static str> {
    if failure_kind.contains("interaction_state_change_missing")
        || failure_kind.contains("input_state_change_missing_after_start")
        || failure_kind.contains("input_state_change_not_evaluated_after_start")
        || failure_kind.contains("text_input_state_change_missing")
    {
        Some(INTERACTION_STATE_CHANGE_REPAIR_REQUIREMENT)
    } else if failure_kind.contains("start_transition_missing") {
        Some(INTERACTION_START_REPAIR_REQUIREMENT)
    } else if failure_kind.contains("persistence_after_reload_reset") {
        Some(PERSISTENCE_RELOAD_REPAIR_REQUIREMENT)
    } else if failure_kind.contains("token_echo_after_reload_only") {
        Some(TEXT_ECHO_AFTER_RELOAD_REPAIR_REQUIREMENT)
    } else if failure_kind.contains("token_echo_missing") {
        Some(TEXT_ECHO_REPAIR_REQUIREMENT)
    } else {
        None
    }
}

pub(super) fn final_acceptance_recovery_reason(
    report: &VerificationReport,
    reason: &str,
    exhausted_reason: &str,
) -> String {
    let pending_keys = verification_missing_signals(report);
    if let Some(pending_reason) = capability_evidence_unresolved_reason(&pending_keys) {
        let mut out =
            format!("{pending_reason}; final acceptance repair stopped: {exhausted_reason}");
        if !reason.contains(&pending_reason) && !reason.trim().is_empty() {
            out.push_str("; ");
            out.push_str(reason);
        }
        if let Some(failure_kind) = final_acceptance_app_behavior_failure_kind(report) {
            out.push_str("; behavioral probe reason: ");
            out.push_str(&failure_kind);
            if let Some(requirement) =
                final_acceptance_concrete_interaction_requirement(&failure_kind)
            {
                out.push_str("\nconcrete requirement: ");
                out.push_str(requirement);
            }
        }
        for remedy in capability_evidence_remedy_lines(&pending_keys) {
            out.push_str("\nremedy: ");
            out.push_str(&remedy);
        }
        let context = final_acceptance_behavioral_probe_context(report, &[]);
        if !context.is_empty() {
            out.push('\n');
            out.push_str(context.trim());
        }
        return out;
    }
    let Some(failure_kind) = final_acceptance_app_behavior_failure_kind(report) else {
        return format!("{reason}; final acceptance repair stopped: {exhausted_reason}");
    };
    let repair_targets = interaction_repair_targets_for_reason(&failure_kind).join(", ");
    let mut out = format!(
        "{failure_kind}; final acceptance repair stopped: {exhausted_reason}; repair target: {repair_targets}"
    );
    if failure_kind.contains("interaction_state_change_missing")
        || failure_kind.contains("input_state_change_missing_after_start")
        || failure_kind.contains("input_state_change_not_evaluated_after_start")
        || failure_kind.contains("text_input_state_change_missing")
    {
        out.push_str("; ");
        out.push_str(INTERACTION_STATE_CHANGE_REPAIR_REQUIREMENT);
    } else if failure_kind.contains("start_transition_missing") {
        out.push_str("; ");
        out.push_str(INTERACTION_START_REPAIR_REQUIREMENT);
    } else if failure_kind.contains("persistence_after_reload_reset") {
        out.push_str("; ");
        out.push_str(PERSISTENCE_RELOAD_REPAIR_REQUIREMENT);
    } else if failure_kind.contains("token_echo_after_reload_only") {
        out.push_str("; ");
        out.push_str(TEXT_ECHO_AFTER_RELOAD_REPAIR_REQUIREMENT);
    } else if failure_kind.contains("token_echo_missing") {
        out.push_str("; ");
        out.push_str(TEXT_ECHO_REPAIR_REQUIREMENT);
    }
    let context = final_acceptance_behavioral_probe_context(report, &[]);
    if !context.is_empty() {
        out.push('\n');
        out.push_str(context.trim());
    }
    out
}

pub(super) fn final_acceptance_recovery_repair_targets(
    report: &VerificationReport,
    fallback: RepairTarget,
) -> Vec<String> {
    let mut targets = if let Some(reason) = final_acceptance_app_behavior_failure_kind(report) {
        interaction_repair_targets_for_reason(&reason)
    } else {
        vec![fallback.as_str().to_string()]
    };
    if !report.compile_errors.is_empty()
        && !targets.iter().any(|target| target == "fix_compile_error")
    {
        targets.insert(0, "fix_compile_error".to_string());
    }
    targets
}

pub(super) fn final_acceptance_recovery_failure_evidence(
    report: &VerificationReport,
    reason: &str,
) -> Vec<String> {
    final_acceptance_recovery_failure_evidence_base(None, "", report, reason)
}

pub(super) fn final_acceptance_recovery_failure_evidence_with_context(
    root: &Path,
    profile: &str,
    report: &VerificationReport,
    reason: &str,
) -> Vec<String> {
    final_acceptance_recovery_failure_evidence_base(Some(root), profile, report, reason)
}

pub(super) fn final_acceptance_recovery_failure_evidence_base(
    root: Option<&Path>,
    profile: &str,
    report: &VerificationReport,
    reason: &str,
) -> Vec<String> {
    let pending_keys = verification_missing_signals(report);
    let failure_kind = final_acceptance_app_behavior_failure_kind(report).unwrap_or_default();
    let probe_json = interaction_probe_json_from_report(report);
    let mut evidence = interaction_root_cause_repair_guidance(&failure_kind, probe_json.as_ref());
    let pending_evidence = if let Some(root) = root.filter(|_| !pending_keys.is_empty()) {
        capability_evidence_failure_evidence(root, profile, &pending_keys, reason)
    } else {
        capability_evidence_remedy_lines(&pending_keys)
    };
    merge_unique_strings(&mut evidence, &pending_evidence);
    evidence.extend(
        compile_error_repair_guidance(&report.compile_errors)
            .into_iter()
            .map(|line| format!("fix_compile_error: {line}")),
    );
    if !reason.trim().is_empty() {
        evidence.push(reason.to_string());
    }
    evidence
}

pub(super) fn route_bound_implementation_paths(expected_paths: &[String]) -> Vec<String> {
    let mut paths = expected_paths
        .iter()
        .filter(|path| {
            let lower = path.to_ascii_lowercase();
            lower.contains("src/app/")
                || lower.contains("app/page")
                || lower.contains("pages/")
                || lower.ends_with(".tsx")
                || lower.ends_with(".jsx")
                || lower.ends_with(".ts")
                || lower.ends_with(".js")
        })
        .cloned()
        .collect::<Vec<_>>();
    if paths.is_empty() {
        paths.extend(expected_paths.iter().cloned());
    }
    dedup_strings(paths)
}

pub(super) fn interaction_probe_json_from_report(report: &VerificationReport) -> Option<Value> {
    report
        .profile_failures
        .iter()
        .find_map(|failure| failure.strip_prefix("interaction evidence path: "))
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
}

pub(super) fn interaction_state_dimensions_changed_from_path(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .map(|value| raw_string_array_field_deep(&value, "state_dimensions_changed"))
        .unwrap_or_default()
}

pub(super) fn interaction_action_hooks_from_path(path: &str) -> Vec<String> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
        .map(|value| raw_string_array_field_deep(&value, "action_hooks"))
        .unwrap_or_default()
}

#[derive(Debug, Clone, Default)]
pub(super) struct InteractionSurfaceFitTelemetry {
    pub(super) raw: Option<Value>,
    pub(super) summary: String,
    pub(super) guidance: String,
}

pub(super) fn interaction_surface_fit_from_path(path: &str) -> InteractionSurfaceFitTelemetry {
    let Some(value) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
    else {
        return InteractionSurfaceFitTelemetry::default();
    };
    interaction_surface_fit_from_value(&value)
}

pub(super) fn interaction_surface_fit_from_value(value: &Value) -> InteractionSurfaceFitTelemetry {
    let Some(fit) = raw_surface_fit_value(value) else {
        return InteractionSurfaceFitTelemetry::default();
    };
    let surface = fit
        .get("surface")
        .and_then(Value::as_str)
        .filter(|surface| !surface.trim().is_empty())
        .unwrap_or("surface");
    let fits = fit
        .get("fits_viewport")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let overflows = surface_fit_overflows(fit);
    let raw = Some(fit.clone());
    let summary = if fits {
        format!("{surface} fits viewport")
    } else {
        format!(
            "{surface} overflows viewport ({})",
            surface_fit_edge_summary(&overflows)
        )
    };
    let guidance = surface_fit_guidance(surface, fits, &overflows);
    InteractionSurfaceFitTelemetry {
        raw,
        summary,
        guidance,
    }
}

pub(super) fn raw_surface_fit_value(value: &Value) -> Option<&Value> {
    raw_value_scopes(value)
        .into_iter()
        .find_map(|scope| scope.get("surface_fit").filter(|fit| fit.is_object()))
}

pub(super) fn surface_fit_overflows(fit: &Value) -> BTreeMap<&'static str, i64> {
    [
        ("top", "overflow_top_px"),
        ("right", "overflow_right_px"),
        ("bottom", "overflow_bottom_px"),
        ("left", "overflow_left_px"),
    ]
    .into_iter()
    .map(|(edge, key)| (edge, raw_i64_field(fit, key).unwrap_or(0).max(0)))
    .collect()
}

pub(super) fn raw_i64_field(value: &Value, name: &str) -> Option<i64> {
    value.get(name).and_then(Value::as_i64).or_else(|| {
        value
            .get(name)
            .and_then(Value::as_f64)
            .map(|value| value.round() as i64)
    })
}

pub(super) fn surface_fit_edge_summary(overflows: &BTreeMap<&'static str, i64>) -> String {
    overflows
        .iter()
        .filter(|(_, px)| **px > 0)
        .map(|(edge, px)| format!("{edge}: {px}px"))
        .collect::<Vec<_>>()
        .join(", ")
}

pub(super) fn surface_fit_guidance(
    surface: &str,
    fits: bool,
    overflows: &BTreeMap<&'static str, i64>,
) -> String {
    if fits {
        return String::new();
    }
    let max_overflow = overflows.values().copied().max().unwrap_or(0);
    if max_overflow <= 0 {
        return String::new();
    }
    format!("{surface} overflows the viewport by {max_overflow}px; consider responsive sizing")
}

pub(super) fn surface_fit_guidance_lines_from_value(value: &Value) -> Vec<String> {
    let guidance = interaction_surface_fit_from_value(value).guidance;
    if guidance.is_empty() {
        Vec::new()
    } else {
        vec![guidance]
    }
}

#[derive(Debug, Clone, Default)]
pub(super) struct InteractionTextTelemetry {
    pub(super) text_entry: String,
    pub(super) text_entry_target: String,
    pub(super) typed_token: String,
    pub(super) token_echoed: Option<bool>,
    pub(super) echo_latency_ms: Option<u64>,
    pub(super) token_echoed_after_reload: Option<bool>,
    pub(super) token_echo_after_reload_latency_ms: Option<u64>,
    pub(super) text_input_state_change: Option<bool>,
}

pub(super) fn interaction_text_telemetry_from_path(path: &str) -> InteractionTextTelemetry {
    let Some(value) = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
    else {
        return InteractionTextTelemetry::default();
    };
    InteractionTextTelemetry {
        text_entry: raw_text_field_deep(&value, &["text_entry"]).unwrap_or_default(),
        text_entry_target: raw_text_field_deep(&value, &["text_entry_target"]).unwrap_or_default(),
        typed_token: raw_text_field_deep(&value, &["typed_token"]).unwrap_or_default(),
        token_echoed: raw_bool_field_deep(&value, "token_echoed"),
        echo_latency_ms: raw_u64_field_deep(&value, "echo_latency_ms"),
        token_echoed_after_reload: raw_bool_field_deep(&value, "token_echoed_after_reload"),
        token_echo_after_reload_latency_ms: raw_u64_field_deep(
            &value,
            "token_echo_after_reload_latency_ms",
        ),
        text_input_state_change: raw_bool_field_deep(&value, "text_input_state_change"),
    }
}

pub(super) fn prompt_marker_excerpt(value: &str) -> String {
    const MAX_CHARS: usize = 600;
    if value.chars().count() <= MAX_CHARS {
        return value.to_string();
    }
    let mut excerpt = value.chars().take(MAX_CHARS).collect::<String>();
    excerpt.push_str("[truncated]");
    excerpt
}

pub(super) fn raw_text_field_deep(value: &Value, names: &[&str]) -> Option<String> {
    for scope in raw_value_scopes(value) {
        for name in names {
            if let Some(found) = scope.get(*name).and_then(Value::as_str) {
                return Some(found.to_string());
            }
        }
    }
    None
}

pub(super) fn raw_string_array_field_deep(value: &Value, name: &str) -> Vec<String> {
    raw_value_scopes(value)
        .into_iter()
        .find_map(|scope| {
            scope
                .get(name)
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect::<Vec<_>>()
                })
                .filter(|items| !items.is_empty())
        })
        .unwrap_or_default()
}

pub(super) fn raw_contract_hook_bool(value: &Value, name: &str) -> Option<bool> {
    raw_value_scopes(value).into_iter().find_map(|scope| {
        scope
            .get("contract_hooks")
            .and_then(|hooks| hooks.get(name))
            .and_then(Value::as_bool)
    })
}

pub(super) fn raw_bool_field_deep(value: &Value, name: &str) -> Option<bool> {
    raw_value_scopes(value)
        .into_iter()
        .find_map(|scope| scope.get(name).and_then(Value::as_bool))
}

pub(super) fn raw_u64_field_deep(value: &Value, name: &str) -> Option<u64> {
    raw_value_scopes(value)
        .into_iter()
        .find_map(|scope| scope.get(name).and_then(Value::as_u64))
}

pub(super) fn interaction_candidate_prompt_lines(value: &Value) -> Vec<String> {
    raw_value_scopes(value)
        .into_iter()
        .find_map(|scope| scope.get("candidate_table").and_then(Value::as_array))
        .into_iter()
        .flatten()
        .take(4)
        .map(|candidate| {
            let rank = candidate
                .get("rank")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let text = candidate
                .get("text_excerpt")
                .and_then(Value::as_str)
                .unwrap_or("");
            let changed = candidate
                .get("changed")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            format!(
                "- rank {rank}: text=\"{}\" changed={changed}",
                prompt_marker_excerpt(text)
            )
        })
        .collect()
}

pub(super) fn raw_value_scopes(value: &Value) -> Vec<&Value> {
    let mut scopes = vec![value];
    if let Some(details) = value.get("details").filter(|details| details.is_object()) {
        scopes.push(details);
    }
    if let Some(details) = value
        .get("browser_details")
        .filter(|details| details.is_object())
    {
        scopes.push(details);
    }
    scopes
}

pub(super) fn render_requested_features_not_detected_line(missing: &[String]) -> String {
    if missing.is_empty() {
        "Requested features not yet detected: none".to_string()
    } else {
        render_bounded_prompt_section(
            "Requested features not yet detected:",
            missing,
            None,
            ULTRA_PROMPT_GUIDANCE_MAX_LINES,
        )
    }
}

pub(super) fn final_acceptance_adherence_guidance(
    report: &VerificationReport,
    repair_target: &str,
    adherence_missing: &[String],
) -> String {
    if adherence_missing.is_empty()
        || !final_acceptance_repairs_contract_evidence(report, repair_target)
    {
        return String::new();
    }
    format!(
        "Secondary plan-adherence guidance:\n- also close if in scope: {}\n\n",
        adherence_missing.join(", ")
    )
}

pub(super) fn final_acceptance_repairs_contract_evidence(
    report: &VerificationReport,
    repair_target: &str,
) -> bool {
    repair_target == RepairTarget::RequiredEvidenceMissing.as_str()
        || final_acceptance_model_fixable_profile_failures(report)
            .iter()
            .any(|failure| {
                let lower = failure.to_ascii_lowercase();
                lower.contains("missing_required_evidence")
                    || lower.contains("weak_verification_evidence")
                    || lower.contains("required evidence missing")
            })
}

pub(super) fn final_acceptance_model_fixable_profile_failures(
    report: &VerificationReport,
) -> Vec<String> {
    report
        .profile_failures
        .iter()
        .filter(|failure| {
            let lower = failure.to_ascii_lowercase();
            !lower.contains("browser_interaction_evidence_required")
                && !lower.contains("interaction_evidence_missing")
                && !lower.contains("interaction evidence status:")
                && !lower.contains("interaction evidence path:")
                && !lower.contains("interaction_unverified:probe_unavailable")
                && !lower.contains("unverified:")
                && !lower.contains("probe_dependency_missing")
                && !lower.contains("probe_infrastructure_failed")
                && !lower.contains("app interaction untested")
        })
        .cloned()
        .collect()
}

pub(super) fn render_prompt_bullets(items: &[String]) -> String {
    if items.is_empty() {
        "- none".to_string()
    } else {
        items
            .iter()
            .map(|item| format!("- {item}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub(super) fn run_profile_repair_with_ultra_session(
    execution: &mut dyn ChatClient,
    ultra_session: &mut SessionSnapshot,
    repair_prompt: &str,
    expected_paths: &[String],
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<RunSessionOutcome> {
    run_session_with_outcome_with_options(
        execution,
        ultra_session,
        repair_prompt,
        expected_paths,
        config,
        ui,
        RunSessionOptions::plan_step(RunSessionStepKind::Implement),
    )
}

pub(super) fn run_final_acceptance_repair_with_ultra_session(
    execution: &mut dyn ChatClient,
    ultra_session: &mut SessionSnapshot,
    repair_prompt: &str,
    expected_paths: &[String],
    config: &Config,
    ui: &dyn InteractionUi,
) -> anyhow::Result<RunSessionOutcome> {
    run_session_with_outcome_with_options(
        execution,
        ultra_session,
        repair_prompt,
        expected_paths,
        config,
        ui,
        RunSessionOptions::final_acceptance_repair(),
    )
}

pub(super) fn compact_workspace_snapshot(root: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(root) else {
        return "- unavailable".to_string();
    };
    let mut names = entries
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().to_string())
        .filter(|name| !matches!(name.as_str(), ".git" | ".anvil" | "target" | ".DS_Store"))
        .take(12)
        .collect::<Vec<_>>();
    names.sort();
    if names.is_empty() {
        "- empty or metadata-only".to_string()
    } else {
        names
            .into_iter()
            .map(|name| format!("- {name}"))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

pub(super) fn acceptance_dependency_deterministic_reconcile_needed(
    config: &Config,
    report: &VerificationReport,
) -> bool {
    if !dependency_setup::package_json_declares_dependencies(&config.workspace_root) {
        return false;
    }
    verification_report_mentions_dependency_setup_missing(report)
}

pub(super) fn ultra_contract_runtime_acceptance_report(
    plan: &UltraPlan,
    config: &Config,
) -> anyhow::Result<RuntimeAcceptanceReport> {
    let effective_profile = canonical_profile_name(&plan.profile);
    let mut required_paths =
        profile_expected_paths(&config.workspace_root, &effective_profile, &plan.goal);
    let mut required_capabilities = inferred_required_capabilities(&effective_profile, &plan.goal);
    let mut required_obligations =
        inferred_required_obligations(&effective_profile, &plan.goal, &required_capabilities);
    let mut required_evidence =
        inferred_required_evidence(&effective_profile, &plan.goal, &required_capabilities);
    let mut requirements = ContractRequirements {
        capabilities: required_capabilities,
        evidence: required_evidence,
        obligations: required_obligations,
    };
    carry_recorded_promotion_contract_requirements(
        config,
        &effective_profile,
        &plan.goal,
        &mut requirements,
    );
    required_capabilities = requirements.capabilities;
    required_evidence = requirements.evidence;
    required_obligations = requirements.obligations;
    let mut evidence_hint_tokens = evidence_hint_tokens_for_goal(&plan.goal);
    let bound_contract = bind_completion_contract_for_acceptance(
        config,
        "ultra-plan-run",
        &effective_profile,
        &plan.goal,
        &required_paths,
        &required_capabilities,
        &required_evidence,
        &required_obligations,
    )?;
    let mut deferred_commands = Vec::new();
    let mut verify_commands = Vec::new();
    if let Some(contract) = bound_contract.as_ref().map(|bound| &bound.contract) {
        merge_unique_strings(&mut required_paths, &contract.required_paths);
        merge_unique_strings(&mut required_capabilities, &contract.required_capabilities);
        merge_unique_strings(&mut required_evidence, &contract.required_evidence);
        merge_unique_strings(&mut required_obligations, &contract.required_obligations);
        merge_unique_strings(&mut verify_commands, &contract.verify_commands);
        merge_unique_strings(&mut evidence_hint_tokens, &contract.evidence_hint_tokens);
        deferred_commands.extend(
            contract
                .deferred_verify_requirements
                .iter()
                .map(|requirement| requirement.command.clone()),
        );
    }
    merge_unique_strings(
        &mut required_evidence,
        &inferred_required_evidence(&effective_profile, &plan.goal, &required_capabilities),
    );
    Ok(verify_runtime_acceptance_with_browser_dirs_and_hints(
        &config.workspace_root,
        &required_paths,
        &verify_commands,
        &required_capabilities,
        &required_evidence,
        &required_obligations,
        &deferred_commands,
        &release_evidence_extra_dirs(config),
        &evidence_hint_tokens,
    ))
}
