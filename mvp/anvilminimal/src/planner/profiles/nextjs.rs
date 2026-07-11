use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::minimal_loop::import_scan::{
    MissingImport, format_missing_import_findings, missing_import_target_path, route_bound_closure,
    scan_relative_imports,
};
use crate::planner::profile::profile_failure;
use crate::planner::profile::{
    ProfileDeterministicStepPlan, ProfileHookAttribute, ProfileHookSnapshotTarget,
    ProfileQualityExpectations,
};
use crate::planner::signals::requested_port_from_text;
use crate::planner::step_plan::{PlanStep, StepPlan};
use crate::planner::ultra_plan::{UltraPhase, UltraPlan};
use crate::planner::verify::{VerificationReport, VerifyStatus};
use crate::tools::path_guard::validate_workspace_relative;

pub const DEFAULT_REQUESTED_PORT: u16 = 3011;

pub fn requested_or_default_port(goal: &str) -> u16 {
    requested_port_from_text(goal).unwrap_or(DEFAULT_REQUESTED_PORT)
}

pub fn generation_rules(intent: &str) -> &'static str {
    match intent {
        "create" => {
            "- Profile nextjs/create: preserve a real Next.js app contract. Include next/react/react-dom dependencies, keep scripts.build as next build, and end with a build verification phase. Put dependency setup before any npm run build verification when node_modules is not already present; setup instructions may install dependencies, but verify must not contain npm install. If dependency setup is not allowed or cannot run, stop with dependency_missing instead of claiming build success. Keep a single route-bound implementation; do not leave capability components unimported. For interactive UI, extend the instrumented skeleton instead of replacing it, and preserve data-anvil-* attributes on route-bound UI: data-anvil-action=\"primary\" on the main start/submit/action control, data-anvil-action=\"input\" on the main text entry surface when one exists, and data-anvil-state with a JSON snapshot of meaningful visible state after each render. When the contract includes start_or_restart_flow, every restart affordance (game-over, victory, and in-play when present) should carry data-anvil-action=\"restart\"; the initial primary action alone cannot satisfy recovery verification. A restart reachable during play (hook or R-key) allows behavioral verification, while an overlay-only restart may verify as unverified:terminal_state_not_reached (partial). If you use Tailwind utility classes or @tailwind directives, include tailwindcss/postcss/autoprefixer and create tailwind.config.* plus postcss.config.*; postcss.config plugins must include BOTH tailwindcss and autoprefixer. Otherwise use plain CSS and do not write Tailwind utility classes. Keep scripts.dev and scripts.start on the explicit requested port when the goal or plan requests one; otherwise use port 3011 with next dev/start -p 3011 or --port 3011.\n"
        }
        "fix" => {
            "- Profile nextjs/fix: preserve the existing Next.js structure and verifier integrity. Keep a single route-bound implementation; do not leave capability components unimported. For interactive UI, extend the instrumented skeleton instead of replacing it, and preserve or add task-agnostic observability hooks: data-anvil-action=\"primary\" on the main start/submit/action control, data-anvil-action=\"input\" on the main text entry surface when one exists, and data-anvil-state with a JSON snapshot of meaningful visible state. When the contract includes start_or_restart_flow, every restart affordance (game-over, victory, and in-play when present) should carry data-anvil-action=\"restart\"; the initial primary action alone cannot satisfy recovery verification. A restart reachable during play (hook or R-key) allows behavioral verification, while an overlay-only restart may verify as unverified:terminal_state_not_reached (partial). Do not weaken next/react/react-dom dependencies, scripts.build, app/page, layout, or TypeScript configuration to make a failing verifier pass.\n"
        }
        "research" => {
            "- Profile nextjs/research: inspect the existing app and produce concrete findings. Do not modify source unless the user explicitly asks for fixes.\n"
        }
        _ => {
            "- Profile nextjs: preserve a real Next.js app when present. Keep next/react/react-dom dependencies, scripts.build as next build, app/ or pages/ entrypoints, and a final build verification phase. Keep a single route-bound implementation; do not leave capability components unimported. For interactive UI, extend the instrumented skeleton instead of replacing it, and preserve task-agnostic observability hooks: data-anvil-action=\"primary\" on the main start/submit/action control, data-anvil-action=\"input\" on the main text entry surface when one exists, and data-anvil-state with a JSON snapshot of meaningful visible state. When the contract includes start_or_restart_flow, every restart affordance (game-over, victory, and in-play when present) should carry data-anvil-action=\"restart\"; the initial primary action alone cannot satisfy recovery verification. A restart reachable during play (hook or R-key) allows behavioral verification, while an overlay-only restart may verify as unverified:terminal_state_not_reached (partial). Keep styling toolchains internally consistent; if Tailwind is used, postcss.config plugins must include BOTH tailwindcss and autoprefixer.\n"
        }
    }
}

pub fn verify(root: &Path, goal: &str) -> VerificationReport {
    let project = match locate_project_root(root) {
        Ok(project) => project,
        Err(reason) => return profile_failure(reason),
    };
    let package_path = project.path.join("package.json");
    let Ok(content) = std::fs::read_to_string(&package_path) else {
        return profile_failure(project.rel_path("package.json missing"));
    };
    let Ok(package): Result<Value, _> = serde_json::from_str(&content) else {
        return profile_failure(project.rel_path("package.json invalid"));
    };
    let deps = package.get("dependencies").and_then(Value::as_object);
    for dep in ["next", "react", "react-dom"] {
        if deps.is_none_or(|deps| !deps.contains_key(dep)) {
            return profile_failure(format!("dependency missing: {dep}"));
        }
    }
    if let Some(reason) = dependency_coherence_failure(&package) {
        return profile_failure(reason);
    }
    let scripts = package.get("scripts").and_then(Value::as_object);
    let build = scripts
        .and_then(|scripts| scripts.get("build"))
        .and_then(Value::as_str);
    if build != Some("next build") || build.is_some_and(is_weakened_script) {
        return profile_failure("scripts.build must be next build");
    }
    if scripts
        .and_then(|scripts| scripts.get("dev"))
        .and_then(Value::as_str)
        .is_some_and(is_weakened_script)
    {
        return profile_failure("scripts.dev must run next dev");
    }
    let port = requested_or_default_port(goal);
    let dev = scripts
        .and_then(|scripts| scripts.get("dev"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !script_runs_next_dev_on_port(dev, port) {
        return profile_failure(format!("dev script must run next dev on port {port}"));
    }
    let start = scripts
        .and_then(|scripts| scripts.get("start"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !start.is_empty() && !script_runs_next_start_on_port(start, port) {
        return profile_failure(format!("start script must run next start on port {port}"));
    }
    let Some(entry) = find_entrypoint(&project.path) else {
        return profile_failure(project.rel_path(
            "Next entrypoint missing: expected src/app/page.tsx, app/page.tsx, or pages/index.tsx",
        ));
    };
    if entry.requires_layout && find_app_layout(&project.path, &entry.app_dir).is_none() {
        return profile_failure(project.rel_path(&format!(
            "Next app router layout missing: expected {}/layout.tsx or layout.jsx",
            entry.app_dir
        )));
    }
    let uses_alias = contains_in_files(&project.path, "@/");
    if uses_alias {
        let Ok(tsconfig) = std::fs::read_to_string(project.path.join("tsconfig.json")) else {
            return profile_failure(project.rel_path("tsconfig.json missing for @/* alias"));
        };
        let Ok(tsconfig): Result<Value, _> = serde_json::from_str(&tsconfig) else {
            return profile_failure("tsconfig.json invalid");
        };
        if !alias_configured(&tsconfig) {
            return profile_failure("tsconfig baseUrl/paths missing @/* alias");
        }
    }
    if let Some(reason) = tsconfig_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = css_side_effect_import_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = missing_app_relative_import_contract_failure(&project.path) {
        return profile_failure(project.rel_path(&reason));
    }
    if let Some(reason) = client_component_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = tailwind_contract_failure(&project.path, &package) {
        return profile_failure(reason);
    }
    VerificationReport::pass()
}

pub fn verify_invariant(root: &Path, goal: &str) -> VerificationReport {
    let project = match locate_project_root(root) {
        Ok(project) => project,
        Err(reason) if reason == "package.json missing" => return VerificationReport::pass(),
        Err(reason) => return profile_failure(reason),
    };
    let package_path = project.path.join("package.json");
    let Ok(content) = std::fs::read_to_string(&package_path) else {
        return profile_failure(project.rel_path("package.json unreadable"));
    };
    let Ok(package): Result<Value, _> = serde_json::from_str(&content) else {
        return profile_failure(project.rel_path("package.json invalid"));
    };
    let deps = package.get("dependencies").and_then(Value::as_object);
    for dep in ["next", "react", "react-dom"] {
        if deps.is_none_or(|deps| !deps.contains_key(dep)) {
            return profile_failure(format!("dependency missing: {dep}"));
        }
    }
    if let Some(reason) = dependency_coherence_failure(&package) {
        return profile_failure(reason);
    }
    let scripts = package.get("scripts").and_then(Value::as_object);
    let build = scripts
        .and_then(|scripts| scripts.get("build"))
        .and_then(Value::as_str);
    if build.is_some_and(|build| build != "next build" || is_weakened_script(build)) {
        return profile_failure("scripts.build must be next build");
    }
    let port = requested_or_default_port(goal);
    let dev = scripts
        .and_then(|scripts| scripts.get("dev"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !dev.is_empty() && !script_runs_next_dev_on_port(dev, port) {
        return profile_failure(format!("dev script must run next dev on port {port}"));
    }
    let start = scripts
        .and_then(|scripts| scripts.get("start"))
        .and_then(Value::as_str)
        .unwrap_or("");
    if !start.is_empty() && !script_runs_next_start_on_port(start, port) {
        return profile_failure(format!("start script must run next start on port {port}"));
    }
    if let Some(reason) = tsconfig_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = css_side_effect_import_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = missing_app_relative_import_contract_failure(&project.path) {
        return profile_failure(project.rel_path(&reason));
    }
    if let Some(reason) = client_component_contract_failure(&project.path) {
        return profile_failure(reason);
    }
    if let Some(reason) = tailwind_contract_failure(&project.path, &package) {
        return profile_failure(reason);
    }
    VerificationReport::pass()
}

fn script_runs_next_dev_on_port(script: &str, port: u16) -> bool {
    script_runs_next_command_on_port(script, "next dev", port)
}

fn script_runs_next_start_on_port(script: &str, port: u16) -> bool {
    script_runs_next_command_on_port(script, "next start", port)
}

fn script_runs_next_command_on_port(script: &str, command: &str, port: u16) -> bool {
    script.contains(command)
        && (script.contains(&format!("-p {port}"))
            || script.contains(&format!("-p{port}"))
            || script.contains(&format!("--port {port}"))
            || script.contains(&format!("--port={port}")))
}

pub fn guidance(goal: &str) -> String {
    let port = requested_or_default_port(goal);
    let port = if requested_port_from_text(goal).is_some() {
        format!(
            " The dev/start scripts must run on the explicitly requested port {port}: `next dev -p {port}` and `next start -p {port}` or equivalent `--port {port}` forms."
        )
    } else {
        format!(
            " No explicit port was requested; use port {port} for scripts.dev and scripts.start (`next dev -p {port}` and `next start -p {port}` or equivalent `--port {port}` forms)."
        )
    };
    format!(
        "For the nextjs profile, create a runnable Next.js app, not only package metadata. \
         Keep the project in the workspace root unless a project subdirectory already exists. \
         Required setup scaffold artifacts by completion: package.json, tsconfig.json, postcss.config.js, exactly one tailwind.config.* file, src/app/layout.tsx, src/app/page.tsx, src/app/globals.css, src/app/global.d.ts. \
         If those files are absent, write the coherent App Router scaffold before further inspection. \
         Use tailwind.config.ts for new scaffolds unless exactly one existing Tailwind config is already being completed. \
         src/app/globals.css must contain the @tailwind directives, and src/app/layout.tsx must import ./globals.css. \
         If any layout imports CSS such as ./globals.css, src/app/global.d.ts must declare module \"*.css\". \
         package.json must include compatible next, react, react-dom, @types/react, @types/react-dom, and TypeScript 5.x dependencies plus scripts.build = `next build`. \
         If Tailwind is used, package.json must include tailwindcss/postcss/autoprefixer and postcss.config plugins must include BOTH tailwindcss and autoprefixer. \
         For TypeScript/TSX apps, create tsconfig.json before treating the app as complete. \
         Keep a single route-bound implementation; do not leave capability components unimported. \
         Do not use deprecated moduleResolution=node10 or target=ES5; prefer moduleResolution=bundler and target=ES2017 or newer. \
         For interactive UI, expose task-agnostic observability hooks: data-anvil-action=\"primary\" on the main start/submit/action control, data-anvil-action=\"input\" on the main text entry surface when one exists, and data-anvil-state containing JSON for meaningful visible state after each render. The data-anvil-state snapshot must include at least one dimension that immediately responds to input, such as player/paddle x position. When the contract includes start_or_restart_flow, every restart affordance (game-over, victory, and in-play when present) should carry data-anvil-action=\"restart\"; the initial primary action alone cannot satisfy recovery verification. A restart reachable during play (hook or R-key) allows behavioral verification, while an overlay-only restart may verify as unverified:terminal_state_not_reached (partial).{port}"
    )
}

pub fn runtime_contract(intent: &str, goal: &str) -> String {
    let port = requested_or_default_port(goal);
    let port = if requested_port_from_text(goal).is_some() {
        format!(
            "\n- Keep scripts.dev and scripts.start on the explicitly requested port {port}: next dev/start -p {port} or --port {port}."
        )
    } else {
        format!(
            "\n- No explicit port was requested; keep scripts.dev and scripts.start on port {port}: next dev/start -p {port} or --port {port}."
        )
    };
    match intent {
        "create" => format!(
            "- Preserve the workspace as a real Next.js app.\n\
- Keep next/react/react-dom dependencies in package.json.\n\
- Keep scripts.build as next build; do not replace it with echo/skip/no-op commands.\n\
- If npm run build cannot run because dependencies are not installed, report dependency_missing or use an explicit setup step; do not fake success.\
{port}\n\
- If using Tailwind utility classes or @tailwind directives, keep the Tailwind toolchain complete: tailwindcss/postcss/autoprefixer dependencies, tailwind.config.*, and postcss.config plugins with BOTH tailwindcss and autoprefixer. Otherwise use plain CSS.\n\
- Keep TypeScript and app router configuration coherent.\n\
- Keep a single route-bound implementation; do not leave capability components unimported.\n\
- For interactive UI, expose data-anvil-action=\"primary\" on the main start/submit/action control, data-anvil-action=\"input\" on the main text entry surface when one exists, and data-anvil-state with a JSON snapshot of meaningful visible state after each render. When the contract includes start_or_restart_flow, every restart affordance (game-over, victory, and in-play when present) should carry data-anvil-action=\"restart\"; the initial primary action alone cannot satisfy recovery verification. A restart reachable during play (hook or R-key) allows behavioral verification, while an overlay-only restart may verify as unverified:terminal_state_not_reached (partial).\n\
- Do not treat scaffold-only, package-only, or build-only output as complete."
        ),
        "fix" => format!(
            "- Preserve the existing Next.js app structure.\n\
- Keep next/react/react-dom dependencies when already present.\n\
- Keep scripts.build as next build when already present; do not weaken build/test scripts to hide failures.\n\
- If npm run build cannot run because dependencies are missing, report dependency_missing or use the existing dependency workflow.\
{port}\n\
- If Tailwind is used, postcss.config plugins must include BOTH tailwindcss and autoprefixer.\n\
- Keep TypeScript and app router configuration coherent.\n\
- Keep a single route-bound implementation; do not leave capability components unimported.\n\
- For interactive UI, preserve or add data-anvil-action=\"primary\" on the main start/submit/action control, data-anvil-action=\"input\" on the main text entry surface when one exists, and data-anvil-state with a JSON snapshot of meaningful visible state. When the contract includes start_or_restart_flow, every restart affordance (game-over, victory, and in-play when present) should carry data-anvil-action=\"restart\"; the initial primary action alone cannot satisfy recovery verification. A restart reachable during play (hook or R-key) allows behavioral verification, while an overlay-only restart may verify as unverified:terminal_state_not_reached (partial).\n\
- Do not treat scaffold-only, package-only, or build-only output as complete."
        ),
        "research" | "investigate" => {
            "- Preserve the existing Next.js app unchanged unless the phase explicitly asks for fixes.\n\
- Produce concrete findings from inspected files and commands.\n\
- Separate observed facts from hypotheses.\n\
- Do not weaken package scripts or test/build checks while investigating."
                .to_string()
        }
        _ => format!(
            "- Preserve the workspace as a Next.js app when one exists.\n\
- Do not convert package.json to a standalone TypeScript/Node project.\n\
- Keep next/react/react-dom dependencies when already present.\n\
- Keep scripts.build as next build when already present.\
{port}\n\
- Keep styling and TypeScript toolchains internally consistent; if Tailwind is used, postcss.config plugins must include BOTH tailwindcss and autoprefixer.\n\
- Keep a single route-bound implementation; do not leave capability components unimported.\n\
- For interactive UI, expose data-anvil-action=\"primary\" on the main start/submit/action control, data-anvil-action=\"input\" on the main text entry surface when one exists, and data-anvil-state with a JSON snapshot of meaningful visible state. When the contract includes start_or_restart_flow, every restart affordance (game-over, victory, and in-play when present) should carry data-anvil-action=\"restart\"; the initial primary action alone cannot satisfy recovery verification. A restart reachable during play (hook or R-key) allows behavioral verification, while an overlay-only restart may verify as unverified:terminal_state_not_reached (partial).\n\
- Do not treat scaffold-only, package-only, or build-only output as complete."
        ),
    }
}

pub fn setup_scaffold_paths(root: &Path) -> Vec<String> {
    let project = locate_project_root(root).ok();
    let prefix = project
        .as_ref()
        .map(|project| project.prefix.as_str())
        .unwrap_or_default();
    let project_root = project
        .as_ref()
        .map(|project| project.path.as_path())
        .unwrap_or(root);
    vec![
        format!("{prefix}package.json"),
        format!("{prefix}tsconfig.json"),
        format!("{prefix}postcss.config.js"),
        format!("{prefix}{}", setup_tailwind_config_rel(project_root)),
        format!("{prefix}src/app/layout.tsx"),
        format!("{prefix}src/app/page.tsx"),
        format!("{prefix}src/app/globals.css"),
        format!("{prefix}src/app/global.d.ts"),
    ]
}

pub fn setup_invariant_required_paths(root: &Path) -> Vec<String> {
    filter_setup_invariant_paths(root, setup_scaffold_paths(root))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupStepChecks {
    pub ownership: &'static str,
    pub expected_paths: Vec<String>,
    pub verify_commands: Vec<String>,
}

pub fn setup_step_checks(
    root: &Path,
    goal: &str,
    step_id: &str,
    instruction: &str,
) -> Option<SetupStepChecks> {
    let text = format!("{step_id} {instruction}").to_ascii_lowercase();
    let port = requested_or_default_port(goal);
    if mentions_package_scripts(&text) {
        let package_path = setup_scaffold_paths(root)
            .into_iter()
            .find(|path| path.ends_with("package.json"))
            .unwrap_or_else(|| "package.json".to_string());
        let mut verify_commands = package_script_port_verify_commands(port);
        verify_commands.push(package_build_script_verify_command());
        return Some(SetupStepChecks {
            ownership: "package_manifest",
            expected_paths: vec![package_path],
            verify_commands,
        });
    }
    if mentions_scaffold_configuration(&text) {
        let expected_paths = setup_invariant_required_paths(root);
        let mut verify_commands = expected_paths
            .iter()
            .map(|path| format!("test -f {}", shell_single_quote(path)))
            .collect::<Vec<_>>();
        verify_commands.extend(package_script_port_verify_commands(port));
        verify_commands.push(package_build_script_verify_command());
        return Some(SetupStepChecks {
            ownership: "scaffold_configuration",
            expected_paths,
            verify_commands,
        });
    }
    None
}

fn mentions_package_scripts(text: &str) -> bool {
    text.contains("package.json")
        || text.contains("package manifest")
        || text.contains("package script")
        || text.contains("port script")
        || text
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|token| matches!(token, "script" | "scripts" | "manifest" | "port"))
}

fn mentions_scaffold_configuration(text: &str) -> bool {
    text.contains("scaffold")
        || text.contains("project shell")
        || text.contains("tsconfig")
        || text.contains("postcss")
        || text.contains("tailwind")
        || text
            .split(|ch: char| !ch.is_ascii_alphanumeric())
            .any(|token| token == "config")
        || ((text.contains("setup") || text.contains("set up"))
            && text.contains("project")
            && !text.contains("dependenc"))
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

pub fn filter_setup_invariant_paths(root: &Path, paths: Vec<String>) -> Vec<String> {
    if plain_css_without_tailwind_artifacts(root) {
        paths
            .into_iter()
            .filter(|path| !tailwind_stack_scaffold_path(path))
            .collect()
    } else {
        paths
    }
}

pub fn expected_paths(root: &Path, _goal: &str) -> Vec<String> {
    setup_scaffold_paths(root)
}

pub fn deterministic_step_plan(
    phase_prompt: &str,
    root: &Path,
    goal: &str,
) -> Option<ProfileDeterministicStepPlan> {
    let phase_text = phase_id_and_task_text(phase_prompt).unwrap_or_else(|| phase_prompt.into());
    let phase_id = phase_field(phase_prompt, "Phase id:").unwrap_or_default();
    let phase_id_lower = phase_id.to_ascii_lowercase();
    let lower = phase_text.to_ascii_lowercase();
    if looks_like_implementation_phase(&lower) {
        return None;
    }
    if looks_like_scaffold_phase(&lower) && looks_like_scaffold_phase_id(&phase_id_lower) {
        return Some(scaffold_step_plan(phase_prompt, root, goal));
    }
    if looks_like_port_script_phase(&lower) {
        return Some(port_script_step_plan(phase_prompt, goal));
    }
    if looks_like_build_verify_phase(&lower) {
        return Some(build_verify_step_plan(phase_prompt, root, goal));
    }
    None
}

pub fn preset_ultra_plan(goal: &str, style: &str, intent: &str) -> Option<UltraPlan> {
    if !style.eq_ignore_ascii_case("default") || !intent.eq_ignore_ascii_case("create") {
        return None;
    }
    Some(UltraPlan {
        goal: goal.to_string(),
        profile: "nextjs".to_string(),
        style: "default".to_string(),
        intent: "create".to_string(),
        phases: vec![
            UltraPhase {
                id: "project-setup".to_string(),
                prompt: "Scaffold and setup the Next.js App Router project shell. Create or complete the package manifest, TypeScript config, styling config, and route-bound scaffold so the deterministic nextjs-scaffold template owns setup artifacts."
                    .to_string(),
            },
            UltraPhase {
                id: "core-implementation".to_string(),
                prompt: format!(
                    "Implement the core task-specific behavior for: {goal}. Keep one route-bound implementation, extend the instrumented skeleton instead of replacing it, and keep the implementation in the Next.js route-bound source."
                ),
            },
            UltraPhase {
                id: "contract-wiring".to_string(),
                prompt: "Wire controls and data-anvil observability. Preserve or add data-anvil-action=\"primary\" on the main start/submit/action control, data-anvil-action=\"input\" on the main text entry surface when one exists, and data-anvil-state with a JSON snapshot of meaningful visible state. The data-anvil-state snapshot must include at least one dimension that immediately responds to input, such as player/paddle x position. When the contract includes start_or_restart_flow, every restart affordance (game-over, victory, and in-play when present) should carry data-anvil-action=\"restart\"; the initial primary action alone cannot satisfy recovery verification."
                    .to_string(),
            },
            UltraPhase {
                id: "build-verification".to_string(),
                prompt: "Run build verification for the deterministic Next.js scaffold. Verify package scripts, dependency boundary, and npm run build / next build only; keep this final phase verification-only."
                    .to_string(),
            },
        ],
    })
}

fn scaffold_step_plan(phase_prompt: &str, root: &Path, goal: &str) -> ProfileDeterministicStepPlan {
    let mut expected_paths = setup_scaffold_paths(root);
    merge_required_final_artifacts(&mut expected_paths, phase_prompt);
    let port = requested_or_default_port(goal);
    ProfileDeterministicStepPlan {
        template_id: "nextjs-scaffold".to_string(),
        plan: StepPlan {
            goal: "Create the deterministic Next.js scaffold.".to_string(),
            steps: vec![
                PlanStep {
                    id: "nextjs-scaffold".to_string(),
                    kind: "setup".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: format!(
                        "Create or complete the Next.js App Router scaffold, package manifest, TypeScript config, styling config, and route-bound page. Keep package.json dev/start scripts on port {port}. Required files: {}.",
                        expected_paths.join(", ")
                    ),
                    expected_paths,
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "nextjs-profile-verify".to_string(),
                    kind: "verify".to_string(),
                    expected_result: "pass".to_string(),
                    instruction:
                        "Verify package scripts use the requested port and the Next.js build command remains executable."
                            .to_string(),
                    expected_paths: Vec::new(),
                    verify: profile_verify_commands(port),
                },
            ],
        },
    }
}

fn port_script_step_plan(phase_prompt: &str, goal: &str) -> ProfileDeterministicStepPlan {
    let port = requested_or_default_port(goal);
    let mut expected_paths = vec!["package.json".to_string()];
    merge_required_final_artifacts(&mut expected_paths, phase_prompt);
    ProfileDeterministicStepPlan {
        template_id: "nextjs-port-scripts".to_string(),
        plan: StepPlan {
            goal: "Configure deterministic Next.js package scripts.".to_string(),
            steps: vec![
                PlanStep {
                    id: "configure-nextjs-port-scripts".to_string(),
                    kind: "implement".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: format!(
                        "Update package.json so scripts.dev runs next dev on port {port}, scripts.start runs next start on port {port} when present, and scripts.build remains next build."
                    ),
                    expected_paths,
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "verify-nextjs-port-scripts".to_string(),
                    kind: "verify".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Verify package scripts preserve the requested Next.js port."
                        .to_string(),
                    expected_paths: Vec::new(),
                    verify: package_script_port_verify_commands(port),
                },
            ],
        },
    }
}

fn build_verify_step_plan(
    phase_prompt: &str,
    root: &Path,
    goal: &str,
) -> ProfileDeterministicStepPlan {
    let mut expected_paths = setup_scaffold_paths(root);
    merge_required_final_artifacts(&mut expected_paths, phase_prompt);
    let port = requested_or_default_port(goal);
    ProfileDeterministicStepPlan {
        template_id: "nextjs-build-verification".to_string(),
        plan: StepPlan {
            goal: "Verify the deterministic Next.js build.".to_string(),
            steps: vec![
                PlanStep {
                    id: "ensure-nextjs-build-inputs".to_string(),
                    kind: "setup".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: format!(
                        "Ensure the Next.js scaffold and route-bound entrypoint exist before build verification. Required files: {}.",
                        expected_paths.join(", ")
                    ),
                    expected_paths,
                    verify: Vec::new(),
                },
                PlanStep {
                    id: "verify-nextjs-build".to_string(),
                    kind: "verify".to_string(),
                    expected_result: "pass".to_string(),
                    instruction: "Run deterministic package script and Next.js build verification."
                        .to_string(),
                    expected_paths: Vec::new(),
                    verify: profile_verify_commands(port),
                },
            ],
        },
    }
}

fn profile_verify_commands(port: u16) -> Vec<String> {
    let mut verify = package_script_port_verify_commands(port);
    verify.push(package_build_script_verify_command());
    verify.push("npm run build".to_string());
    verify
}

fn package_script_port_verify_commands(port: u16) -> Vec<String> {
    vec![
        package_script_required_port_verify_command("dev", port),
        package_script_optional_port_verify_command("start", port),
    ]
}

fn package_script_required_port_verify_command(script: &str, port: u16) -> String {
    format!(
        concat!(
            "node -p \"",
            "['{script}'].every(function(k){{",
            "return String(Object(require('./package.json').scripts)[k]).split(' ')",
            ".some(function(t,i,a){{return t=='next' ? a.slice(i+1).find(function(x){{return x}})==k : false}}) ? ",
            "String(Object(require('./package.json').scripts)[k]).split(' ')",
            ".some(function(t,i,a){{",
            "return t=='--port={port}' ? true : ",
            "t=='-p' ? a.slice(i+1).find(function(x){{return x}})=='{port}' : ",
            "t=='-p{port}' ? true : ",
            "t=='--port' ? a.slice(i+1).find(function(x){{return x}})=='{port}' : ",
            "false",
            "}}) : false",
            "}}) ? true : process.exit(1)",
            "\""
        ),
        script = script,
        port = port
    )
}

fn package_script_optional_port_verify_command(script: &str, port: u16) -> String {
    format!(
        concat!(
            "node -p \"",
            "['{script}'].every(function(k){{",
            "return Object(require('./package.json').scripts)[k] ? ",
            "String(Object(require('./package.json').scripts)[k]).split(' ')",
            ".some(function(t,i,a){{return t=='next' ? a.slice(i+1).find(function(x){{return x}})==k : false}}) ? ",
            "String(Object(require('./package.json').scripts)[k]).split(' ')",
            ".some(function(t,i,a){{",
            "return t=='--port={port}' ? true : ",
            "t=='-p' ? a.slice(i+1).find(function(x){{return x}})=='{port}' : ",
            "t=='-p{port}' ? true : ",
            "t=='--port' ? a.slice(i+1).find(function(x){{return x}})=='{port}' : ",
            "false",
            "}}) : false : true",
            "}}) ? true : process.exit(1)",
            "\""
        ),
        script = script,
        port = port
    )
}

fn package_build_script_verify_command() -> String {
    "node -p \"String(require('./package.json').scripts.build)=='next build' ? true : process.exit(1)\""
        .to_string()
}

fn merge_required_final_artifacts(paths: &mut Vec<String>, phase_prompt: &str) {
    for path in required_final_artifact_paths(phase_prompt) {
        push_unique_path(paths, &path);
    }
}

fn required_final_artifact_paths(phase_prompt: &str) -> Vec<String> {
    let mut in_section = false;
    let mut out = Vec::new();
    for line in phase_prompt.lines() {
        let trimmed = line.trim();
        if trimmed == "Required final artifacts:" {
            in_section = true;
            continue;
        }
        if !in_section {
            continue;
        }
        if trimmed.starts_with('-') {
            if let Some(path) = normalize_required_artifact_line(trimmed.trim_start_matches('-')) {
                push_unique_path(&mut out, &path);
            }
            continue;
        }
        if !trimmed.is_empty() {
            break;
        }
    }
    out
}

fn normalize_required_artifact_line(line: &str) -> Option<String> {
    let raw = line.trim();
    let token = raw
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_matches(['`', '"', '\''])
        .trim_end_matches([',', ';']);
    if token.is_empty() || validate_workspace_relative(token).is_err() {
        return None;
    }
    Some(token.to_string())
}

fn phase_id_and_task_text(phase_prompt: &str) -> Option<String> {
    let mut out = Vec::new();
    for line in phase_prompt.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("Phase id:") || trimmed.starts_with("Phase task:") {
            out.push(trimmed.to_string());
        }
    }
    (!out.is_empty()).then(|| out.join("\n"))
}

fn phase_field(phase_prompt: &str, prefix: &str) -> Option<String> {
    phase_prompt.lines().find_map(|line| {
        line.trim_start()
            .strip_prefix(prefix)
            .map(|value| value.trim().to_string())
    })
}

fn looks_like_scaffold_phase(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "scaffold",
            "setup",
            "set up",
            "project shell",
            "initialize",
            "initialise",
            "bootstrap",
            "app router scaffold",
            "初期",
            "セットアップ",
        ],
    )
}

fn looks_like_scaffold_phase_id(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "scaffold",
            "setup",
            "set-up",
            "project-setup",
            "bootstrap",
            "initialize",
            "initialise",
        ],
    )
}

fn looks_like_port_script_phase(lower: &str) -> bool {
    (lower.contains("port") || lower.contains("ポート"))
        && contains_any(
            lower,
            &[
                "script",
                "package",
                "package.json",
                "dev/start",
                "dev script",
                "start script",
                "設定",
            ],
        )
}

fn looks_like_build_verify_phase(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "build verification",
            "verify build",
            "build verifier",
            "npm run build",
            "next build",
            "ビルド検証",
        ],
    )
}

fn looks_like_implementation_phase(lower: &str) -> bool {
    contains_any(
        lower,
        &[
            "game logic",
            "gameplay",
            "mechanic",
            "adversary",
            "challenge",
            "collision",
            "failure rule",
            "score",
            "player control",
            "canvas",
            "stateful update",
            "user input",
            "interactive surface",
            "ゲームロジック",
            "衝突",
            "スコア",
            "敵",
            "プレイヤー",
            "操作",
        ],
    )
}

fn contains_any(text: &str, tokens: &[&str]) -> bool {
    tokens.iter().any(|token| text.contains(token))
}

pub fn complete_scaffold(root: &Path, missing_paths: &[String]) -> anyhow::Result<Vec<String>> {
    let project = locate_project_root(root).unwrap_or_else(|_| ProjectRoot {
        path: root.to_path_buf(),
        prefix: String::new(),
    });
    let mut created = Vec::new();
    for rel in missing_paths {
        let Some(project_rel) = rel.strip_prefix(&project.prefix) else {
            continue;
        };
        let Some(content) = scaffold_file_content(&project.path, project_rel) else {
            continue;
        };
        if write_absent(&root.join(rel), content)? {
            created.push(rel.clone());
        }
    }
    Ok(created)
}

fn scaffold_file_content(project_root: &Path, project_rel: &str) -> Option<&'static str> {
    match project_rel {
        "package.json" => Some(canonical_package_json()),
        "tsconfig.json" => Some(canonical_tsconfig()),
        "postcss.config.js" => Some(canonical_postcss_config()),
        "src/app/globals.css" => Some(canonical_tailwind_css()),
        "src/app/global.d.ts" => Some(canonical_global_d_ts()),
        "src/app/layout.tsx" => Some(canonical_layout_tsx()),
        "src/app/page.tsx" => Some(fallback_page()),
        rel if rel == setup_tailwind_config_rel(project_root) => Some(canonical_tailwind_config()),
        _ => None,
    }
}

pub fn app_source_paths(root: &Path) -> Vec<String> {
    let Ok(project) = locate_project_root(root) else {
        return Vec::new();
    };
    project_app_source_paths(&project.path)
        .into_iter()
        .map(|path| format!("{}{}", project.prefix, path))
        .collect()
}

pub fn evidence_repair_target_paths(root: &Path, evidence_keys: &[String]) -> Vec<String> {
    if !evidence_keys
        .iter()
        .any(|key| behavioral_repair_evidence(key))
    {
        return Vec::new();
    }
    let route_bound = route_bound_source_paths(root);
    let mut out = Vec::new();
    for rel in route_bound
        .iter()
        .filter(|rel| source_contains_data_anvil(root, rel))
    {
        push_unique_path(&mut out, rel);
    }
    for entrypoint in [
        "src/app/page.tsx",
        "src/app/page.jsx",
        "src/app/page.ts",
        "src/app/page.js",
        "app/page.tsx",
        "app/page.jsx",
        "app/page.ts",
        "app/page.js",
        "pages/index.tsx",
        "pages/index.jsx",
        "pages/index.ts",
        "pages/index.js",
        "src/pages/index.tsx",
        "src/pages/index.jsx",
        "src/pages/index.ts",
        "src/pages/index.js",
    ] {
        for rel in route_bound
            .iter()
            .filter(|rel| rel.as_str() == entrypoint || rel.ends_with(&format!("/{entrypoint}")))
        {
            push_unique_path(&mut out, rel);
        }
    }
    for rel in &route_bound {
        push_unique_path(&mut out, rel);
    }
    out
}

pub fn hook_snapshot_targets(root: &Path) -> Vec<ProfileHookSnapshotTarget> {
    route_bound_source_paths(root)
        .into_iter()
        .map(|relative_path| ProfileHookSnapshotTarget {
            relative_path,
            required_attributes: vec![
                ProfileHookAttribute::PrimaryAction,
                ProfileHookAttribute::RestartAction,
                ProfileHookAttribute::State,
            ],
        })
        .collect()
}

fn route_bound_source_paths(root: &Path) -> Vec<String> {
    route_bound_closure(root, "nextjs")
        .into_iter()
        .map(|path| path.display().to_string().replace('\\', "/"))
        .filter(|rel| is_import_scan_source_path(Path::new(rel)))
        .collect()
}

fn source_contains_data_anvil(root: &Path, rel: &str) -> bool {
    std::fs::read_to_string(root.join(rel)).is_ok_and(|content| content.contains("data-anvil-"))
}

fn behavioral_repair_evidence(key: &str) -> bool {
    matches!(
        key.trim(),
        "restart_or_recoverable_state_evidence"
            | "challenge_or_adversary_evidence"
            | "failure_or_collision_evidence"
            | "user_input_handler_evidence"
            | "stateful_update_evidence"
            | "visible_interactive_surface_evidence"
            | "interactive_ui_source_evidence"
            | "non_static_screen_evidence"
            | "score_or_progression_evidence"
    )
}

fn push_unique_path(out: &mut Vec<String>, path: &str) {
    if !path.trim().is_empty() && !out.iter().any(|existing| existing == path) {
        out.push(path.to_string());
    }
}

pub fn quality_expectations(root: &Path, goal: &str) -> ProfileQualityExpectations {
    ProfileQualityExpectations {
        required_artifacts: expected_paths(root, goal),
        preferred_verify: vec!["npm run build".to_string()],
        forbidden_verify: vec![
            "next dev".to_string(),
            "npm install".to_string(),
            "pnpm install".to_string(),
            "yarn install".to_string(),
        ],
        dependency_order_hint: Some(
            "Create package.json and a Next.js entrypoint before npm run build. Known-profile scaffolds are pre-provisioned before phase 1 when absent; verify or extend existing scaffold rather than re-planning file creation.".to_string(),
        ),
    }
}

pub fn repair_prompt(root: &Path, goal: &str, report: &VerificationReport) -> String {
    let expected = expected_paths(root, goal).join(", ");
    let failure = match &report.status {
        VerifyStatus::ProfileContractFailed(reason) => reason.as_str(),
        _ => "profile verification failed",
    };
    format!(
        "Repair the Next.js profile contract for this goal: {goal}\n\
         Failure: {failure}\n\
         Required paths: {expected}\n\
         Make the smallest bounded change inside the workspace. \
         If package.json exists only in a project subdirectory, continue using that subdirectory. \
         Ensure the app has a concrete playable page and layout, package dependencies, \
         scripts.build = `next build`, and dev/start scripts on the explicit requested port or 3011 when no port was requested. \
         Keep a single route-bound implementation; do not leave capability components unimported. \
         If Tailwind is used, postcss.config plugins must include BOTH tailwindcss and autoprefixer. \
         Use tools for file changes, then stop."
    )
}

pub fn auto_repair(root: &Path, goal: &str, report: &VerificationReport) -> anyhow::Result<bool> {
    if report.is_pass() {
        return Ok(false);
    }
    let reason = report.primary_reason();
    if reason.contains("tailwind_contract_failure") {
        return repair_tailwind_contract(root, goal, &reason);
    }
    if reason.contains("missing relative imports") {
        let project = locate_project_root(root).unwrap_or_else(|_| ProjectRoot {
            path: root.to_path_buf(),
            prefix: String::new(),
        });
        return repair_missing_css_import_artifacts(&project.path);
    }
    let project = locate_project_root(root).unwrap_or_else(|_| ProjectRoot {
        path: root.to_path_buf(),
        prefix: String::new(),
    });
    std::fs::create_dir_all(project.path.join("src/app"))?;
    ensure_package_json(&project.path, goal)?;
    ensure_file(
        &project.path.join("next.config.js"),
        "/** @type {import('next').NextConfig} */\nconst nextConfig = {};\n\nmodule.exports = nextConfig;\n",
    )?;
    ensure_file(
        &project.path.join("tsconfig.json"),
        r#"{"compilerOptions":{"target":"ES2017","lib":["dom","dom.iterable","esnext"],"allowJs":true,"skipLibCheck":true,"strict":true,"noEmit":true,"esModuleInterop":true,"module":"esnext","moduleResolution":"bundler","resolveJsonModule":true,"isolatedModules":true,"jsx":"preserve","incremental":true,"plugins":[{"name":"next"}],"baseUrl":".","paths":{"@/*":["./src/*"]}},"include":["next-env.d.ts","**/*.ts","**/*.tsx",".next/types/**/*.ts"],"exclude":["node_modules"]}"#,
    )?;
    ensure_file(
        &project.path.join("src/app/globals.css"),
        "* { box-sizing: border-box; }\nhtml, body { margin: 0; min-height: 100%; background: #05070d; color: #eef7ff; }\nbutton { font: inherit; }\n",
    )?;
    ensure_file(
        &project.path.join("src/app/global.d.ts"),
        "declare module \"*.css\";\n",
    )?;
    ensure_file(
        &project.path.join("src/app/layout.tsx"),
        r#"import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Interactive Challenge",
  description: "A compact interactive challenge generated by anvilminimal",
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
"#,
    )?;
    ensure_file(&project.path.join("src/app/page.tsx"), fallback_page())?;
    Ok(true)
}

pub fn repair_tailwind_contract(root: &Path, goal: &str, reason: &str) -> anyhow::Result<bool> {
    if !reason.contains("tailwind_contract_failure") {
        return Ok(false);
    }
    let project = locate_project_root(root).unwrap_or_else(|_| ProjectRoot {
        path: root.to_path_buf(),
        prefix: String::new(),
    });
    let project_root = project.path.as_path();
    let mut changed = false;
    changed |= repair_missing_css_import_artifacts(project_root)?;
    if reason.contains("Tailwind toolchain dependency missing:") {
        changed |= ensure_package_json_changed(project_root, goal)?;
        return Ok(changed);
    }
    if reason.contains("Tailwind config file missing") {
        if !has_tailwind_config(project_root) {
            changed |= write_file_if_changed(
                &project_root.join(setup_tailwind_config_rel(project_root)),
                canonical_tailwind_config(),
            )?;
        }
        return Ok(changed);
    }
    if reason.contains("PostCSS config file missing") {
        changed |= ensure_package_json_changed(project_root, goal)?;
        changed |= write_file_if_changed(
            &project_root.join("postcss.config.js"),
            canonical_postcss_config(),
        )?;
        return Ok(changed);
    }
    if reason.contains("PostCSS config uses ESM export default") {
        changed |= write_file_if_changed(
            &project_root.join("postcss.config.js"),
            canonical_postcss_config(),
        )?;
        return Ok(changed);
    }
    if reason.contains("Tailwind config uses ESM export default") {
        changed |= repair_tailwind_module_format(project_root)?;
        return Ok(changed);
    }
    if reason.contains("PostCSS config must include the Tailwind plugin")
        || reason.contains("PostCSS config must include autoprefixer")
        || reason.contains("PostCSS config must export a plugins key")
    {
        changed |= ensure_package_json_changed(project_root, goal)?;
        changed |= repair_postcss_plugins(project_root)?;
        return Ok(changed);
    }
    if reason.contains("@tailwind CSS file must be imported by app layout") {
        return repair_tailwind_layout_import(project_root, reason);
    }
    Ok(false)
}

pub fn repair_manifest_coherence(root: &Path, goal: &str) -> anyhow::Result<bool> {
    let Ok(project) = locate_project_root(root) else {
        return Ok(false);
    };
    let path = project.path.join("package.json");
    if !path.is_file() {
        return Ok(false);
    }
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    ensure_package_json(&project.path, goal)?;
    let after = std::fs::read_to_string(&path).unwrap_or_default();
    Ok(before != after)
}

#[derive(Debug, Clone)]
struct ProjectRoot {
    path: PathBuf,
    prefix: String,
}

impl ProjectRoot {
    fn rel_path(&self, message: &str) -> String {
        if self.prefix.is_empty() {
            message.to_string()
        } else {
            format!("{}: {message}", self.prefix.trim_end_matches('/'))
        }
    }
}

#[derive(Debug, Clone)]
struct EntryPoint {
    app_dir: String,
    requires_layout: bool,
}

fn locate_project_root(root: &Path) -> Result<ProjectRoot, String> {
    if root.join("package.json").is_file() {
        return Ok(ProjectRoot {
            path: root.to_path_buf(),
            prefix: String::new(),
        });
    }
    let mut nested = Vec::new();
    let entries = std::fs::read_dir(root).map_err(|_| "package.json missing".to_string())?;
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() || entry.path().join("node_modules").is_dir() {
            continue;
        }
        if entry.path().join("package.json").is_file() {
            let name = entry.file_name().to_string_lossy().to_string();
            nested.push(ProjectRoot {
                path: entry.path(),
                prefix: format!("{name}/"),
            });
        }
    }
    match nested.len() {
        0 => Err("package.json missing".to_string()),
        1 => Ok(nested.remove(0)),
        _ => Err(
            "multiple nested package.json files found; keep one Next.js project in the workspace"
                .to_string(),
        ),
    }
}

const TAILWIND_CONFIG_RELS: &[&str] = &[
    "tailwind.config.ts",
    "tailwind.config.js",
    "tailwind.config.cjs",
    "tailwind.config.mjs",
];

fn setup_tailwind_config_rel(project_root: &Path) -> &'static str {
    TAILWIND_CONFIG_RELS
        .iter()
        .copied()
        .find(|rel| project_root.join(rel).is_file())
        .unwrap_or("tailwind.config.ts")
}

fn ensure_package_json(root: &Path, goal: &str) -> anyhow::Result<()> {
    let path = root.join("package.json");
    let mut package = std::fs::read_to_string(&path)
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    package
        .entry("name")
        .or_insert_with(|| Value::String("anvilminimal-nextjs-app".to_string()));
    package
        .entry("version")
        .or_insert_with(|| Value::String("1.0.0".to_string()));
    package
        .entry("private")
        .or_insert_with(|| Value::Bool(true));
    let deps = object_entry(&mut package, "dependencies");
    ensure_dependency(deps, "next", "^14.2.0");
    ensure_dependency(deps, "react", "^18.3.0");
    ensure_dependency(deps, "react-dom", "^18.3.0");
    let tailwind_used = uses_tailwind(root, &Value::Object(package.clone()));
    let dev_deps = object_entry(&mut package, "devDependencies");
    ensure_dependency(dev_deps, "typescript", "^5.5.0");
    ensure_dependency(dev_deps, "@types/node", "^20.14.0");
    ensure_dependency(dev_deps, "@types/react", "^18.3.0");
    ensure_dependency(dev_deps, "@types/react-dom", "^18.3.0");
    if tailwind_used {
        ensure_dependency(dev_deps, "tailwindcss", "^3.4.19");
        ensure_dependency(dev_deps, "postcss", "^8.5.15");
        ensure_dependency(dev_deps, "autoprefixer", "^10.4.20");
    }
    let scripts = object_entry(&mut package, "scripts");
    let requested_port = requested_or_default_port(goal);
    let dev = format!("next dev -p {requested_port}");
    scripts.insert("dev".to_string(), Value::String(dev));
    scripts.insert("build".to_string(), Value::String("next build".to_string()));
    scripts.insert(
        "start".to_string(),
        Value::String(format!("next start -p {requested_port}")),
    );
    let content = serde_json::to_string_pretty(&Value::Object(package))?;
    std::fs::write(path, format!("{content}\n"))?;
    Ok(())
}

fn object_entry<'a>(package: &'a mut Map<String, Value>, key: &str) -> &'a mut Map<String, Value> {
    let value = package
        .entry(key.to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    if !value.is_object() {
        *value = Value::Object(Map::new());
    }
    value.as_object_mut().expect("value was just made object")
}

fn ensure_dependency(deps: &mut Map<String, Value>, name: &str, version: &str) {
    let needs_update = deps
        .get(name)
        .and_then(Value::as_str)
        .is_none_or(|current| dependency_version_needs_repair(name, current));
    if needs_update {
        deps.insert(name.to_string(), Value::String(version.to_string()));
    }
}

fn ensure_file(path: &Path, content: &str) -> anyhow::Result<()> {
    if !path.is_file() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, content)?;
    }
    Ok(())
}

fn write_absent(path: &Path, content: &str) -> anyhow::Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(true)
}

fn ensure_package_json_changed(root: &Path, goal: &str) -> anyhow::Result<bool> {
    let path = root.join("package.json");
    let before = std::fs::read_to_string(&path).unwrap_or_default();
    ensure_package_json(root, goal)?;
    let after = std::fs::read_to_string(path).unwrap_or_default();
    Ok(before != after)
}

fn write_file_if_changed(path: &Path, content: &str) -> anyhow::Result<bool> {
    if std::fs::read_to_string(path).is_ok_and(|existing| existing == content) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(true)
}

fn canonical_tailwind_config() -> &'static str {
    "import type { Config } from \"tailwindcss\";\n\nconst config: Config = {\n  content: [\n    \"./src/pages/**/*.{js,ts,jsx,tsx,mdx}\",\n    \"./src/components/**/*.{js,ts,jsx,tsx,mdx}\",\n    \"./src/app/**/*.{js,ts,jsx,tsx,mdx}\",\n  ],\n  theme: { extend: {} },\n  plugins: [],\n};\n\nexport default config;\n"
}

fn canonical_tailwind_config_cjs() -> &'static str {
    "module.exports = {\n  content: [\n    \"./src/pages/**/*.{js,ts,jsx,tsx,mdx}\",\n    \"./src/components/**/*.{js,ts,jsx,tsx,mdx}\",\n    \"./src/app/**/*.{js,ts,jsx,tsx,mdx}\",\n  ],\n  theme: { extend: {} },\n  plugins: [],\n};\n"
}

fn canonical_package_json() -> &'static str {
    "{\n  \"name\": \"anvilminimal-nextjs-app\",\n  \"version\": \"1.0.0\",\n  \"private\": true,\n  \"dependencies\": {\n    \"next\": \"^14.2.0\",\n    \"react\": \"^18.3.0\",\n    \"react-dom\": \"^18.3.0\"\n  },\n  \"devDependencies\": {\n    \"@types/node\": \"^20.14.0\",\n    \"@types/react\": \"^18.3.0\",\n    \"@types/react-dom\": \"^18.3.0\",\n    \"autoprefixer\": \"^10.4.20\",\n    \"postcss\": \"^8.5.15\",\n    \"tailwindcss\": \"^3.4.19\",\n    \"typescript\": \"^5.5.0\"\n  },\n  \"scripts\": {\n    \"build\": \"next build\",\n    \"dev\": \"next dev -p 3011\",\n    \"start\": \"next start -p 3011\"\n  }\n}\n"
}

fn canonical_tsconfig() -> &'static str {
    r#"{"compilerOptions":{"target":"ES2017","lib":["dom","dom.iterable","esnext"],"allowJs":true,"skipLibCheck":true,"strict":true,"noEmit":true,"esModuleInterop":true,"module":"esnext","moduleResolution":"bundler","resolveJsonModule":true,"isolatedModules":true,"jsx":"preserve","incremental":true,"plugins":[{"name":"next"}],"baseUrl":".","paths":{"@/*":["./src/*"]}},"include":["next-env.d.ts","**/*.ts","**/*.tsx",".next/types/**/*.ts"],"exclude":["node_modules"]}"#
}

fn canonical_postcss_config() -> &'static str {
    "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n"
}

fn canonical_tailwind_css() -> &'static str {
    "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n"
}

fn canonical_global_d_ts() -> &'static str {
    "declare module \"*.css\";\n"
}

fn canonical_layout_tsx() -> &'static str {
    r#"import type { Metadata } from "next";
import "./globals.css";

export const metadata: Metadata = {
  title: "Interactive Challenge",
  description: "A compact interactive challenge generated by anvilminimal",
};

export default function RootLayout({
  children,
}: Readonly<{ children: React.ReactNode }>) {
  return (
    <html lang="en">
      <body>{children}</body>
    </html>
  );
}
"#
}

fn missing_app_relative_import_contract_failure(root: &Path) -> Option<String> {
    let missing = missing_app_relative_imports(root).ok()?;
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "missing relative imports: {}",
        format_missing_import_findings(root, &missing).join("; ")
    ))
}

fn missing_app_relative_imports(root: &Path) -> anyhow::Result<Vec<MissingImport>> {
    let mut paths = project_app_source_paths(root);
    paths.extend(
        route_bound_closure(root, "nextjs")
            .into_iter()
            .map(|path| path.display().to_string()),
    );
    paths.sort();
    paths.dedup();
    scan_relative_imports(root, &paths)
}

fn repair_missing_css_import_artifacts(root: &Path) -> anyhow::Result<bool> {
    let mut changed = false;
    for missing in missing_app_relative_imports(root)?
        .into_iter()
        .filter(|missing| missing.specifier.ends_with(".css"))
    {
        let Some(target) = missing_import_target_path(root, &missing) else {
            continue;
        };
        let content = if is_app_global_stylesheet(root, &target) {
            canonical_tailwind_css()
        } else {
            ""
        };
        changed |= write_file_if_changed(&target, content)?;
    }
    Ok(changed)
}

fn is_app_global_stylesheet(root: &Path, path: &Path) -> bool {
    path.strip_prefix(root)
        .ok()
        .and_then(|path| path.to_str())
        .is_some_and(|rel| {
            matches!(
                rel.replace('\\', "/").as_str(),
                "src/app/globals.css" | "src/app/global.css" | "app/globals.css" | "app/global.css"
            )
        })
}

fn project_app_source_paths(root: &Path) -> Vec<String> {
    let mut paths = Vec::new();
    for rel in ["src/app", "app"] {
        collect_source_paths(root, Path::new(rel), &mut paths);
    }
    paths.sort();
    paths.dedup();
    paths
}

fn collect_source_paths(root: &Path, rel_dir: &Path, out: &mut Vec<String>) {
    let dir = root.join(rel_dir);
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let file_name = entry.file_name();
        let child_rel = rel_dir.join(file_name);
        if path.is_dir() {
            collect_source_paths(root, &child_rel, out);
        } else if is_import_scan_source_path(&path) {
            out.push(child_rel.display().to_string());
        }
    }
}

fn is_import_scan_source_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| matches!(ext, "js" | "jsx" | "ts" | "tsx"))
}

fn repair_postcss_plugins(root: &Path) -> anyhow::Result<bool> {
    let Some(path) = postcss_config_path(root) else {
        return write_file_if_changed(&root.join("postcss.config.js"), canonical_postcss_config());
    };
    let content = std::fs::read_to_string(&path).unwrap_or_default();
    let lower = content.to_ascii_lowercase();
    if !postcss_has_plugins_key(&lower) {
        return write_file_if_changed(&path, canonical_postcss_config());
    }
    let needs_tailwind = !(lower.contains("tailwindcss") || lower.contains("@tailwindcss/postcss"));
    let needs_autoprefixer = !lower.contains("autoprefixer");
    if !needs_tailwind && !needs_autoprefixer {
        return Ok(false);
    }
    let repaired = insert_missing_postcss_plugins(&content, needs_tailwind, needs_autoprefixer)
        .unwrap_or_else(|| canonical_postcss_config().to_string());
    write_file_if_changed(&path, &repaired)
}

fn repair_tailwind_module_format(root: &Path) -> anyhow::Result<bool> {
    let Some(path) = tailwind_config_paths(root)
        .into_iter()
        .find(|path| js_config_uses_esm_default_export(path))
    else {
        return Ok(false);
    };
    write_file_if_changed(&path, canonical_tailwind_config_cjs())
}

fn insert_missing_postcss_plugins(
    content: &str,
    needs_tailwind: bool,
    needs_autoprefixer: bool,
) -> Option<String> {
    let (open, close) = find_plugins_object_block(content)?;
    let base_indent = line_indent_before(content, open);
    let entry_indent = format!("{base_indent}  ");
    let body = &content[open + 1..close];
    let needs_separator = !body.trim().is_empty() && !body.trim_end().ends_with(',');
    let mut insertion = String::new();
    if needs_separator {
        insertion.push(',');
    }
    if needs_tailwind {
        insertion.push('\n');
        insertion.push_str(&entry_indent);
        insertion.push_str("tailwindcss: {},");
    }
    if needs_autoprefixer {
        insertion.push('\n');
        insertion.push_str(&entry_indent);
        insertion.push_str("autoprefixer: {},");
    }
    insertion.push('\n');
    insertion.push_str(&base_indent);

    let mut repaired = String::new();
    repaired.push_str(&content[..close]);
    repaired.push_str(&insertion);
    repaired.push_str(&content[close..]);
    Some(repaired)
}

fn find_plugins_object_block(content: &str) -> Option<(usize, usize)> {
    let lower = content.to_ascii_lowercase();
    let mut search_from = 0usize;
    while let Some(relative) = lower[search_from..].find("plugins") {
        let index = search_from + relative;
        let before = content[..index].chars().next_back();
        let after = content[index + "plugins".len()..].chars().next();
        if before.is_some_and(is_identifier_char) || after.is_some_and(is_identifier_char) {
            search_from = index + "plugins".len();
            continue;
        }
        let mut cursor = index + "plugins".len();
        cursor = skip_ascii_whitespace(content, cursor);
        if content[cursor..].starts_with(':') {
            cursor += 1;
        } else {
            search_from = cursor;
            continue;
        }
        cursor = skip_ascii_whitespace(content, cursor);
        if !content[cursor..].starts_with('{') {
            search_from = cursor;
            continue;
        }
        let close = find_matching_brace(content, cursor)?;
        return Some((cursor, close));
    }
    None
}

fn postcss_has_plugins_key(lower_content: &str) -> bool {
    let mut search_from = 0usize;
    while let Some(relative) = lower_content[search_from..].find("plugins") {
        let index = search_from + relative;
        let before = lower_content[..index].chars().next_back();
        let after = lower_content[index + "plugins".len()..].chars().next();
        if before.is_some_and(is_identifier_char) || after.is_some_and(is_identifier_char) {
            search_from = index + "plugins".len();
            continue;
        }
        return true;
    }
    false
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch == '$' || ch.is_ascii_alphanumeric()
}

fn skip_ascii_whitespace(content: &str, mut cursor: usize) -> usize {
    while let Some(ch) = content[cursor..].chars().next()
        && ch.is_ascii_whitespace()
    {
        cursor += ch.len_utf8();
    }
    cursor
}

fn find_matching_brace(content: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut string_quote: Option<char> = None;
    let mut escaped = false;
    let mut line_comment = false;
    let mut block_comment = false;
    let mut chars = content[open..].char_indices().peekable();
    while let Some((offset, ch)) = chars.next() {
        let index = open + offset;
        if line_comment {
            if ch == '\n' {
                line_comment = false;
            }
            continue;
        }
        if block_comment {
            if ch == '*'
                && let Some((_, '/')) = chars.peek().copied()
            {
                let _ = chars.next();
                block_comment = false;
            }
            continue;
        }
        if let Some(quote) = string_quote {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                string_quote = None;
            }
            continue;
        }
        if ch == '/'
            && let Some((_, next)) = chars.peek().copied()
        {
            if next == '/' {
                let _ = chars.next();
                line_comment = true;
                continue;
            }
            if next == '*' {
                let _ = chars.next();
                block_comment = true;
                continue;
            }
        }
        match ch {
            '"' | '\'' | '`' => string_quote = Some(ch),
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

fn line_indent_before(content: &str, index: usize) -> String {
    let line_start = content[..index].rfind('\n').map(|idx| idx + 1).unwrap_or(0);
    content[line_start..index]
        .chars()
        .take_while(|ch| ch.is_ascii_whitespace())
        .collect()
}

fn repair_tailwind_layout_import(root: &Path, reason: &str) -> anyhow::Result<bool> {
    let Some(css_path) = reported_tailwind_css_path(root, reason) else {
        return Ok(false);
    };
    let Some(layout_path) = app_layout_paths(root)
        .into_iter()
        .find(|path| path.is_file())
    else {
        return Ok(false);
    };
    let content = std::fs::read_to_string(&layout_path).unwrap_or_default();
    let import_path = css_import_path_for_layout(&layout_path, &css_path)
        .unwrap_or_else(|| "./globals.css".to_string());
    if css_imports_from_content(&content)
        .iter()
        .any(|existing| existing == &import_path)
    {
        return Ok(false);
    }
    let import_line = format!("import \"{import_path}\";\n");
    let repaired = insert_import_after_directives(&content, &import_line);
    write_file_if_changed(&layout_path, &repaired)
}

fn reported_tailwind_css_path(root: &Path, reason: &str) -> Option<PathBuf> {
    let files = tailwind_directive_files(root);
    files
        .iter()
        .find(|path| reason.contains(&path.display().to_string()))
        .cloned()
        .or_else(|| files.into_iter().next())
}

fn app_layout_paths(root: &Path) -> Vec<PathBuf> {
    [
        "src/app/layout.tsx",
        "src/app/layout.jsx",
        "src/app/layout.ts",
        "src/app/layout.js",
        "app/layout.tsx",
        "app/layout.jsx",
        "app/layout.ts",
        "app/layout.js",
    ]
    .iter()
    .map(|rel| root.join(rel))
    .collect()
}

fn css_import_path_for_layout(layout_path: &Path, css_path: &Path) -> Option<String> {
    let layout_dir = layout_path.parent()?;
    let relative = css_path.strip_prefix(layout_dir).ok()?;
    if relative.components().count() != 1 {
        return None;
    }
    Some(format!(
        "./{}",
        relative.to_string_lossy().replace('\\', "/")
    ))
}

fn insert_import_after_directives(content: &str, import_line: &str) -> String {
    let mut offset = 0usize;
    for line in content.split_inclusive('\n') {
        let trimmed = line.trim().trim_end_matches(';');
        if matches!(trimmed, "\"use client\"" | "'use client'") || trimmed.is_empty() {
            offset += line.len();
            continue;
        }
        break;
    }
    let mut repaired = String::new();
    repaired.push_str(&content[..offset]);
    repaired.push_str(import_line);
    repaired.push_str(&content[offset..]);
    repaired
}

fn fallback_page() -> &'static str {
    r#""use client";

import { useEffect, useMemo, useState } from "react";

type Hazard = { id: number; x: number; y: number; alive: boolean };
type Pulse = { x: number; y: number };

const columns = 9;
const rows = 4;

function initialHazards(): Hazard[] {
  return Array.from({ length: columns * rows }, (_, id) => ({
    id,
    x: 8 + (id % columns) * 10,
    y: 12 + Math.floor(id / columns) * 8,
    alive: true,
  }));
}

export default function Page() {
  const [player, setPlayer] = useState(50);
  const [pulses, setPulses] = useState<Pulse[]>([]);
  const [hazards, setHazards] = useState<Hazard[]>(() => initialHazards());
  const [tick, setTick] = useState(0);
  const [running, setRunning] = useState(true);
  const [lives, setLives] = useState(3);

  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === "ArrowLeft") setPlayer((value) => Math.max(5, value - 4));
      if (event.key === "ArrowRight") setPlayer((value) => Math.min(95, value + 4));
      if (event.key === " ") setPulses((value) => [...value, { x: player, y: 86 }].slice(-6));
      if (event.key.toLowerCase() === "r") {
        setHazards(initialHazards());
        setPulses([]);
        setRunning(true);
        setLives(3);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [player]);

  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => {
      setTick((value) => value + 1);
      setPulses((value) => value.map((pulse) => ({ ...pulse, y: pulse.y - 5 })).filter((pulse) => pulse.y > 4));
      setHazards((value) =>
        value.map((hazard) => ({
          ...hazard,
          x: hazard.x + Math.sin((tick + hazard.id) / 8) * 0.45,
          y: hazard.y + 0.035,
        })),
      );
    }, 70);
    return () => window.clearInterval(timer);
  }, [running, tick]);

  useEffect(() => {
    setHazards((current) =>
      current.map((hazard) => {
        if (!hazard.alive) return hazard;
        const hit = pulses.some((pulse) => Math.abs(pulse.x - hazard.x) < 3.2 && Math.abs(pulse.y - hazard.y) < 3.8);
        return hit ? { ...hazard, alive: false } : hazard;
      }),
    );
  }, [pulses]);

  const alive = hazards.filter((hazard) => hazard.alive).length;
  const score = useMemo(() => (columns * rows - alive) * 100, [alive]);

  useEffect(() => {
    const breach = hazards.some((hazard) => hazard.alive && hazard.y > 78);
    if (breach) setLives((value) => Math.max(0, value - 1));
    if (alive === 0 || breach || lives === 0) {
      setRunning(false);
    }
  }, [alive, hazards, lives]);

  return (
    <main className="screen">
      <section className="hud">
        <strong>INTERACTIVE CHALLENGE</strong>
        <span>SCORE {score}</span>
        <span>LIVES {lives}</span>
        <span>{running ? "LIVE" : alive === 0 ? "CLEAR" : "RESET READY"}</span>
      </section>
      <section className="arena" aria-label="Interactive challenge play field">
        <div className="stars" />
        {hazards.map((hazard) =>
          hazard.alive ? (
            <div
              className="hazard"
              key={hazard.id}
              style={{ left: `${hazard.x}%`, top: `${hazard.y}%` }}
            />
          ) : null,
        )}
        {pulses.map((pulse, index) => (
          <div className="pulse" key={`${pulse.x}-${pulse.y}-${index}`} style={{ left: `${pulse.x}%`, top: `${pulse.y}%` }} />
        ))}
        <div className="player" style={{ left: `${player}%` }} />
      </section>
      <nav className="controls">
        <button onClick={() => setPlayer((value) => Math.max(5, value - 5))}>Left</button>
        <button onClick={() => setPulses((value) => [...value, { x: player, y: 86 }].slice(-6))}>Action</button>
        <button onClick={() => setPlayer((value) => Math.min(95, value + 5))}>Right</button>
        <button
          onClick={() => {
            setHazards(initialHazards());
            setPulses([]);
            setRunning(true);
            setLives(3);
          }}
        >
          Reset
        </button>
      </nav>
      <style jsx>{`
        .screen {
          min-height: 100vh;
          padding: 24px;
          display: grid;
          grid-template-rows: auto 1fr auto;
          gap: 16px;
          background: #05070d;
          color: #edfaff;
          font-family: ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
        }
        .hud, .controls {
          display: flex;
          justify-content: center;
          gap: 12px;
          flex-wrap: wrap;
        }
        .hud span, .hud strong, .controls button {
          border: 1px solid rgba(129, 245, 255, 0.45);
          background: rgba(5, 12, 24, 0.72);
          color: #effcff;
          padding: 10px 14px;
          border-radius: 6px;
          box-shadow: 0 0 18px rgba(0, 229, 255, 0.16);
        }
        .controls button { cursor: pointer; min-width: 84px; }
        .arena {
          position: relative;
          overflow: hidden;
          min-height: 560px;
          border: 1px solid rgba(129, 245, 255, 0.36);
          background: rgba(2, 4, 12, 0.84);
          box-shadow: inset 0 0 70px rgba(0, 229, 255, 0.12);
        }
        .stars {
          position: absolute;
          inset: 0;
          background-image: radial-gradient(#fff 1px, transparent 1px);
          background-size: 31px 29px;
          opacity: 0.2;
        }
        .hazard, .pulse, .player { position: absolute; transform: translate(-50%, -50%); }
        .hazard {
          width: 24px;
          height: 18px;
          background: #7dffbf;
          clip-path: polygon(12% 0, 88% 0, 100% 35%, 70% 35%, 70% 70%, 88% 70%, 88% 100%, 60% 78%, 40% 78%, 12% 100%, 12% 70%, 30% 70%, 30% 35%, 0 35%);
          filter: drop-shadow(0 0 12px #7dffbf);
        }
        .pulse {
          width: 4px;
          height: 20px;
          border-radius: 999px;
          background: #ffec7d;
          box-shadow: 0 0 14px #ffec7d;
        }
        .player {
          bottom: 28px;
          width: 46px;
          height: 30px;
          background: #7dc7ff;
          clip-path: polygon(50% 0, 100% 100%, 66% 82%, 34% 82%, 0 100%);
          filter: drop-shadow(0 0 16px #7dc7ff);
        }
      `}</style>
    </main>
  );
}
"#
}

fn find_entrypoint(root: &Path) -> Option<EntryPoint> {
    for (rel, app_dir) in [
        ("src/app/page.tsx", "src/app"),
        ("src/app/page.jsx", "src/app"),
        ("src/app/page.ts", "src/app"),
        ("src/app/page.js", "src/app"),
        ("app/page.tsx", "app"),
        ("app/page.jsx", "app"),
        ("app/page.ts", "app"),
        ("app/page.js", "app"),
    ] {
        if root.join(rel).is_file() {
            return Some(EntryPoint {
                app_dir: app_dir.to_string(),
                requires_layout: true,
            });
        }
    }
    for rel in [
        "pages/index.tsx",
        "pages/index.jsx",
        "pages/index.ts",
        "pages/index.js",
        "src/pages/index.tsx",
        "src/pages/index.jsx",
        "src/pages/index.ts",
        "src/pages/index.js",
    ] {
        if root.join(rel).is_file() {
            return Some(EntryPoint {
                app_dir: String::new(),
                requires_layout: false,
            });
        }
    }
    None
}

fn find_app_layout(root: &Path, app_dir: &str) -> Option<PathBuf> {
    ["layout.tsx", "layout.jsx", "layout.ts", "layout.js"]
        .iter()
        .map(|name| root.join(app_dir).join(name))
        .find(|path| path.is_file())
}

fn contains_in_files(root: &Path, needle: &str) -> bool {
    for rel in [
        "app/page.tsx",
        "app/page.jsx",
        "pages/index.tsx",
        "pages/index.jsx",
        "src/app/page.tsx",
        "src/app/page.jsx",
        "src/app/globals.css",
        "app/globals.css",
        "src/pages/index.tsx",
        "src/pages/index.jsx",
    ] {
        if std::fs::read_to_string(root.join(rel)).is_ok_and(|content| content.contains(needle)) {
            return true;
        }
    }
    false
}

fn is_weakened_script(value: &str) -> bool {
    let value = value.trim().to_ascii_lowercase();
    value.is_empty()
        || value == "true"
        || value == "echo ok"
        || value == "echo done"
        || value.starts_with("echo ")
}

fn tsconfig_contract_failure(root: &Path) -> Option<String> {
    let path = root.join("tsconfig.json");
    if !path.is_file() {
        return None;
    }
    let content = std::fs::read_to_string(path).ok()?;
    let value: Value = serde_json::from_str(&content).ok()?;
    let compiler = value.get("compilerOptions").and_then(Value::as_object)?;
    if compiler
        .get("moduleResolution")
        .and_then(Value::as_str)
        .is_some_and(|value| value.eq_ignore_ascii_case("node10"))
    {
        return Some(
            "tsconfig.moduleResolution=node10 is deprecated for Next.js builds; use bundler or node16"
                .to_string(),
        );
    }
    let root_dir = compiler.get("rootDir").and_then(Value::as_str)?;
    if !matches!(root_dir, "." | "./") {
        Some("tsconfig.rootDir must not constrain Next.js generated files".to_string())
    } else {
        None
    }
}

fn css_side_effect_import_contract_failure(root: &Path) -> Option<String> {
    let imports_css = [
        "src/app/layout.tsx",
        "src/app/layout.ts",
        "app/layout.tsx",
        "app/layout.ts",
    ]
    .iter()
    .any(|rel| {
        std::fs::read_to_string(root.join(rel))
            .is_ok_and(|content| content.contains(".css\"") || content.contains(".css'"))
    });
    if !imports_css {
        return None;
    }
    if css_module_declaration_exists(root) {
        None
    } else {
        Some(
            "CSS side-effect imports require a declaration file such as src/app/global.d.ts with declare module \"*.css\""
                .to_string(),
        )
    }
}

fn css_module_declaration_exists(root: &Path) -> bool {
    for rel in [
        "src/app/global.d.ts",
        "src/global.d.ts",
        "global.d.ts",
        "app/global.d.ts",
    ] {
        if std::fs::read_to_string(root.join(rel)).is_ok_and(|content| {
            content.contains("declare module \"*.css\"")
                || content.contains("declare module '*.css'")
        }) {
            return true;
        }
    }
    false
}

fn alias_configured(tsconfig: &Value) -> bool {
    let Some(compiler) = tsconfig.get("compilerOptions").and_then(Value::as_object) else {
        return false;
    };
    let Some(base_url) = compiler.get("baseUrl").and_then(Value::as_str) else {
        return false;
    };
    if !matches!(base_url, "." | "./") {
        return false;
    }
    compiler
        .get("paths")
        .and_then(Value::as_object)
        .and_then(|paths| paths.get("@/*"))
        .and_then(Value::as_array)
        .is_some_and(|values| {
            values.iter().any(|value| {
                matches!(
                    value.as_str(),
                    Some("./src/*") | Some("src/*") | Some("./*") | Some("*")
                )
            })
        })
}

fn tailwind_contract_failure(root: &Path, package: &Value) -> Option<String> {
    if !uses_tailwind(root, package) {
        return None;
    }
    for dep in ["tailwindcss", "postcss", "autoprefixer"] {
        if !package_has_dependency(package, dep) {
            return Some(tailwind_failure(format!(
                "Tailwind toolchain dependency missing: {dep}"
            )));
        }
    }
    let tailwind_configs = tailwind_config_paths(root);
    if tailwind_configs.is_empty() {
        return Some(tailwind_failure(format!(
            "Tailwind config file missing: expected {}",
            setup_tailwind_config_rel(root)
        )));
    }
    if tailwind_configs.len() > 1 {
        let names = tailwind_configs
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Some(tailwind_failure(format!(
            "exactly one Tailwind config file is allowed: {names}"
        )));
    }
    let Some(postcss_config) = postcss_config_path(root) else {
        return Some(tailwind_failure("PostCSS config file missing for Tailwind"));
    };
    if let Some(reason) = module_format_contract_failure(
        package,
        &postcss_config,
        "PostCSS config",
        "use CommonJS module.exports or rename the config to .mjs/add package.json type module",
    ) {
        return Some(tailwind_failure(reason));
    }
    for tailwind_config in &tailwind_configs {
        if let Some(reason) = tailwind_module_format_contract_failure(package, tailwind_config) {
            return Some(tailwind_failure(reason));
        }
    }
    let postcss_config = std::fs::read_to_string(postcss_config).unwrap_or_default();
    let postcss_lower = postcss_config.to_ascii_lowercase();
    if find_plugins_object_block(&postcss_config).is_none() {
        return Some(tailwind_failure("PostCSS config must export a plugins key"));
    }
    if !(postcss_lower.contains("tailwindcss") || postcss_lower.contains("@tailwindcss/postcss")) {
        return Some(tailwind_failure(
            "PostCSS config must include the Tailwind plugin",
        ));
    }
    if !postcss_lower.contains("autoprefixer") {
        return Some(tailwind_failure(
            "PostCSS config must include autoprefixer for Tailwind",
        ));
    }
    let tailwind_css_files = tailwind_directive_files(root);
    if !tailwind_css_files.is_empty() {
        let imported = imported_app_css_paths(root);
        if !tailwind_css_files
            .iter()
            .any(|path| imported.iter().any(|imported| imported == path))
        {
            let css_list = tailwind_css_files
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Some(tailwind_failure(format!(
                "@tailwind CSS file must be imported by app layout: {css_list}"
            )));
        }
    }
    None
}

fn tailwind_failure(message: impl AsRef<str>) -> String {
    format!("tailwind_contract_failure: {}", message.as_ref())
}

fn plain_css_without_tailwind_artifacts(root: &Path) -> bool {
    let package = std::fs::read_to_string(root.join("package.json"))
        .ok()
        .and_then(|content| serde_json::from_str::<Value>(&content).ok())
        .unwrap_or(Value::Null);
    !uses_tailwind(root, &package)
}

fn tailwind_stack_scaffold_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with("postcss.config.js")
        || lower.ends_with("postcss.config.cjs")
        || lower.ends_with("postcss.config.mjs")
        || lower.ends_with("postcss.config")
        || lower.contains("tailwind.config.")
}

fn client_component_contract_failure(root: &Path) -> Option<String> {
    for rel in [
        "src/app/page.tsx",
        "src/app/page.jsx",
        "src/app/page.ts",
        "src/app/page.js",
        "app/page.tsx",
        "app/page.jsx",
        "app/page.ts",
        "app/page.js",
    ] {
        let Ok(content) = std::fs::read_to_string(root.join(rel)) else {
            continue;
        };
        if uses_client_only_features(&content) && !has_use_client_directive(&content) {
            return Some(format!(
                "{rel} uses browser/client APIs and must start with \"use client\""
            ));
        }
    }
    None
}

fn uses_client_only_features(content: &str) -> bool {
    let lower = content.to_ascii_lowercase();
    [
        "usestate",
        "useeffect",
        "useref",
        "usereducer",
        "window.",
        "document.",
        "addeventlistener",
        "requestanimationframe",
        "setinterval",
        "settimeout",
        "onclick=",
        "onkeydown=",
        "onkeyup=",
        "onpointer",
        "onmouse",
        "ref={",
        "<canvas",
    ]
    .iter()
    .any(|needle| lower.contains(needle))
}

fn has_use_client_directive(content: &str) -> bool {
    content
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with("//"))
        .map(|line| {
            let line = line.strip_suffix(';').unwrap_or(line);
            matches!(line, "\"use client\"" | "'use client'")
        })
        .unwrap_or(false)
}

fn uses_tailwind(root: &Path, package: &Value) -> bool {
    package_has_dependency(package, "tailwindcss")
        || !tailwind_directive_files(root).is_empty()
        || has_tailwind_config(root)
        || postcss_config_references_tailwind(root)
}

fn has_tailwind_config(root: &Path) -> bool {
    !tailwind_config_paths(root).is_empty()
}

fn tailwind_config_paths(root: &Path) -> Vec<PathBuf> {
    TAILWIND_CONFIG_RELS
        .iter()
        .map(|rel| root.join(rel))
        .filter(|path| path.is_file())
        .collect()
}

fn postcss_config_references_tailwind(root: &Path) -> bool {
    let Some(path) = postcss_config_path(root) else {
        return false;
    };
    std::fs::read_to_string(path)
        .is_ok_and(|content| content.to_ascii_lowercase().contains("tailwind"))
}

fn postcss_config_path(root: &Path) -> Option<PathBuf> {
    [
        "postcss.config.js",
        "postcss.config.mjs",
        "postcss.config.cjs",
        "postcss.config",
    ]
    .iter()
    .map(|rel| root.join(rel))
    .find(|path| path.is_file())
}

fn module_format_contract_failure(
    package: &Value,
    path: &Path,
    label: &str,
    remedy: &str,
) -> Option<String> {
    if package_type_module(package) || !js_config_uses_esm_default_export(path) {
        return None;
    }
    Some(format!(
        "{label} uses ESM export default but package.json lacks \"type\":\"module\"; {remedy}"
    ))
}

fn tailwind_module_format_contract_failure(package: &Value, path: &Path) -> Option<String> {
    module_format_contract_failure(
        package,
        path,
        "Tailwind config",
        "use CommonJS module.exports or rename the config to .mjs/add package.json type module",
    )
}

fn package_type_module(package: &Value) -> bool {
    package
        .get("type")
        .and_then(Value::as_str)
        .is_some_and(|value| value == "module")
}

fn js_config_uses_esm_default_export(path: &Path) -> bool {
    if !is_common_js_module_boundary(path) {
        return false;
    }
    std::fs::read_to_string(path).is_ok_and(|content| {
        content
            .lines()
            .any(|line| line.trim_start().starts_with("export default"))
    })
}

fn is_common_js_module_boundary(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
        return false;
    };
    if name.ends_with(".mjs") || name.ends_with(".cjs") || name.ends_with(".ts") {
        return false;
    }
    name.ends_with(".js") || matches!(name, "postcss.config" | "tailwind.config")
}

fn tailwind_directive_files(root: &Path) -> Vec<PathBuf> {
    [
        "src/app/globals.css",
        "src/app/global.css",
        "app/globals.css",
        "app/global.css",
        "src/styles/globals.css",
        "styles/globals.css",
    ]
    .iter()
    .filter_map(|rel| {
        let path = root.join(rel);
        std::fs::read_to_string(&path)
            .ok()
            .filter(|content| content.contains("@tailwind"))
            .map(|_| path)
    })
    .collect()
}

fn imported_app_css_paths(root: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for rel in [
        "src/app/layout.tsx",
        "src/app/layout.jsx",
        "src/app/layout.ts",
        "src/app/layout.js",
        "app/layout.tsx",
        "app/layout.jsx",
        "app/layout.ts",
        "app/layout.js",
    ] {
        let layout_path = root.join(rel);
        let Ok(content) = std::fs::read_to_string(&layout_path) else {
            continue;
        };
        let layout_dir = layout_path.parent().unwrap_or(root);
        for import in css_imports_from_content(&content) {
            let path = layout_dir
                .join(import.trim_start_matches("./"))
                .components()
                .collect::<PathBuf>();
            paths.push(path);
        }
    }
    paths
}

fn css_imports_from_content(content: &str) -> Vec<String> {
    let mut imports = Vec::new();
    for quote in ['"', '\''] {
        let mut parts = content.split(quote);
        while let Some(_) = parts.next() {
            let Some(candidate) = parts.next() else {
                break;
            };
            if candidate.ends_with(".css") {
                imports.push(candidate.to_string());
            }
        }
    }
    imports
}

fn package_has_dependency(package: &Value, name: &str) -> bool {
    ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|key| package.get(*key).and_then(Value::as_object))
        .any(|deps| deps.contains_key(name))
}

fn dependency_coherence_failure(package: &Value) -> Option<String> {
    let next = dependency_version(package, "next")?;
    let react = dependency_version(package, "react")?;
    let react_dom = dependency_version(package, "react-dom")?;
    if dependency_version_needs_repair(
        "typescript",
        dependency_version(package, "typescript").unwrap_or(""),
    ) {
        return Some(
            "typescript dependency must use a deterministic 5.x range such as ^5.5.0".to_string(),
        );
    }
    let next_major = semver_major(next)?;
    let react_major = semver_major(react)?;
    let react_dom_major = semver_major(react_dom)?;
    if next_major >= 15 && (react_major < 19 || react_dom_major < 19) {
        return Some("Next 15+ requires React/React DOM 19.x compatibility".to_string());
    }
    if next_major <= 14 && (react_major != 18 || react_dom_major != 18) {
        return Some("Next 14 profile expects React/React DOM 18.x compatibility".to_string());
    }
    if let Some(types_react) = dependency_version(package, "@types/react")
        && let Some(types_major) = semver_major(types_react)
        && ((react_major >= 19 && types_major < 19) || (react_major == 18 && types_major != 18))
    {
        return Some("@types/react major must match React major".to_string());
    }
    if let Some(types_react_dom) = dependency_version(package, "@types/react-dom")
        && let Some(types_major) = semver_major(types_react_dom)
        && ((react_dom_major >= 19 && types_major < 19)
            || (react_dom_major == 18 && types_major != 18))
    {
        return Some("@types/react-dom major must match React DOM major".to_string());
    }
    None
}

fn dependency_version<'a>(package: &'a Value, name: &str) -> Option<&'a str> {
    ["dependencies", "devDependencies"]
        .iter()
        .filter_map(|key| package.get(*key).and_then(Value::as_object))
        .find_map(|deps| deps.get(name).and_then(Value::as_str))
}

fn dependency_version_needs_repair(name: &str, version: &str) -> bool {
    if version.trim().is_empty() {
        return true;
    }
    match name {
        "typescript" => {
            let Some(major) = semver_major(version) else {
                return false;
            };
            major != 5 || version.trim() == "5.0.0"
        }
        "@types/node" => semver_major(version).is_none_or(|major| major != 20),
        "@types/react" | "@types/react-dom" => {
            semver_major(version).is_none_or(|major| major != 18)
        }
        "next" => semver_major(version).is_none_or(|major| major != 14),
        "react" | "react-dom" => semver_major(version).is_none_or(|major| major != 18),
        _ => false,
    }
}

fn semver_major(version: &str) -> Option<u64> {
    let trimmed = version.trim();
    let digits = trimmed
        .trim_start_matches(['^', '~', '=', 'v'])
        .split(|ch: char| !ch.is_ascii_digit())
        .next()
        .unwrap_or_default();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::lint::lint_step_plan_report_with_workspace;
    use crate::planner::sanitizer::sanitize_step_plan_against_policy;
    use crate::planner::step_plan::repair_generated_step_plan_contract;
    use crate::planner::verify::VerifyStatus;
    use crate::planner::verify::normalize_verify_command;

    fn package_json() -> &'static str {
        r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#
    }

    #[test]
    fn nextjs_guidance_includes_interaction_observability_hooks() {
        let text = format!(
            "{}\n{}\n{}",
            generation_rules("create"),
            guidance("Create an interactive todo app"),
            runtime_contract("create", "Create an interactive game")
        );

        assert!(text.contains("data-anvil-action=\"primary\""), "{text}");
        assert!(text.contains("data-anvil-action=\"input\""), "{text}");
        assert!(text.contains("data-anvil-state"), "{text}");
        assert!(text.contains("extend the instrumented skeleton"), "{text}");
        assert!(text.contains("JSON snapshot"), "{text}");
        assert!(
            text.contains("immediately responds to input")
                && text.contains("player/paddle x position"),
            "{text}"
        );
        assert!(
            text.contains("every restart affordance")
                && text.contains("data-anvil-action=\"restart\"")
                && text
                    .contains("initial primary action alone cannot satisfy recovery verification"),
            "{text}"
        );
        assert!(text.contains("hook or R-key"), "{text}");
        assert!(
            text.contains("unverified:terminal_state_not_reached"),
            "{text}"
        );
    }

    #[test]
    fn nextjs_evidence_repair_targets_route_bound_behavioral_source() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("package.json"), package_json()).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "import Game from './game';\nexport default function Page(){ return <Game />; }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/game.tsx"),
            "export default function Game(){ return <button data-anvil-action=\"restart\">Restart</button>; }\n",
        )
        .unwrap();

        let targets = evidence_repair_target_paths(
            dir.path(),
            &["restart_or_recoverable_state_evidence".to_string()],
        );

        assert_eq!(
            targets.first().map(String::as_str),
            Some("src/app/game.tsx")
        );
        assert!(targets.iter().any(|path| path == "src/app/page.tsx"));
    }

    #[test]
    fn nextjs_hook_snapshot_targets_route_bound_sources() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("package.json"), package_json()).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "import Game from './game';\nexport default function Page(){ return <Game />; }\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/game.tsx"),
            "export default function Game(){ return <main data-anvil-state=\"{}\"><button data-anvil-action=\"primary\">Start</button><button data-anvil-action=\"restart\">Restart</button></main>; }\n",
        )
        .unwrap();

        let targets = hook_snapshot_targets(dir.path());
        let paths = targets
            .iter()
            .map(|target| target.relative_path.as_str())
            .collect::<Vec<_>>();

        assert!(paths.contains(&"src/app/page.tsx"), "{paths:?}");
        assert!(paths.contains(&"src/app/game.tsx"), "{paths:?}");
        assert!(
            targets
                .iter()
                .all(|target| target.required_attributes.len() == 3)
        );
    }

    #[test]
    fn nextjs_deterministic_scaffold_prompt_returns_template() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "Original ultra goal: Build a browser game on port 3011\n\
Phase id: project-setup\n\
Phase task: Scaffold and initialize the Next.js project shell";

        let template = deterministic_step_plan(prompt, dir.path(), prompt).unwrap();

        assert_eq!(template.template_id, "nextjs-scaffold");
        assert!(
            template.plan.steps[0]
                .expected_paths
                .contains(&"src/app/page.tsx".to_string())
        );
        assert!(
            template
                .plan
                .steps
                .iter()
                .flat_map(|step| step.verify.iter())
                .any(|command| command == "npm run build")
        );
    }

    #[test]
    fn nextjs_deterministic_implementation_prompt_falls_back_to_planner() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "Original ultra goal: Build a browser game on port 3011\n\
Phase id: gameplay\n\
Phase task: Implement game logic, player control, collision, score, and canvas behavior";

        assert!(deterministic_step_plan(prompt, dir.path(), prompt).is_none());
    }

    #[test]
    fn nextjs_deterministic_template_passes_sanitizer_and_lint() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "Original ultra goal: Build a Next.js app on port 3011\n\
Phase id: project-setup\n\
Phase task: Scaffold the Next.js app and configure package scripts";
        let mut plan = deterministic_step_plan(prompt, dir.path(), prompt)
            .unwrap()
            .plan;

        repair_generated_step_plan_contract(&mut plan);
        let report = sanitize_step_plan_against_policy(&mut plan, Some(dir.path()));
        assert!(report.shell_control_splits.is_empty(), "{report:?}");
        for command in plan.steps.iter().flat_map(|step| step.verify.iter()) {
            normalize_verify_command(command).unwrap();
        }
        assert!(
            lint_step_plan_report_with_workspace(&plan, Some(dir.path())).is_pass(),
            "{plan:?}"
        );
    }

    #[test]
    fn nextjs_preset_ultra_plan_create_default_has_four_phases_and_goal() {
        let goal = "Build a Next.js score tracker";
        let plan = preset_ultra_plan(goal, "default", "create").unwrap();

        assert_eq!(plan.goal, goal);
        assert_eq!(plan.profile, "nextjs");
        assert_eq!(plan.style, "default");
        assert_eq!(plan.intent, "create");
        assert_eq!(plan.phases.len(), 4);
        assert_eq!(plan.phases[0].id, "project-setup");
        assert_eq!(plan.phases[3].id, "build-verification");
        assert!(plan.phases[1].prompt.contains(goal));
        assert!(
            plan.phases[2]
                .prompt
                .contains("immediately responds to input"),
            "{plan:?}"
        );
    }

    #[test]
    fn nextjs_preset_ultra_plan_skips_fix_and_non_default_styles() {
        assert!(preset_ultra_plan("Build an app", "default", "fix").is_none());
        assert!(preset_ultra_plan("Build an app", "tdd", "create").is_none());
        assert!(preset_ultra_plan("Build an app", "test-hardening", "create").is_none());
    }

    #[test]
    fn nextjs_preset_ultra_plan_phases_hit_deterministic_templates() {
        let dir = tempfile::tempdir().unwrap();
        let plan =
            preset_ultra_plan("Build a browser game on port 3011", "default", "create").unwrap();
        let scaffold_prompt = format!(
            "Original ultra goal: {}\nPhase id: {}\nPhase task: {}",
            plan.goal, plan.phases[0].id, plan.phases[0].prompt
        );
        let build_prompt = format!(
            "Original ultra goal: {}\nPhase id: {}\nPhase task: {}",
            plan.goal, plan.phases[3].id, plan.phases[3].prompt
        );

        assert_eq!(
            deterministic_step_plan(&scaffold_prompt, dir.path(), &plan.goal)
                .unwrap()
                .template_id,
            "nextjs-scaffold"
        );
        assert_eq!(
            deterministic_step_plan(&build_prompt, dir.path(), &plan.goal)
                .unwrap()
                .template_id,
            "nextjs-build-verification"
        );
    }

    #[test]
    fn nextjs_preset_ultra_plan_passes_lint_and_template_sanitizer() {
        let dir = tempfile::tempdir().unwrap();
        let plan =
            preset_ultra_plan("Build a browser game on port 3011", "default", "create").unwrap();

        assert!(
            crate::planner::lint::lint_ultra_plan_report(&plan).is_pass(),
            "{plan:?}"
        );
        for phase in [&plan.phases[0], &plan.phases[3]] {
            let prompt = format!(
                "Original ultra goal: {}\nPhase id: {}\nPhase task: {}",
                plan.goal, phase.id, phase.prompt
            );
            let mut step_plan = deterministic_step_plan(&prompt, dir.path(), &plan.goal)
                .unwrap()
                .plan;
            repair_generated_step_plan_contract(&mut step_plan);
            let report = sanitize_step_plan_against_policy(&mut step_plan, Some(dir.path()));
            assert!(report.shell_control_splits.is_empty(), "{report:?}");
            for command in step_plan.steps.iter().flat_map(|step| step.verify.iter()) {
                normalize_verify_command(command).unwrap();
            }
            assert!(
                lint_step_plan_report_with_workspace(&step_plan, Some(dir.path())).is_pass(),
                "{step_plan:?}"
            );
        }
    }

    #[test]
    fn nextjs_deterministic_template_preserves_required_final_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let prompt = "Required final artifacts:\n\
- src/app/page.tsx\n\
- src/components/Game.tsx\n\n\
Original ultra goal: Build a Next.js app on port 3011\n\
Phase id: project-setup\n\
Phase task: Scaffold the Next.js app";

        let template = deterministic_step_plan(prompt, dir.path(), prompt).unwrap();
        let paths = template
            .plan
            .steps
            .iter()
            .flat_map(|step| step.expected_paths.iter())
            .cloned()
            .collect::<Vec<_>>();

        assert!(paths.contains(&"src/app/page.tsx".to_string()));
        assert!(paths.contains(&"src/components/Game.tsx".to_string()));
    }

    #[test]
    fn nextjs_default_port_required_when_goal_has_no_port() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev"}}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "Next.jsアプリを作成してください");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("port 3011")
        ));
    }

    #[test]
    fn nextjs_no_port_repair_writes_default_dev_and_start_scripts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3000","start":"next start -p 3000"}}"#,
        )
        .unwrap();
        let report = verify_invariant(dir.path(), "ブラウザで使えるメモアプリ");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(ref reason) if reason.contains("port 3011")
        ));

        assert!(repair_manifest_coherence(dir.path(), "ブラウザで使えるメモアプリ").unwrap());

        assert_eq!(
            package_script(dir.path(), "dev").as_deref(),
            Some("next dev -p 3011")
        );
        assert_eq!(
            package_script(dir.path(), "start").as_deref(),
            Some("next start -p 3011")
        );
        assert!(verify_invariant(dir.path(), "ブラウザで使えるメモアプリ").is_pass());
    }

    #[test]
    fn nextjs_verify_rejects_route_bound_missing_named_export_before_build() {
        let dir = complete_app();
        std::fs::create_dir_all(dir.path().join("src/components")).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "import SpaceInvaders from '../components/SpaceInvaders'; export default function Page(){ return <SpaceInvaders/>; }",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/components/SpaceInvaders.tsx"),
            "import { CANVAS_W } from './game-engine'; export default function SpaceInvaders(){ return <canvas width={CANVAS_W}/>; }",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/components/game-engine.ts"),
            "export const CANVAS_H = 600;\n",
        )
        .unwrap();

        let report = verify(dir.path(), "3011");
        let reason = report.primary_reason();

        assert!(reason.contains("missing relative imports"), "{reason}");
        assert!(reason.contains("does not export CANVAS_W"), "{reason}");

        std::fs::write(
            dir.path().join("src/components/game-engine.ts"),
            "export const CANVAS_W = 800;\nexport const CANVAS_H = 600;\n",
        )
        .unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_verify_rejects_route_bound_jsx_in_ts_before_build() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "import { GameView } from './game-engine'; export default function Page(){ return <GameView/>; }",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/game-engine.ts"),
            "export function GameView(){ return <div data-testid=\"game\" />; }\n",
        )
        .unwrap();

        let report = verify(dir.path(), "3011");
        let reason = report.primary_reason();

        assert!(reason.contains("missing relative imports"), "{reason}");
        assert!(
            reason.contains("rename it to .tsx or remove JSX"),
            "{reason}"
        );
    }

    #[test]
    fn nextjs_explicit_port_wins_over_default_repair() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3000","start":"next start -p 3000"}}"#,
        )
        .unwrap();

        assert!(repair_manifest_coherence(dir.path(), "4000番ポートで起動").unwrap());

        assert_eq!(
            package_script(dir.path(), "dev").as_deref(),
            Some("next dev -p 4000")
        );
        assert_eq!(
            package_script(dir.path(), "start").as_deref(),
            Some("next start -p 4000")
        );
        assert!(verify_invariant(dir.path(), "4000番ポートで起動").is_pass());
    }

    #[test]
    fn nextjs_requested_port_is_not_3011_specific() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev"}}"#,
        )
        .unwrap();
        let report = verify_invariant(dir.path(), "4000番ポートで起動");
        assert!(
            matches!(
                report.status,
                VerifyStatus::ProfileContractFailed(ref reason) if reason.contains("port 4000")
            ),
            "{report:?}"
        );

        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 4000"}}"#,
        )
        .unwrap();
        assert!(verify_invariant(dir.path(), "4000番ポートで起動").is_pass());
    }

    #[test]
    fn nextjs_requires_entrypoint() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), package_json()).unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("entrypoint")
        ));
    }

    #[test]
    fn nextjs_invariant_allows_pending_entrypoint() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), package_json()).unwrap();
        assert!(verify_invariant(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_invariant_rejects_weakened_build_script() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"echo ok","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        let report = verify_invariant(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("scripts.build")
        ));
    }

    fn package_script(root: &Path, name: &str) -> Option<String> {
        let text = std::fs::read_to_string(root.join("package.json")).ok()?;
        let value = serde_json::from_str::<Value>(&text).ok()?;
        value
            .get("scripts")?
            .get(name)?
            .as_str()
            .map(str::to_string)
    }

    #[test]
    fn nextjs_accepts_single_nested_complete_project() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("space-invaders");
        std::fs::create_dir_all(app.join("src/app")).unwrap();
        std::fs::write(app.join("package.json"), package_json()).unwrap();
        std::fs::write(
            app.join("src/app/page.tsx"),
            "export default function Page(){return null;}",
        )
        .unwrap();
        std::fs::write(app.join("src/app/layout.tsx"), "export default function Layout({children}:{children:React.ReactNode}){return children;}").unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn expected_paths_follow_existing_nested_project() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("space-invaders");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(app.join("package.json"), package_json()).unwrap();
        assert_eq!(
            expected_paths(dir.path(), "Implement game"),
            vec![
                "space-invaders/package.json",
                "space-invaders/tsconfig.json",
                "space-invaders/postcss.config.js",
                "space-invaders/tailwind.config.ts",
                "space-invaders/src/app/layout.tsx",
                "space-invaders/src/app/page.tsx",
                "space-invaders/src/app/globals.css",
                "space-invaders/src/app/global.d.ts"
            ]
        );
    }

    #[test]
    fn expected_paths_do_not_degrade_when_goal_mentions_scaffold() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(
            expected_paths(dir.path(), "Scaffold a polished Next.js app"),
            setup_scaffold_paths(dir.path())
        );
        assert_eq!(
            expected_paths(dir.path(), "Scaffold a polished Next.js app"),
            vec![
                "package.json",
                "tsconfig.json",
                "postcss.config.js",
                "tailwind.config.ts",
                "src/app/layout.tsx",
                "src/app/page.tsx",
                "src/app/globals.css",
                "src/app/global.d.ts"
            ]
        );
    }

    #[test]
    fn setup_invariant_required_paths_are_scaffold_paths() {
        let dir = tempfile::tempdir().unwrap();
        let fallback_paths = setup_scaffold_paths(dir.path());

        for path in setup_invariant_required_paths(dir.path()) {
            assert!(
                fallback_paths.contains(&path),
                "invariant path {path} missing from fallback paths {fallback_paths:?}"
            );
        }
    }

    #[test]
    fn setup_scaffold_paths_use_existing_single_tailwind_config() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("package.json"), package_json()).unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.js"),
            "module.exports = {};\n",
        )
        .unwrap();

        let paths = setup_scaffold_paths(dir.path());

        assert!(paths.contains(&"tailwind.config.js".to_string()));
        assert!(!paths.contains(&"tailwind.config.ts".to_string()));
    }

    #[test]
    fn complete_scaffold_creates_only_absent_nextjs_config_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        let package_before = package_json().to_string();
        let page_before = "export default function Page(){return <main>done</main>;}\n".to_string();
        std::fs::write(dir.path().join("package.json"), &package_before).unwrap();
        std::fs::write(dir.path().join("tsconfig.json"), canonical_tsconfig()).unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            canonical_layout_tsx(),
        )
        .unwrap();
        std::fs::write(dir.path().join("src/app/page.tsx"), &page_before).unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            canonical_tailwind_css(),
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            canonical_global_d_ts(),
        )
        .unwrap();

        let created = complete_scaffold(
            dir.path(),
            &[
                "postcss.config.js".to_string(),
                "tailwind.config.ts".to_string(),
            ],
        )
        .unwrap();

        assert_eq!(
            created,
            vec![
                "postcss.config.js".to_string(),
                "tailwind.config.ts".to_string()
            ]
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap(),
            canonical_postcss_config()
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("tailwind.config.ts")).unwrap(),
            canonical_tailwind_config()
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
            package_before
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap(),
            page_before
        );
    }

    #[test]
    fn complete_scaffold_authors_absent_application_page() {
        let dir = tempfile::tempdir().unwrap();
        let created = complete_scaffold(dir.path(), &["src/app/page.tsx".to_string()]).unwrap();

        assert_eq!(created, vec!["src/app/page.tsx".to_string()]);
        let page = std::fs::read_to_string(dir.path().join("src/app/page.tsx")).unwrap();
        assert!(page.contains("export default function Page"), "{page}");
        assert!(page.contains("INTERACTIVE CHALLENGE"), "{page}");
    }

    #[test]
    fn before_phase_pre_provisions_missing_nextjs_scaffold() {
        let dir = tempfile::tempdir().unwrap();
        crate::planner::profile::profile_before_phase(dir.path(), "nextjs").unwrap();
        assert!(dir.path().join("package.json").is_file());
        assert!(dir.path().join("src/app/page.tsx").is_file());
        assert!(dir.path().join("src/app/layout.tsx").is_file());
    }

    #[test]
    fn nextjs_rejects_missing_css_declaration_for_global_import() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("package.json"), package_json()).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){return null;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "body { margin: 0; }",
        )
        .unwrap();

        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("declare module")
        ));

        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_rejects_script_weakening() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"echo ok","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){return null;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "export default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("scripts.build")
        ));
    }

    #[test]
    fn nextjs_rejects_tsconfig_rootdir_that_breaks_next() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"rootDir":"src"}}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("rootDir")
        ));
    }

    #[test]
    fn nextjs_rejects_deprecated_module_resolution_build_risk() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"moduleResolution":"node10"}}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("moduleResolution=node10")
        ));
    }

    #[test]
    fn nextjs_rejects_invalid_typescript_exact_version() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"5.0.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("typescript dependency")
        ));
    }

    #[test]
    fn nextjs_rejects_next_react_major_mismatch() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^15.0.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("Next 15")
        ));
    }

    #[test]
    fn repair_manifest_coherence_restores_known_good_dependency_set() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^15.0.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"5.0.0","@types/node":"^18.0.0","@types/react":"^19.0.0","@types/react-dom":"^19.0.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();

        assert!(repair_manifest_coherence(dir.path(), "3011").unwrap());
        let package: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(dependency_version(&package, "next"), Some("^14.2.0"));
        assert_eq!(dependency_version(&package, "react"), Some("^18.3.0"));
        assert_eq!(
            dependency_version(&package, "@types/react"),
            Some("^18.3.0")
        );
        assert_eq!(dependency_version(&package, "typescript"), Some("^5.5.0"));
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_allows_legacy_14_0_dependency_range_until_build_verifier_runs() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"14.0.0","react":"18.2.0","react-dom":"18.2.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_rejects_interactive_app_page_without_use_client() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#"export default function Page() {
  return <canvas ref={() => {}} onKeyDown={() => {}} />;
}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("\"use client\"")
        ));

        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            r#""use client";

export default function Page() {
  return <canvas ref={() => {}} onKeyDown={() => {}} />;
}"#,
        )
        .unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_rejects_alias_without_baseurl_or_paths() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "import Widget from '@/Widget'; export default function Page(){return <Widget/>;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tsconfig.json"),
            r#"{"compilerOptions":{"paths":{"@/*":["./src/*"]}}}"#,
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("baseUrl/paths")
        ));
    }

    #[test]
    fn nextjs_rejects_missing_tailwind_toolchain_when_tailwind_used() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("tailwind_contract_failure") && reason.contains("Tailwind")
        ));
    }

    #[test]
    fn nextjs_accepts_tailwind_cjs_config_variants() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.cjs"),
            "module.exports = { content: ['./src/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("postcss.config.cjs"),
            "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n",
        )
        .unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_rejects_esm_postcss_js_without_module_type_and_repairs_to_cjs() {
        let dir = complete_tailwind_app(
            "export default { plugins: { tailwindcss: {}, autoprefixer: {} } };\n",
        );
        let report = verify_invariant(dir.path(), "3011");
        let reason = report.primary_reason();
        assert!(
            reason.contains("PostCSS config uses ESM export default"),
            "{reason}"
        );

        assert!(repair_tailwind_contract(dir.path(), "3011", &reason).unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap(),
            canonical_postcss_config()
        );
        assert!(verify_invariant(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_allows_esm_postcss_mjs_config() {
        let dir = complete_tailwind_app(canonical_postcss_config());
        std::fs::remove_file(dir.path().join("postcss.config.js")).unwrap();
        std::fs::write(
            dir.path().join("postcss.config.mjs"),
            "export default { plugins: { tailwindcss: {}, autoprefixer: {} } };\n",
        )
        .unwrap();

        assert!(verify_invariant(dir.path(), "3011").is_pass());
    }

    #[test]
    fn nextjs_rejects_tailwind_without_autoprefixer_dependency() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.js"),
            "module.exports = { content: ['./src/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("postcss.config.js"),
            "module.exports = { plugins: { tailwindcss: {} } };\n",
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("autoprefixer")
        ));
    }

    #[test]
    fn nextjs_rejects_tailwind_postcss_config_without_plugins() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.js"),
            "module.exports = { content: ['./src/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("postcss.config.js"),
            "module.exports = { plugins: {} };\n",
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("PostCSS config must include the Tailwind plugin")
        ));
    }

    #[test]
    fn nextjs_rejects_multiple_tailwind_config_files() {
        let dir = complete_tailwind_app(
            "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n",
        );
        std::fs::write(
            dir.path().join("tailwind.config.ts"),
            "export default { content: ['./src/**/*.{ts,tsx}'] };\n",
        )
        .unwrap();

        let report = verify(dir.path(), "3011");

        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("exactly one Tailwind config")
        ));
    }

    #[test]
    fn nextjs_rejects_tailwind_directive_css_not_imported_by_layout() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.js"),
            "module.exports = { content: ['./src/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("postcss.config.js"),
            "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n",
        )
        .unwrap();
        let report = verify(dir.path(), "3011");
        assert!(matches!(
            report.status,
            VerifyStatus::ProfileContractFailed(reason) if reason.contains("@tailwind CSS file must be imported")
        ));
    }

    #[test]
    fn nextjs_allows_plain_css_without_tailwind_toolchain() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "body { margin: 0; background: #05070d; color: white; }\n",
        )
        .unwrap();
        assert!(verify(dir.path(), "3011").is_pass());
        assert!(verify_invariant(dir.path(), "3011").is_pass());
        let invariant_paths = setup_invariant_required_paths(dir.path());
        assert!(!invariant_paths.contains(&"postcss.config.js".to_string()));
        assert!(!invariant_paths.contains(&"tailwind.config.ts".to_string()));
        assert!(invariant_paths.contains(&"src/app/globals.css".to_string()));

        let report = verify_invariant(dir.path(), "3011");
        assert!(!auto_repair(dir.path(), "3011", &report).unwrap());
        assert!(!dir.path().join("postcss.config.js").exists());
        assert!(!dir.path().join("tailwind.config.ts").exists());
        assert!(!dir.path().join("tailwind.config.js").exists());
    }

    #[test]
    fn nextjs_manifest_coherence_adds_tailwind_toolchain_before_install() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return children;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.js"),
            "module.exports = { content: ['./src/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("postcss.config.js"),
            "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n",
        )
        .unwrap();

        assert!(repair_manifest_coherence(dir.path(), "3011").unwrap());
        let package: Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
        )
        .unwrap();
        for dep in ["tailwindcss", "postcss", "autoprefixer"] {
            assert!(package_has_dependency(&package, dep), "{dep}");
        }
        assert!(verify(dir.path(), "3011").is_pass());
    }

    #[test]
    fn repair_tailwind_contract_adds_missing_autoprefixer_plugin_and_is_idempotent() {
        let dir = complete_tailwind_app("module.exports = { plugins: { tailwindcss: {} } };\n");
        let report = verify_invariant(dir.path(), "3011");
        let reason = report.primary_reason();
        assert!(reason.contains("PostCSS config must include autoprefixer"));

        assert!(repair_tailwind_contract(dir.path(), "3011", &reason).unwrap());
        let postcss = std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap();
        assert!(postcss.contains("tailwindcss"));
        assert!(postcss.contains("autoprefixer"));
        assert!(verify_invariant(dir.path(), "3011").is_pass());

        let before = std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap();
        assert!(!repair_tailwind_contract(dir.path(), "3011", &reason).unwrap());
        let after = std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap();
        assert_eq!(before, after);
    }

    #[test]
    fn repair_tailwind_contract_rewrites_postcss_config_missing_plugins_key() {
        let dir =
            complete_tailwind_app("module.exports = { tailwindcss: {}, autoprefixer: {} };\n");
        let report = verify_invariant(dir.path(), "3011");
        let reason = report.primary_reason();
        assert!(reason.contains("PostCSS config must export a plugins key"));

        assert!(repair_tailwind_contract(dir.path(), "3011", &reason).unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap(),
            canonical_postcss_config()
        );
        assert!(verify_invariant(dir.path(), "3011").is_pass());
    }

    #[test]
    fn partial_tailwind_config_without_directives_still_repairs_coherence() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.ts"),
            "export default { content: ['./src/**/*.{ts,tsx}'] };\n",
        )
        .unwrap();

        let report = verify_invariant(dir.path(), "3011");
        let reason = report.primary_reason();
        assert!(reason.contains("PostCSS config file missing for Tailwind"));
        assert!(repair_tailwind_contract(dir.path(), "3011", &reason).unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap(),
            canonical_postcss_config()
        );
        assert!(verify_invariant(dir.path(), "3011").is_pass());
    }

    #[test]
    fn repair_tailwind_contract_rewrites_unrecognizable_postcss_config() {
        let dir = complete_tailwind_app("export default [require('tailwindcss')];\n");
        let report = verify_invariant(dir.path(), "3011");
        let reason = report.primary_reason();
        assert!(reason.contains("PostCSS config uses ESM export default"));

        assert!(repair_tailwind_contract(dir.path(), "3011", &reason).unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("postcss.config.js")).unwrap(),
            canonical_postcss_config()
        );
        assert!(verify_invariant(dir.path(), "3011").is_pass());
    }

    #[test]
    fn repair_tailwind_contract_adds_missing_layout_import() {
        let dir = complete_tailwind_app(
            "module.exports = { plugins: { tailwindcss: {}, autoprefixer: {} } };\n",
        );
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "export default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}",
        )
        .unwrap();
        let report = verify_invariant(dir.path(), "3011");
        let reason = report.primary_reason();
        assert!(reason.contains("@tailwind CSS file must be imported"));

        assert!(repair_tailwind_contract(dir.path(), "3011", &reason).unwrap());
        let layout = std::fs::read_to_string(dir.path().join("src/app/layout.tsx")).unwrap();
        assert!(layout.contains("import \"./globals.css\";"));
        assert!(verify_invariant(dir.path(), "3011").is_pass());
    }

    #[test]
    fn auto_repair_creates_missing_global_css_import_with_tailwind_directives() {
        let dir = complete_tailwind_app(canonical_postcss_config());
        std::fs::remove_file(dir.path().join("src/app/globals.css")).unwrap();

        let report = verify_invariant(dir.path(), "3011");
        let reason = report.primary_reason();
        assert!(reason.contains("missing relative imports"), "{reason}");
        assert!(reason.contains("src/app/globals.css"), "{reason}");

        assert!(auto_repair(dir.path(), "3011", &report).unwrap());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("src/app/globals.css")).unwrap(),
            canonical_tailwind_css()
        );
        assert!(verify_invariant(dir.path(), "3011").is_pass());
    }

    #[test]
    fn auto_repair_does_not_synthesize_missing_ts_imports() {
        let dir = complete_tailwind_app(canonical_postcss_config());
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "import Widget from './Widget';\nexport default function Page(){return <Widget />;}",
        )
        .unwrap();

        let report = verify_invariant(dir.path(), "3011");
        let reason = report.primary_reason();
        assert!(reason.contains("missing relative imports"), "{reason}");

        assert!(!auto_repair(dir.path(), "3011", &report).unwrap());
        assert!(!dir.path().join("src/app/Widget.tsx").exists());
    }

    #[test]
    fn repair_tailwind_contract_does_not_duplicate_existing_ts_config() {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.ts"),
            "export default { content: ['./src/**/*.{ts,tsx}'] };\n",
        )
        .unwrap();

        assert!(
            !repair_tailwind_contract(
                dir.path(),
                "3011",
                "tailwind_contract_failure: Tailwind config file missing"
            )
            .unwrap()
        );
        assert!(!dir.path().join("tailwind.config.js").exists());
        assert!(dir.path().join("tailwind.config.ts").exists());
    }

    fn complete_tailwind_app(postcss_config: &str) -> tempfile::TempDir {
        let dir = complete_app();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"^14.2.0","react":"^18.3.0","react-dom":"^18.3.0"},"devDependencies":{"typescript":"^5.5.0","@types/node":"^20.14.0","@types/react":"^18.3.0","@types/react-dom":"^18.3.0","tailwindcss":"^3.4.19","postcss":"^8.5.15","autoprefixer":"^10.4.20"},"scripts":{"build":"next build","dev":"next dev -p 3011"}}"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/layout.tsx"),
            "import './globals.css';\nexport default function Layout({children}:{children:React.ReactNode}){return <html><body>{children}</body></html>;}",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/global.d.ts"),
            "declare module \"*.css\";\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("src/app/globals.css"),
            "@tailwind base;\n@tailwind components;\n@tailwind utilities;\n",
        )
        .unwrap();
        std::fs::write(
            dir.path().join("tailwind.config.js"),
            "module.exports = { content: ['./src/pages/**/*.{ts,tsx}', './src/components/**/*.{ts,tsx}', './src/app/**/*.{ts,tsx}'], theme: { extend: {} }, plugins: [] };\n",
        )
        .unwrap();
        std::fs::write(dir.path().join("postcss.config.js"), postcss_config).unwrap();
        dir
    }

    fn complete_app() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src/app")).unwrap();
        std::fs::write(dir.path().join("package.json"), package_json()).unwrap();
        std::fs::write(
            dir.path().join("src/app/page.tsx"),
            "export default function Page(){return null;}",
        )
        .unwrap();
        std::fs::write(dir.path().join("src/app/layout.tsx"), "export default function Layout({children}:{children:React.ReactNode}){return children;}").unwrap();
        dir
    }
}
