use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};

use crate::eval_events;
use crate::minimal_loop::verifier_env;

const AVAILABILITY_TIMEOUT: Duration = Duration::from_secs(10);
const INTERACTION_TIMEOUT: Duration = Duration::from_secs(60);
const PROVISION_TIMEOUT: Duration = Duration::from_secs(180);
const PROBE_SCRIPT_NAME: &str = "browser-interaction-probe.cjs";
const MANAGED_INTERACTION_PROBE_REL: &[&str] = &[".anvil", "tools", "interaction-probe"];
pub const INTERACTION_PROBE_SETUP_REMEDIATION: &str = "run /setup-interaction-probe (or anvilminimal --setup-interaction-probe) to enable interaction release checks";
const PLAYWRIGHT_BROWSER_BINARIES_REMEDIATION: &str = INTERACTION_PROBE_SETUP_REMEDIATION;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeAvailability {
    Available(PlaywrightResolution),
    Unavailable(String),
}

impl ProbeAvailability {
    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            Self::Available(_) => None,
            Self::Unavailable(reason) => Some(reason),
        }
    }

    fn resolution(&self) -> Option<&PlaywrightResolution> {
        match self {
            Self::Available(resolution) => Some(resolution),
            Self::Unavailable(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlaywrightResolution {
    pub module_path: String,
    pub module_dir: String,
    pub node_path: Option<String>,
    pub location: String,
    pub version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionProbeSetupReport {
    pub tool_dir: PathBuf,
    pub resolution: PlaywrightResolution,
    pub installed: bool,
    pub log_paths: Vec<PathBuf>,
}

impl InteractionProbeSetupReport {
    pub fn summary_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        if self.installed {
            lines.push(format!(
                "interaction probe setup: installed managed Playwright in {}",
                self.tool_dir.display()
            ));
        } else {
            lines.push(format!(
                "interaction probe setup: managed Playwright already available in {}",
                self.tool_dir.display()
            ));
        }
        lines.push(format!(
            "probe ready: playwright {} ({})",
            self.resolution.version, self.resolution.location
        ));
        lines
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct InteractionProbeCandidateEvidence {
    pub rank: usize,
    pub index: usize,
    pub text_excerpt: String,
    pub changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Default)]
pub struct BrowserInteractionProbeOptions {
    pub persistence_required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserInteractionObservation {
    pub ok: bool,
    pub status: String,
    pub url: String,
    pub evidence_path: PathBuf,
    pub script_path: PathBuf,
    pub steps: Vec<String>,
    pub before_marker: String,
    pub after_marker: String,
    pub input_before_marker: String,
    pub input_after_marker: String,
    pub recovery_before_marker: String,
    pub recovery_after_marker: String,
    pub input_state_changed: bool,
    pub input_state_evaluated_after_start: bool,
    pub probe_mode: String,
    pub contract_hook_status: String,
    pub candidate_table: Vec<InteractionProbeCandidateEvidence>,
    pub input_dispatches: Vec<String>,
    pub state_dimensions_changed: Vec<String>,
    pub persistence_after_reload: String,
    pub persistence_changed_dimensions: Vec<String>,
    pub persistence_before_reload_marker: String,
    pub persistence_after_reload_marker: String,
    pub action_hooks: Vec<String>,
    pub primary_transition_observed: bool,
    pub start_control_found: bool,
    pub informational_failure_kinds: Vec<String>,
    pub recovery_transition_observed: bool,
    pub recovery_transition_not_observed: bool,
    pub failure_kind: String,
    pub stage: String,
    pub error: String,
    pub remediation: String,
    pub duration_ms: u128,
    pub output_excerpt: String,
    pub stdout_excerpt: String,
    pub stderr_excerpt: String,
    pub raw_stdout_excerpt: String,
    pub child_spawned: bool,
    pub child_reaped: bool,
    pub playwright_resolution: Option<PlaywrightResolution>,
    pub server_http_status: Option<i64>,
    pub server_http_error: String,
    pub navigation_failure_kind: String,
    pub has_canvas: Option<bool>,
    pub interactive_control_count: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaticHtmlProbeSelection {
    pub probe_mode: String,
    pub contract_hook_status: String,
    pub primary_present: bool,
    pub primary_text_excerpt: String,
    pub restart_present: bool,
    pub restart_text_excerpt: String,
    pub state_count: usize,
    pub valid_state_count: usize,
    pub invalid_state_count: usize,
    pub candidate_table: Vec<StaticHtmlProbeCandidate>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct StaticHtmlProbeCandidate {
    pub rank: usize,
    pub index: usize,
    pub text_excerpt: String,
    pub text_bucket: usize,
    pub area: i64,
    pub centrality_milli: i64,
    pub contract_primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionProbeOutcome {
    Unavailable(String),
    Observation(Box<BrowserInteractionObservation>),
}

impl InteractionProbeOutcome {
    pub fn observation(&self) -> Option<&BrowserInteractionObservation> {
        match self {
            Self::Observation(observation) => Some(observation),
            Self::Unavailable(_) => None,
        }
    }
}

pub fn playwright_availability(root: &Path) -> ProbeAvailability {
    #[cfg(test)]
    if let Some(availability) = load_test_availability_override(root) {
        return availability;
    }
    #[cfg(test)]
    {
        let _ = root;
        ProbeAvailability::Unavailable("playwright_not_installed".to_string())
    }
    #[cfg(not(test))]
    {
        playwright_availability_from_command(root)
    }
}

#[cfg(not(test))]
fn playwright_availability_from_command(root: &Path) -> ProbeAvailability {
    playwright_availability_from_programs_with_home(
        root,
        OsStr::new("node"),
        OsStr::new("npm"),
        std::env::var_os("ANVIL_PLAYWRIGHT_DIR").map(PathBuf::from),
        Some(&home_dir()),
    )
}

#[cfg(test)]
fn playwright_availability_from_programs(
    root: &Path,
    node_program: &OsStr,
    npm_program: &OsStr,
    configured_tool_dir: Option<PathBuf>,
) -> ProbeAvailability {
    playwright_availability_from_programs_with_home(
        root,
        node_program,
        npm_program,
        configured_tool_dir,
        Some(&home_dir()),
    )
}

fn playwright_availability_from_programs_with_home(
    root: &Path,
    node_program: &OsStr,
    npm_program: &OsStr,
    configured_tool_dir: Option<PathBuf>,
    home: Option<&Path>,
) -> ProbeAvailability {
    if let Some(resolution) = resolve_playwright_module(root, node_program, None, "workspace_root")
    {
        return ProbeAvailability::Available(resolution);
    }
    if let Some(home) = home
        && let Some(resolution) = resolve_playwright_module(
            root,
            node_program,
            Some(&managed_interaction_probe_node_modules_dir(home)),
            "managed_interaction_probe",
        )
    {
        return ProbeAvailability::Available(resolution);
    }
    if let Some(global_root) = npm_global_root(root, npm_program)
        && let Some(resolution) =
            resolve_playwright_module(root, node_program, Some(&global_root), "npm_global")
    {
        return ProbeAvailability::Available(resolution);
    }
    if let Some(tool_dir) = configured_tool_dir
        && let Some(resolution) =
            resolve_playwright_module(root, node_program, Some(&tool_dir), "ANVIL_PLAYWRIGHT_DIR")
    {
        return ProbeAvailability::Available(resolution);
    }
    ProbeAvailability::Unavailable("playwright_not_installed".to_string())
}

pub fn setup_interaction_probe() -> anyhow::Result<InteractionProbeSetupReport> {
    setup_interaction_probe_with_progress(|_| {})
}

pub fn setup_interaction_probe_with_stdout_progress() -> anyhow::Result<InteractionProbeSetupReport>
{
    setup_interaction_probe_with_progress(|line| println!("{line}"))
}

fn setup_interaction_probe_with_progress(
    progress: impl FnMut(&str),
) -> anyhow::Result<InteractionProbeSetupReport> {
    setup_interaction_probe_with_programs(
        Path::new("."),
        OsStr::new("node"),
        OsStr::new("npm"),
        OsStr::new("npx"),
        &home_dir(),
        PROVISION_TIMEOUT,
        progress,
    )
}

fn setup_interaction_probe_with_programs(
    root: &Path,
    node_program: &OsStr,
    npm_program: &OsStr,
    npx_program: &OsStr,
    home: &Path,
    timeout: Duration,
    mut progress: impl FnMut(&str),
) -> anyhow::Result<InteractionProbeSetupReport> {
    let tool_dir = managed_interaction_probe_tool_dir(home);
    std::fs::create_dir_all(&tool_dir)
        .with_context(|| format!("failed to create {}", tool_dir.display()))?;
    progress(&format!(
        "interaction probe setup: using {}",
        tool_dir.display()
    ));

    if let Some(resolution) =
        resolve_managed_playwright_module(&tool_dir, node_program, "managed_interaction_probe")
    {
        progress(&format!(
            "interaction probe setup: existing playwright {} resolved from {}",
            resolution.version, resolution.location
        ));
        return Ok(InteractionProbeSetupReport {
            tool_dir,
            resolution,
            installed: false,
            log_paths: Vec::new(),
        });
    }

    ensure_managed_package_json(&tool_dir)?;
    let log_dir = tool_dir.join("logs");
    std::fs::create_dir_all(&log_dir)
        .with_context(|| format!("failed to create {}", log_dir.display()))?;

    let npm_log = log_dir.join("npm-install-playwright.log");
    progress("interaction probe setup: npm install playwright");
    run_setup_command(
        root,
        &tool_dir,
        npm_program,
        &["install", "playwright"],
        &npm_log,
        timeout,
    )?;

    let npx_log = log_dir.join("npx-playwright-install-chromium.log");
    progress("interaction probe setup: npx playwright install chromium");
    run_setup_command(
        root,
        &tool_dir,
        npx_program,
        &["playwright", "install", "chromium"],
        &npx_log,
        timeout,
    )?;

    let resolution =
        resolve_managed_playwright_module(&tool_dir, node_program, "managed_interaction_probe")
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "managed interaction probe setup completed but playwright could not be resolved from {}",
                    managed_interaction_probe_node_modules_dir(home).display()
                )
            })?;
    Ok(InteractionProbeSetupReport {
        tool_dir,
        resolution,
        installed: true,
        log_paths: vec![
            npm_log.with_extension("out"),
            npm_log.with_extension("err"),
            npx_log.with_extension("out"),
            npx_log.with_extension("err"),
        ],
    })
}

fn node_program_for_root(root: &Path) -> OsString {
    #[cfg(test)]
    if let Some(path) = load_test_node_program_override(root) {
        return path.into_os_string();
    }
    let _ = root;
    OsString::from("node")
}

fn resolve_playwright_module(
    root: &Path,
    node_program: &OsStr,
    node_path: Option<&Path>,
    location: &str,
) -> Option<PlaywrightResolution> {
    let output = run_command_stdout(
        verifier_env::normalized_command_at_root(node_program, root)
            .arg("-e")
            .arg(
                "const path=require.resolve('playwright');\
const version=require('playwright/package.json').version||'unknown';\
console.log(JSON.stringify({path,version}));",
            )
            .current_dir(root)
            .stdin(Stdio::null())
            .envs(node_path_env(node_path)),
    )?;
    let line = output.lines().find(|line| !line.trim().is_empty())?.trim();
    let (module_path, version) = match serde_json::from_str::<Value>(line) {
        Ok(value) => (
            value.get("path")?.as_str()?.to_string(),
            value
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string(),
        ),
        Err(_) => (line.to_string(), "unknown".to_string()),
    };
    let module_dir = Path::new(&module_path)
        .parent()
        .map(|path| path.display().to_string())
        .unwrap_or_default();
    Some(PlaywrightResolution {
        module_path,
        module_dir,
        node_path: node_path.map(|path| path.display().to_string()),
        location: location.to_string(),
        version,
    })
}

fn resolve_managed_playwright_module(
    tool_dir: &Path,
    node_program: &OsStr,
    location: &str,
) -> Option<PlaywrightResolution> {
    resolve_playwright_module(
        tool_dir,
        node_program,
        Some(&tool_dir.join("node_modules")),
        location,
    )
}

fn managed_interaction_probe_tool_dir(home: &Path) -> PathBuf {
    MANAGED_INTERACTION_PROBE_REL
        .iter()
        .fold(home.to_path_buf(), |path, part| path.join(part))
}

fn managed_interaction_probe_node_modules_dir(home: &Path) -> PathBuf {
    managed_interaction_probe_tool_dir(home).join("node_modules")
}

fn home_dir() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
}

fn ensure_managed_package_json(tool_dir: &Path) -> anyhow::Result<()> {
    let path = tool_dir.join("package.json");
    if path.is_file() {
        return Ok(());
    }
    std::fs::write(
        &path,
        "{\n  \"private\": true,\n  \"description\": \"Anvil managed interaction probe tools\"\n}\n",
    )
    .with_context(|| format!("failed to write {}", path.display()))
}

fn run_setup_command(
    root: &Path,
    cwd: &Path,
    program: &OsStr,
    args: &[&str],
    log_path: &Path,
    timeout: Duration,
) -> anyhow::Result<()> {
    let stdout_path = log_path.with_extension("out");
    let stderr_path = log_path.with_extension("err");
    let stdout = std::fs::File::create(&stdout_path)
        .with_context(|| format!("failed to create {}", stdout_path.display()))?;
    let stderr = std::fs::File::create(&stderr_path)
        .with_context(|| format!("failed to create {}", stderr_path.display()))?;
    let mut command = verifier_env::normalized_command_at_root(program, root);
    command
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let started = Instant::now();
    let mut child = command
        .spawn()
        .with_context(|| format!("failed to spawn {}", program.to_string_lossy()))?;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = child.wait();
                if status.success() {
                    return Ok(());
                }
                bail!(
                    "command failed: {} {}\n{}",
                    program.to_string_lossy(),
                    args.join(" "),
                    setup_command_excerpt(&stdout_path, &stderr_path)
                );
            }
            Ok(None) => {}
            Err(err) => {
                terminate_child_group(&mut child);
                let _ = child.wait();
                return Err(err).context("failed to wait for interaction probe setup command");
            }
        }
        if started.elapsed() >= timeout {
            terminate_child_group(&mut child);
            let _ = child.wait();
            bail!(
                "command timed out after {} ms: {} {}\n{}",
                timeout.as_millis(),
                program.to_string_lossy(),
                args.join(" "),
                setup_command_excerpt(&stdout_path, &stderr_path)
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn setup_command_excerpt(stdout_path: &Path, stderr_path: &Path) -> String {
    let stdout = std::fs::read_to_string(stdout_path).unwrap_or_default();
    let stderr = std::fs::read_to_string(stderr_path).unwrap_or_default();
    eval_events::body_snippet(format!("{stdout}\n{stderr}").trim())
}

fn npm_global_root(root: &Path, npm_program: &OsStr) -> Option<PathBuf> {
    let output = run_command_stdout(
        verifier_env::normalized_command_at_root(npm_program, root)
            .args(["root", "-g"])
            .current_dir(root)
            .stdin(Stdio::null()),
    )?;
    output
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(|line| PathBuf::from(line.trim()))
}

fn node_path_env(node_path: Option<&Path>) -> Vec<(&'static str, OsString)> {
    node_path
        .map(|path| vec![("NODE_PATH", path.as_os_str().to_os_string())])
        .unwrap_or_default()
}

fn run_command_stdout(command: &mut std::process::Command) -> Option<String> {
    command.stdout(Stdio::piped()).stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return None,
    };
    let deadline = Instant::now() + AVAILABILITY_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child.wait_with_output().ok()?;
                return if status.success() {
                    Some(String::from_utf8_lossy(&output.stdout).to_string())
                } else {
                    None
                };
            }
            Ok(None) => {}
            Err(_) => {
                terminate_child_group(&mut child);
                let _ = child.wait();
                return None;
            }
        }
        if Instant::now() >= deadline {
            terminate_child_group(&mut child);
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn browser_interaction_evidence_path(root: &Path) -> PathBuf {
    root.join(".anvil")
        .join("evidence")
        .join("browser-interaction.json")
}

pub fn probe_browser_interaction_against_running_server(
    root: &Path,
    port: u16,
    run_dir: &Path,
    evidence_path: &Path,
    timeout: Duration,
) -> InteractionProbeOutcome {
    probe_browser_interaction_against_running_server_with_options(
        root,
        port,
        run_dir,
        evidence_path,
        timeout,
        BrowserInteractionProbeOptions::default(),
    )
}

pub fn probe_browser_interaction_against_running_server_with_options(
    root: &Path,
    port: u16,
    run_dir: &Path,
    evidence_path: &Path,
    timeout: Duration,
    options: BrowserInteractionProbeOptions,
) -> InteractionProbeOutcome {
    let availability = playwright_availability(root);
    let Some(resolution) = availability.resolution().cloned() else {
        let ProbeAvailability::Unavailable(reason) = availability else {
            unreachable!("availability without resolution must be unavailable");
        };
        return InteractionProbeOutcome::Unavailable(reason);
    };
    #[cfg(test)]
    if let Some(value) = load_test_result_override(root) {
        let observation = observation_from_value(
            evidence_path,
            &run_dir.join(PROBE_SCRIPT_NAME),
            &format!("http://127.0.0.1:{port}/"),
            value,
            Instant::now(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            false,
            true,
            Some(resolution.clone()),
        );
        write_interaction_value(
            root,
            evidence_path,
            &interaction_observation_json(&observation),
        );
        return InteractionProbeOutcome::Observation(Box::new(observation));
    }

    let started = Instant::now();
    let timeout = if timeout.is_zero() {
        INTERACTION_TIMEOUT
    } else {
        timeout.min(INTERACTION_TIMEOUT)
    };
    let script_path = run_dir.join(PROBE_SCRIPT_NAME);
    if let Err(err) = write_probe_script(&script_path) {
        let observation = failure_observation(
            root,
            evidence_path,
            &script_path,
            port,
            started,
            "probe_script_write_failed",
            &err.to_string(),
            "",
            "",
            "",
            "",
            false,
            true,
            Some(resolution.clone()),
        );
        return InteractionProbeOutcome::Observation(Box::new(observation));
    }

    let (stdout_log, stderr_log) = match open_stdio_logs(run_dir) {
        Ok(logs) => logs,
        Err(err) => {
            let observation = failure_observation(
                root,
                evidence_path,
                &script_path,
                port,
                started,
                "probe_stdio_open_failed",
                &err.to_string(),
                "",
                "",
                "",
                "",
                false,
                true,
                Some(resolution.clone()),
            );
            return InteractionProbeOutcome::Observation(Box::new(observation));
        }
    };

    let url = format!("http://127.0.0.1:{port}/");
    let node_program = node_program_for_root(root);
    let mut command = verifier_env::normalized_command_at_root(&node_program, root);
    command
        .arg(&script_path)
        .arg(&url)
        .arg(evidence_path)
        .arg(
            serde_json::to_string(&options)
                .unwrap_or_else(|_| "{\"persistence_required\":false}".to_string()),
        )
        .current_dir(root)
        .envs(node_path_env(
            resolution.node_path.as_deref().map(Path::new),
        ))
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_log))
        .stderr(Stdio::from(stderr_log));
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            let observation = failure_observation(
                root,
                evidence_path,
                &script_path,
                port,
                started,
                "probe_spawn_failed",
                &err.to_string(),
                "",
                "",
                "",
                "",
                false,
                true,
                Some(resolution.clone()),
            );
            return InteractionProbeOutcome::Observation(Box::new(observation));
        }
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let reaped = child.wait().is_ok();
                let logs = interaction_stdio_logs(run_dir);
                if status.success() {
                    let value = read_interaction_value(evidence_path).unwrap_or_else(|| {
                        interaction_failure_json(
                            &url,
                            "probe_evidence_missing",
                            &logs.output_excerpt,
                            started.elapsed().as_millis(),
                        )
                    });
                    let observation = observation_from_value(
                        evidence_path,
                        &script_path,
                        &url,
                        value,
                        started,
                        logs.output_excerpt,
                        logs.stdout_excerpt,
                        logs.stderr_excerpt,
                        String::new(),
                        true,
                        reaped,
                        Some(resolution.clone()),
                    );
                    mirror_interaction_observation(root, evidence_path, &observation);
                    return InteractionProbeOutcome::Observation(Box::new(observation));
                }
                if let Some(value) = read_interaction_value(evidence_path) {
                    let value = merge_script_stdout_failure_value(value, &logs);
                    let observation = observation_from_value(
                        evidence_path,
                        &script_path,
                        &url,
                        value,
                        started,
                        logs.output_excerpt,
                        logs.stdout_excerpt,
                        logs.stderr_excerpt,
                        String::new(),
                        true,
                        reaped,
                        Some(resolution.clone()),
                    );
                    mirror_interaction_observation(root, evidence_path, &observation);
                    return InteractionProbeOutcome::Observation(Box::new(observation));
                }
                let observation = failure_observation(
                    root,
                    evidence_path,
                    &script_path,
                    port,
                    started,
                    "probe_command_failed",
                    &logs.output_excerpt,
                    &logs.stdout_raw,
                    &logs.stdout_excerpt,
                    &logs.stderr_excerpt,
                    &logs.raw_stdout_excerpt,
                    true,
                    reaped,
                    Some(resolution.clone()),
                );
                return InteractionProbeOutcome::Observation(Box::new(observation));
            }
            Ok(None) => {}
            Err(err) => {
                terminate_child_group(&mut child);
                let reaped = child.wait().is_ok();
                let observation = failure_observation(
                    root,
                    evidence_path,
                    &script_path,
                    port,
                    started,
                    "probe_status_unreadable",
                    &err.to_string(),
                    "",
                    "",
                    "",
                    "",
                    true,
                    reaped,
                    Some(resolution.clone()),
                );
                return InteractionProbeOutcome::Observation(Box::new(observation));
            }
        }
        if Instant::now() >= deadline {
            terminate_child_group(&mut child);
            let reaped = child.wait().is_ok();
            let logs = interaction_stdio_logs(run_dir);
            let observation = failure_observation(
                root,
                evidence_path,
                &script_path,
                port,
                started,
                "probe_timeout",
                &logs.output_excerpt,
                &logs.stdout_raw,
                &logs.stdout_excerpt,
                &logs.stderr_excerpt,
                &logs.raw_stdout_excerpt,
                true,
                reaped,
                Some(resolution.clone()),
            );
            return InteractionProbeOutcome::Observation(Box::new(observation));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

pub fn static_html_probe_selection(html: &str) -> StaticHtmlProbeSelection {
    let elements = static_probe_elements(html);
    let primary = elements
        .iter()
        .find(|element| element.attr("data-anvil-action") == Some("primary"));
    let restart = elements
        .iter()
        .find(|element| element.attr("data-anvil-action") == Some("restart"));
    let state_values = static_data_anvil_state_values(html);
    let valid_state_count = state_values
        .iter()
        .filter(|value| serde_json::from_str::<Value>(value).is_ok())
        .count();
    let invalid_state_count = state_values.len().saturating_sub(valid_state_count);
    let primary_present = primary.is_some();
    let state_present = !state_values.is_empty();
    let usable = primary_present && valid_state_count > 0;
    let contract_hook_status = if usable {
        "usable"
    } else if !primary_present {
        "primary_missing"
    } else if !state_present {
        "state_missing"
    } else {
        "state_invalid"
    }
    .to_string();
    let probe_mode = if usable { "contract" } else { "heuristic" }.to_string();
    let mut candidates = elements
        .iter()
        .enumerate()
        .filter(|(_, element)| element.is_control() && element.visible())
        .map(|(index, element)| {
            let text = element.text_excerpt();
            StaticHtmlProbeCandidate {
                rank: 0,
                index,
                text_bucket: usize::from(text.len() >= 2),
                text_excerpt: text,
                area: element.area(),
                centrality_milli: element.centrality_milli(),
                contract_primary: element.attr("data-anvil-action") == Some("primary"),
            }
        })
        .collect::<Vec<_>>();
    candidates.sort_by(|a, b| {
        b.text_bucket
            .cmp(&a.text_bucket)
            .then_with(|| b.area.cmp(&a.area))
            .then_with(|| b.centrality_milli.cmp(&a.centrality_milli))
            .then_with(|| a.index.cmp(&b.index))
    });
    candidates.truncate(4);
    for (rank, candidate) in candidates.iter_mut().enumerate() {
        candidate.rank = rank + 1;
    }
    StaticHtmlProbeSelection {
        probe_mode,
        contract_hook_status,
        primary_present,
        primary_text_excerpt: primary
            .map(StaticProbeElement::text_excerpt)
            .unwrap_or_default(),
        restart_present: restart.is_some(),
        restart_text_excerpt: restart
            .map(StaticProbeElement::text_excerpt)
            .unwrap_or_default(),
        state_count: state_values.len(),
        valid_state_count,
        invalid_state_count,
        candidate_table: candidates,
    }
}

#[derive(Debug, Clone)]
struct StaticProbeElement {
    tag: String,
    attrs: Vec<(String, String)>,
    inner: String,
}

impl StaticProbeElement {
    fn attr(&self, name: &str) -> Option<&str> {
        self.attrs
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }

    fn is_control(&self) -> bool {
        self.tag.eq_ignore_ascii_case("button")
            || self.attr("role").is_some_and(|role| role == "button")
    }

    fn visible(&self) -> bool {
        if self.attr("hidden").is_some() {
            return false;
        }
        let style = self.attr("style").unwrap_or("").to_ascii_lowercase();
        !style.contains("display:none")
            && !style.contains("display: none")
            && !style.contains("visibility:hidden")
            && !style.contains("visibility: hidden")
            && !style.contains("opacity:0")
            && !style.contains("opacity: 0")
    }

    fn text_excerpt(&self) -> String {
        let text = self
            .attr("aria-label")
            .or_else(|| self.attr("title"))
            .filter(|value| !value.trim().is_empty())
            .map(str::to_string)
            .unwrap_or_else(|| strip_html_tags_static(&self.inner));
        collapse_whitespace_static(&decode_common_html_entities(&text))
            .chars()
            .take(80)
            .collect()
    }

    fn area(&self) -> i64 {
        self.attr("data-anvil-probe-area")
            .and_then(|value| value.parse::<i64>().ok())
            .or_else(|| style_dimension_product(self.attr("style").unwrap_or("")))
            .unwrap_or_else(|| {
                let width = self
                    .attr("width")
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(0);
                let height = self
                    .attr("height")
                    .and_then(|value| value.parse::<i64>().ok())
                    .unwrap_or(0);
                if width > 0 && height > 0 {
                    width * height
                } else {
                    let text_len = self.text_excerpt().len() as i64;
                    (text_len.max(1) * 120).max(1)
                }
            })
    }

    fn centrality_milli(&self) -> i64 {
        self.attr("data-anvil-probe-centrality")
            .and_then(|value| value.parse::<i64>().ok())
            .unwrap_or(500)
    }
}

fn static_probe_elements(html: &str) -> Vec<StaticProbeElement> {
    let mut elements = Vec::new();
    collect_static_elements(
        html,
        Regex::new(r#"(?is)<button\b([^>]*)>(.*?)</\s*button\s*>"#)
            .expect("valid static button regex"),
        "button",
        &mut elements,
    );
    collect_static_elements(
        html,
        Regex::new(
            r#"(?is)<([a-z][a-z0-9-]*)\b([^>]*(?:\brole\s*=\s*["']?button["']?|\bdata-anvil-action\s*=)[^>]*)>(.*?)</\s*[a-z][a-z0-9-]*\s*>"#,
        )
        .expect("valid static role/action regex"),
        "",
        &mut elements,
    );
    elements.sort_by_key(|(offset, _)| *offset);
    let mut out = Vec::new();
    for (_, element) in elements {
        if !out.iter().any(|existing: &StaticProbeElement| {
            existing.tag == element.tag
                && existing.inner == element.inner
                && existing.attrs == element.attrs
        }) {
            out.push(element);
        }
    }
    out
}

fn collect_static_elements(
    html: &str,
    re: Regex,
    fixed_tag: &str,
    out: &mut Vec<(usize, StaticProbeElement)>,
) {
    for captures in re.captures_iter(html) {
        let Some(whole) = captures.get(0) else {
            continue;
        };
        let (tag, attrs, inner) = if fixed_tag.is_empty() {
            (
                captures.get(1).map_or("", |m| m.as_str()),
                captures.get(2).map_or("", |m| m.as_str()),
                captures.get(3).map_or("", |m| m.as_str()),
            )
        } else {
            (
                fixed_tag,
                captures.get(1).map_or("", |m| m.as_str()),
                captures.get(2).map_or("", |m| m.as_str()),
            )
        };
        out.push((
            whole.start(),
            StaticProbeElement {
                tag: tag.to_ascii_lowercase(),
                attrs: parse_static_attrs(attrs),
                inner: inner.to_string(),
            },
        ));
    }
}

fn static_data_anvil_state_values(html: &str) -> Vec<String> {
    let tag_re = Regex::new(r#"(?is)<[a-z][a-z0-9-]*\b([^>]*\bdata-anvil-state\s*=)[^>]*>"#)
        .expect("valid static state tag regex");
    tag_re
        .captures_iter(html)
        .filter_map(|captures| {
            let attrs = captures.get(0).map_or("", |m| m.as_str());
            parse_static_attrs(attrs)
                .into_iter()
                .find(|(key, _)| key == "data-anvil-state")
                .map(|(_, value)| decode_common_html_entities(&value))
        })
        .collect()
}

fn parse_static_attrs(raw: &str) -> Vec<(String, String)> {
    let attr_re =
        Regex::new(r#"(?is)([a-z_:][-a-z0-9_:.]*)\s*(?:=\s*("([^"]*)"|'([^']*)'|([^\s"'>]+)))?"#)
            .expect("valid html attribute regex");
    attr_re
        .captures_iter(raw)
        .filter_map(|captures| {
            let key = captures.get(1)?.as_str().to_ascii_lowercase();
            let value = captures
                .get(3)
                .or_else(|| captures.get(4))
                .or_else(|| captures.get(5))
                .map_or("", |m| m.as_str())
                .to_string();
            Some((key, value))
        })
        .collect()
}

fn style_dimension_product(style: &str) -> Option<i64> {
    let width = style_dimension(style, "width")?;
    let height = style_dimension(style, "height")?;
    Some(width * height)
}

fn style_dimension(style: &str, name: &str) -> Option<i64> {
    let pattern = format!(r#"(?i)\b{}\s*:\s*([0-9]+)px\b"#, regex::escape(name));
    Regex::new(&pattern)
        .ok()?
        .captures(style)?
        .get(1)?
        .as_str()
        .parse::<i64>()
        .ok()
}

fn strip_html_tags_static(text: &str) -> String {
    let tag_re = Regex::new(r#"(?is)<[^>]+>"#).expect("valid html tag regex");
    tag_re.replace_all(text, " ").to_string()
}

fn collapse_whitespace_static(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_common_html_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
}

fn write_probe_script(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, interaction_probe_script())
}

fn interaction_probe_script() -> &'static str {
    r#"const fs = require("fs");
const http = require("http");

const url = process.argv[2];
const outputPath = process.argv[3];
const probeOptions = (() => {
  try {
    return JSON.parse(process.argv[4] || "{}");
  } catch (_) {
    return {};
  }
})();
const persistenceRequired = !!probeOptions.persistence_required;
const started = Date.now();
const LAUNCH_TIMEOUT_MS = 20000;
const GOTO_TIMEOUT_MS = 12000;
const SERVER_CHECK_TIMEOUT_MS = 5000;
const steps = [];
let stage = "resolving";
let before_marker = "";
let after_marker = "";
let input_before_marker = "";
let input_after_marker = "";
let recovery_before_marker = "";
let recovery_after_marker = "";
let recovery_transition_status = "unknown";
let probe_mode = "heuristic";
let contract_hook_status = "unknown";
let contract_hooks = null;
let primary_transition_observed = false;
let start_control_found = true;
let input_state_evaluated_after_start = false;
let candidate_table = [];
let input_dispatches = [];
let state_dimensions_changed = [];
let persistence_after_reload = "not_evaluated";
let persistence_changed_dimensions = [];
let persistence_before_reload_marker = "";
let persistence_after_reload_marker = "";
let action_hooks = [];
let informational_failure_kinds = [];
let server_check = { ok: false, status: null, error: "" };
let post_js_surface = null;

function write(value) {
  fs.mkdirSync(require("path").dirname(outputPath), { recursive: true });
  fs.writeFileSync(outputPath, JSON.stringify(value, null, 2) + "\n");
}

function writeFailure(value) {
  write(value);
  try {
    process.stdout.write(JSON.stringify(value) + "\n");
  } catch (_) {}
}

function mark(nextStage, extra = {}) {
  stage = nextStage;
  try {
    process.stderr.write(JSON.stringify({ stage, ...extra }) + "\n");
  } catch (_) {}
}

function rawHttpGet(targetUrl) {
  return new Promise((resolve) => {
    let parsed;
    try {
      parsed = new URL(targetUrl);
    } catch (err) {
      resolve({ ok: false, status: null, error: err && err.message ? err.message : String(err) });
      return;
    }
    const request = http.get({
      protocol: parsed.protocol,
      hostname: parsed.hostname,
      port: parsed.port,
      path: `${parsed.pathname || "/"}${parsed.search || ""}`,
      timeout: SERVER_CHECK_TIMEOUT_MS,
      headers: {
        "Connection": "close",
        "User-Agent": "anvilminimal-interaction-probe"
      }
    }, (response) => {
      response.resume();
      response.on("end", () => {
        resolve({ ok: true, status: response.statusCode || 0, error: "" });
      });
    });
    request.on("timeout", () => {
      request.destroy(new Error("server_check_timeout"));
    });
    request.on("error", (err) => {
      const code = err && err.code ? `${err.code}: ` : "";
      resolve({ ok: false, status: null, error: `${code}${err && err.message ? err.message : String(err)}` });
    });
  });
}

async function marker(page) {
  return await page.evaluate(() => {
    const textOf = (el) => (
      el.getAttribute("aria-label") ||
      el.getAttribute("title") ||
      el.textContent ||
      el.value ||
      ""
    ).trim();
    const buttons = Array.from(document.querySelectorAll("button,[role=button]"))
      .map((el) => textOf(el))
      .join("|");
    const body = (document.body && document.body.innerText ? document.body.innerText : "")
      .replace(/\s+/g, " ")
      .slice(0, 800);
    const element_count = document.querySelectorAll("*").length;
    const canvases = Array.from(document.querySelectorAll("canvas"))
      .slice(0, 3)
      .map((canvas) => {
        try {
          return canvas.toDataURL("image/png").slice(0, 2048);
        } catch (_) {
          return `${canvas.width}x${canvas.height}:unreadable`;
        }
      });
    return JSON.stringify({ buttons, body, element_count, canvases });
  });
}

async function surfaceSnapshot(page) {
  return await page.evaluate(() => {
    const controls = document.querySelectorAll("button,[role=button],input,select,textarea,a[href]");
    const canvases = document.querySelectorAll("canvas");
    const title = document.title || "";
    return {
      has_canvas: canvases.length > 0,
      canvas_count: canvases.length,
      interactive_control_count: controls.length,
      title_text_excerpt: title.slice(0, 120)
    };
  });
}

function navigationFailureDetail(err) {
  const message = err && err.message ? err.message : String(err);
  const net = message.match(/net::([A-Z0-9_]+)/);
  if (net) return net[1];
  if (/timeout/i.test(message)) return "timeout";
  if (/page crashed/i.test(message)) return "page_crash";
  if (/target closed/i.test(message)) return "target_closed";
  return "navigation_error";
}

async function gotoWithRetry(page, targetUrl) {
  let lastErr;
  for (let attempt = 1; attempt <= 2; attempt += 1) {
    try {
      return await page.goto(targetUrl, { waitUntil: "domcontentloaded", timeout: GOTO_TIMEOUT_MS });
    } catch (err) {
      lastErr = err;
      if (attempt === 1) {
        mark("goto_retry", { attempt, error: err && err.message ? err.message : String(err) });
        await page.waitForTimeout(1000);
      }
    }
  }
  const detail = navigationFailureDetail(lastErr);
  const err = new Error(lastErr && lastErr.message ? lastErr.message : String(lastErr));
  err.anvilFailureKind = server_check.ok ? "app_route_unresponsive" : "probe_infrastructure_failed:server_unreachable";
  err.navigationFailureKind = `probe_navigation_failed:${detail}`;
  throw err;
}

async function markerAfterChange(page, previous, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let current = await marker(page);
  while (Date.now() < deadline) {
    if (current !== previous) {
      return current;
    }
    await page.waitForTimeout(80);
    current = await marker(page);
  }
  return current;
}

async function contractStateMarker(page) {
  return await page.evaluate(() => {
    const stable = (value) => {
      if (Array.isArray(value)) return value.map(stable);
      if (value && typeof value === "object") {
        return Object.keys(value).sort().reduce((out, key) => {
          out[key] = stable(value[key]);
          return out;
        }, {});
      }
      return value;
    };
    const states = Array.from(document.querySelectorAll("[data-anvil-state]"))
      .map((el, index) => {
        const raw = el.getAttribute("data-anvil-state") || "";
        try {
          return { index, state: stable(JSON.parse(raw)) };
        } catch (_) {
          return null;
        }
      })
      .filter(Boolean);
    return JSON.stringify({ states });
  });
}

function stableStateString(value) {
  if (typeof value === "undefined") return "__anvil_undefined__";
  try {
    return JSON.stringify(value);
  } catch (_) {
    return String(value);
  }
}

function contractStatesFromMarker(markerText) {
  try {
    const parsed = JSON.parse(markerText || "{}");
    if (!parsed || !Array.isArray(parsed.states)) return [];
    return parsed.states
      .map((entry) => entry && entry.state && typeof entry.state === "object" && !Array.isArray(entry.state)
        ? entry.state
        : {})
      .filter(Boolean);
  } catch (_) {
    return [];
  }
}

function changedTopLevelStateKeys(beforeText, afterText) {
  const before = contractStatesFromMarker(beforeText);
  const after = contractStatesFromMarker(afterText);
  const keys = new Set();
  const count = Math.max(before.length, after.length);
  for (let index = 0; index < count; index += 1) {
    const beforeState = before[index] || {};
    const afterState = after[index] || {};
    const names = new Set([...Object.keys(beforeState), ...Object.keys(afterState)]);
    for (const name of names) {
      if (stableStateString(beforeState[name]) !== stableStateString(afterState[name])) {
        keys.add(name);
      }
    }
  }
  return Array.from(keys).sort();
}

function mergeStateDimensionsChanged(keys) {
  for (const key of keys || []) {
    if (key && !state_dimensions_changed.includes(key)) {
      state_dimensions_changed.push(key);
    }
  }
  state_dimensions_changed.sort();
}

function retainedChangedStateKeys(inputBeforeText, inputAfterText, beforeReloadText) {
  const changed = changedTopLevelStateKeys(inputBeforeText, inputAfterText);
  if (changed.length === 0) return [];
  const inputBefore = contractStatesFromMarker(inputBeforeText);
  const inputAfter = contractStatesFromMarker(inputAfterText);
  const beforeReload = contractStatesFromMarker(beforeReloadText);
  const retained = new Set();
  const count = Math.max(inputBefore.length, inputAfter.length, beforeReload.length);
  for (let index = 0; index < count; index += 1) {
    const beforeState = inputBefore[index] || {};
    const afterState = inputAfter[index] || {};
    const reloadState = beforeReload[index] || {};
    for (const key of changed) {
      if (
        stableStateString(beforeState[key]) !== stableStateString(afterState[key]) &&
        stableStateString(afterState[key]) === stableStateString(reloadState[key])
      ) {
        retained.add(key);
      }
    }
  }
  return Array.from(retained).sort();
}

function changedStateKeysPreservedAfterReload(keys, beforeReloadText, afterReloadText) {
  const beforeReload = contractStatesFromMarker(beforeReloadText);
  const afterReload = contractStatesFromMarker(afterReloadText);
  const count = Math.max(beforeReload.length, afterReload.length);
  for (const key of keys) {
    let sawKey = false;
    for (let index = 0; index < count; index += 1) {
      const beforeState = beforeReload[index] || {};
      const afterState = afterReload[index] || {};
      if (Object.prototype.hasOwnProperty.call(beforeState, key)) {
        sawKey = true;
      }
      if (stableStateString(beforeState[key]) !== stableStateString(afterState[key])) {
        return false;
      }
    }
    if (!sawKey) return false;
  }
  return keys.length > 0;
}

function evaluatePersistenceAfterReload(mode, beforeReloadText, afterReloadText) {
  if (mode === "contract") {
    const retained = retainedChangedStateKeys(input_before_marker, input_after_marker, beforeReloadText);
    persistence_changed_dimensions = retained;
    if (retained.length === 0) return "not_evaluated";
    return changedStateKeysPreservedAfterReload(retained, beforeReloadText, afterReloadText)
      ? "preserved"
      : "reset";
  }
  if (!input_before_marker || !input_after_marker || input_before_marker === input_after_marker) {
    persistence_changed_dimensions = [];
    return "not_evaluated";
  }
  if (beforeReloadText !== input_after_marker) {
    persistence_changed_dimensions = [];
    return "not_evaluated";
  }
  persistence_changed_dimensions = ["marker"];
  return beforeReloadText === afterReloadText ? "preserved" : "reset";
}

async function waitForAnySurface(page) {
  const surface = page.locator("canvas, button, [role=button], input, select, textarea, [contenteditable='true'], [data-anvil-action], [data-anvil-state]").first();
  await surface.waitFor({ timeout: 10000 });
}

async function evaluatePersistenceReload(page, mode) {
  if (!persistenceRequired) return;
  if (!steps.includes("input_state_change")) {
    steps.push("persistence_reload:not_evaluated");
    persistence_after_reload = "not_evaluated";
    return;
  }
  mark("persistence_reload");
  persistence_before_reload_marker = await activeMarker(page, mode);
  try {
    await page.reload({ waitUntil: "domcontentloaded", timeout: GOTO_TIMEOUT_MS });
    await waitForAnySurface(page);
    await page.waitForTimeout(120);
    persistence_after_reload_marker = await activeMarker(page, mode);
    persistence_after_reload = evaluatePersistenceAfterReload(
      mode,
      persistence_before_reload_marker,
      persistence_after_reload_marker
    );
    steps.push(
      persistence_after_reload === "not_evaluated"
        ? "persistence_reload:not_evaluated"
        : "persistence_reload"
    );
  } catch (err) {
    persistence_after_reload = "not_evaluated";
    steps.push("persistence_reload:not_evaluated");
    informational_failure_kinds.push("persistence_reload_not_evaluated");
  }
}

function contractResetWardChange(beforeText, afterText, baselineText) {
  if (!beforeText || !afterText || beforeText === afterText) return false;
  const changed = changedTopLevelStateKeys(beforeText, afterText);
  if (changed.length === 0) return false;
  const before = contractStatesFromMarker(beforeText);
  const after = contractStatesFromMarker(afterText);
  const baseline = contractStatesFromMarker(baselineText);
  const count = Math.max(before.length, after.length, baseline.length);
  for (let index = 0; index < count; index += 1) {
    const beforeState = before[index] || {};
    const afterState = after[index] || {};
    const baselineState = baseline[index] || {};
    for (const key of changed) {
      const beforeValue = beforeState[key];
      const afterValue = afterState[key];
      const baselineValue = baselineState[key];
      if (
        stableStateString(afterValue) === stableStateString(baselineValue) &&
        stableStateString(beforeValue) !== stableStateString(baselineValue)
      ) {
        return true;
      }
      if (
        typeof beforeValue === "number" &&
        typeof afterValue === "number" &&
        typeof baselineValue === "number" &&
        Math.abs(afterValue - baselineValue) < Math.abs(beforeValue - baselineValue)
      ) {
        return true;
      }
    }
  }
  return true;
}

async function contractHookStatus(page) {
  return await page.evaluate(() => {
    const textOf = (el) => (
      el.getAttribute("aria-label") ||
      el.getAttribute("title") ||
      el.textContent ||
      el.value ||
      ""
    ).trim();
    const primary = document.querySelector('[data-anvil-action="primary"]');
    const restart = document.querySelector('[data-anvil-action="restart"]');
    const action_hooks = Array.from(document.querySelectorAll("[data-anvil-action]"))
      .map((el) => (el.getAttribute("data-anvil-action") || "").trim())
      .filter(Boolean)
      .filter((value, index, values) => values.indexOf(value) === index)
      .sort();
    const stateEls = Array.from(document.querySelectorAll("[data-anvil-state]"));
    let valid_state_count = 0;
    let invalid_state_count = 0;
    for (const el of stateEls) {
      try {
        JSON.parse(el.getAttribute("data-anvil-state") || "");
        valid_state_count += 1;
      } catch (_) {
        invalid_state_count += 1;
      }
    }
    const primary_present = !!primary;
    const state_present = stateEls.length > 0;
    const usable = primary_present && valid_state_count > 0;
    const status = usable
      ? "usable"
      : !primary_present
        ? "primary_missing"
        : !state_present
          ? "state_missing"
          : "state_invalid";
    return {
      status,
      usable,
      primary_present,
      primary_text_excerpt: primary ? textOf(primary).slice(0, 80) : "",
      restart_present: !!restart,
      restart_text_excerpt: restart ? textOf(restart).slice(0, 80) : "",
      action_hooks,
      state_present,
      state_count: stateEls.length,
      valid_state_count,
      invalid_state_count
    };
  });
}

async function activeMarker(page, mode) {
  return mode === "contract" ? await contractStateMarker(page) : await marker(page);
}

async function markerAfterActiveChange(page, mode, previous, timeoutMs) {
  const deadline = Date.now() + timeoutMs;
  let current = await activeMarker(page, mode);
  while (Date.now() < deadline) {
    if (current !== previous) {
      return current;
    }
    await page.waitForTimeout(80);
    current = await activeMarker(page, mode);
  }
  return current;
}

async function controlText(locator) {
  try {
    return await locator.evaluate((el) => (
      el.getAttribute("aria-label") ||
      el.getAttribute("title") ||
      el.textContent ||
      el.value ||
      ""
    ).trim());
  } catch (_) {
    return "";
  }
}

async function rankedControlCandidates(page, skipContractPrimary) {
  return await page.locator("button,[role=button]").evaluateAll((els, skipPrimary) => {
    const viewportWidth = window.innerWidth || document.documentElement.clientWidth || 1;
    const viewportHeight = window.innerHeight || document.documentElement.clientHeight || 1;
    const centerX = viewportWidth / 2;
    const centerY = viewportHeight / 2;
    const maxDistance = Math.sqrt(centerX * centerX + centerY * centerY) || 1;
    const textOf = (el) => (
      el.getAttribute("aria-label") ||
      el.getAttribute("title") ||
      el.textContent ||
      el.value ||
      ""
    ).trim().replace(/\s+/g, " ");
    const visible = (el, box) => {
      const style = window.getComputedStyle(el);
      return box.width > 0
        && box.height > 0
        && style.visibility !== "hidden"
        && style.display !== "none"
        && Number(style.opacity || "1") > 0;
    };
    return els
      .map((el, index) => {
        const box = el.getBoundingClientRect();
        const text = textOf(el);
        const area = Math.round(box.width * box.height);
        const distance = Math.sqrt(
          Math.pow((box.left + box.width / 2) - centerX, 2) +
          Math.pow((box.top + box.height / 2) - centerY, 2)
        );
        const centrality_milli = Math.round(Math.max(0, 1 - distance / maxDistance) * 1000);
        return {
          index,
          text_excerpt: text.slice(0, 80),
          text_len: text.length,
          text_bucket: text.length >= 2 ? 1 : 0,
          area,
          centrality_milli,
          contract_primary: el.getAttribute("data-anvil-action") === "primary",
          visible: visible(el, box)
        };
      })
      .filter((candidate) => candidate.visible)
      .filter((candidate) => !(skipPrimary && candidate.contract_primary))
      .sort((a, b) =>
        (b.text_bucket - a.text_bucket) ||
        (b.area - a.area) ||
        (b.centrality_milli - a.centrality_milli)
      )
      .slice(0, 4)
      .map((candidate, rank) => ({ ...candidate, rank: rank + 1 }));
  }, skipContractPrimary);
}

async function attemptRankedCandidateTransitions(page, mode, skipContractPrimary) {
  const candidates = await rankedControlCandidates(page, skipContractPrimary);
  for (const candidate of candidates) {
    const entry = {
      rank: candidate.rank,
      index: candidate.index,
      text_excerpt: candidate.text_excerpt,
      area: candidate.area,
      centrality_milli: candidate.centrality_milli,
      changed: false
    };
    const before = await activeMarker(page, mode);
    try {
      await page.locator("button,[role=button]").nth(candidate.index).click({ timeout: 1200 });
      const after = await markerAfterActiveChange(page, mode, before, 800);
      entry.changed = before !== after;
      candidate_table.push(entry);
      if (entry.changed) {
        return { observed: true, before, after, source: "candidate", candidate: entry };
      }
    } catch (_) {
      candidate_table.push(entry);
    }
  }
  return { observed: false, before: "", after: "", source: "", candidate: null };
}

async function hasStartLikeControl(page) {
  return await page.locator("button,[role=button],input[type=button],input[type=submit]").evaluateAll((els) => {
    const textOf = (el) => (
      el.getAttribute("aria-label") ||
      el.getAttribute("title") ||
      el.textContent ||
      el.value ||
      ""
    ).trim();
    const startPattern = /(start|begin|play|restart|retry|new game|スタート|開始|再開|リスタート)/i;
    return els.some((el) => {
      const box = el.getBoundingClientRect();
      const style = window.getComputedStyle(el);
      const visible = box.width > 0
        && box.height > 0
        && style.visibility !== "hidden"
        && style.display !== "none"
        && Number(style.opacity || "1") > 0;
      return visible && startPattern.test(textOf(el));
    });
  });
}

async function dispatchPostTransitionInputs(page, mode) {
  input_before_marker = await activeMarker(page, mode);
  const canvas = page.locator("canvas").first();
  let clicked = false;
  if (await canvas.count()) {
    const box = await canvas.boundingBox();
    if (box) {
      await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
      clicked = true;
    }
  }
  if (!clicked) {
    const viewport = page.viewportSize() || { width: 1280, height: 720 };
    await page.mouse.click(viewport.width / 2, viewport.height / 2);
  }
  input_dispatches.push("canvas/center click");
  for (const key of ["ArrowLeft", "ArrowRight", "Space"]) {
    await page.keyboard.down(key);
    await page.keyboard.up(key);
    input_dispatches.push(`${key} keydown`);
  }
  steps.push("control_input_dispatched");
  steps.push("input_state_evaluated_after_start");
  input_state_evaluated_after_start = true;
  input_after_marker = await markerAfterActiveChange(page, mode, input_before_marker, 800);
  if (mode === "contract") {
    mergeStateDimensionsChanged(changedTopLevelStateKeys(input_before_marker, input_after_marker));
  }
  if (input_before_marker !== input_after_marker) {
    steps.push("input_state_change");
  }
}

async function dispatchStartlessTextInput(page, mode) {
  const target = page.locator('input:not([type="hidden"]):not([disabled]), textarea:not([disabled]), [contenteditable="true"]').first();
  if (!(await target.count())) {
    return false;
  }
  input_before_marker = await activeMarker(page, mode);
  try {
    await target.click({ timeout: 1200 });
    const tag = await target.evaluate((el) => el.tagName.toLowerCase());
    if (tag === "input" || tag === "textarea") {
      await target.fill("anvil probe input", { timeout: 1200 });
    } else {
      await page.keyboard.type("anvil probe input");
    }
    input_dispatches.push("direct text input");
    steps.push("control_input_dispatched");
    steps.push("input_state_evaluated_after_start");
    input_state_evaluated_after_start = true;
    input_after_marker = await markerAfterActiveChange(page, mode, input_before_marker, 800);
    if (input_before_marker !== input_after_marker) {
      steps.push("input_state_change");
      return true;
    }
  } catch (_) {}
  return false;
}

async function recoveryCandidateIndex(page, initialStartText) {
  return await page.locator("button,[role=button]").evaluateAll((els, initialText) => {
    const normalizedInitial = (initialText || "").trim();
    const textOf = (el) => (
      el.getAttribute("aria-label") ||
      el.getAttribute("title") ||
      el.textContent ||
      el.value ||
      ""
    ).trim();
    const candidates = els
      .map((el, index) => ({ index, text: textOf(el) }))
      .filter((candidate) => candidate.text.length > 0);
    const changed = candidates.find((candidate) => candidate.text !== normalizedInitial);
    if (changed) return changed.index;
    const rerendered = candidates.find((candidate) => normalizedInitial && candidate.text === normalizedInitial);
    return rerendered ? rerendered.index : -1;
  }, initialStartText);
}

async function attemptRecoveryTransition(page, initialStartText, mode) {
  const deadline = Date.now() + 1200;
  while (Date.now() < deadline) {
    const index = await recoveryCandidateIndex(page, initialStartText);
    if (index >= 0) {
      const candidate = page.locator("button,[role=button]").nth(index);
      recovery_before_marker = await activeMarker(page, mode);
      try {
        await candidate.click({ timeout: 1000 });
        recovery_after_marker = await markerAfterActiveChange(page, mode, recovery_before_marker, 700);
        if (recovery_before_marker !== recovery_after_marker) {
          steps.push("recovery_transition");
          recovery_transition_status = "observed";
          return;
        }
      } catch (_) {}
    }
    await page.waitForTimeout(120);
  }
  steps.push("recovery_transition:not_observed");
  recovery_transition_status = "not_observed";
}

async function attemptContractRecoveryTransition(page, postStartMarker) {
  const deadline = Date.now() + 1200;
  while (Date.now() < deadline) {
    let candidate = page.locator('[data-anvil-action="restart"]').first();
    if (!(await candidate.count())) {
      candidate = page.locator('[data-anvil-action="primary"]').first();
    }
    if (await candidate.count()) {
      recovery_before_marker = await activeMarker(page, "contract");
      try {
        await candidate.click({ timeout: 1000 });
        recovery_after_marker = await markerAfterActiveChange(page, "contract", recovery_before_marker, 700);
        if (contractResetWardChange(recovery_before_marker, recovery_after_marker, postStartMarker)) {
          steps.push("recovery_transition");
          recovery_transition_status = "observed";
          return;
        }
      } catch (_) {}
    }
    await page.waitForTimeout(120);
  }
  steps.push("recovery_transition:not_observed");
  recovery_transition_status = "not_observed";
}

function interactionFailureKind(transitionObserved, inputEvaluated, inputStateChanged) {
  if (!transitionObserved) return "start_transition_missing";
  if (!inputEvaluated) return "input_state_change_not_evaluated_after_start";
  if (!inputStateChanged) return "input_state_change_missing_after_start";
  return "";
}

(async () => {
  let browser;
  try {
    mark("resolving");
    const { chromium } = require("playwright");
    mark("launching");
    browser = await chromium.launch({ headless: true, timeout: LAUNCH_TIMEOUT_MS });
    const page = await browser.newPage();
    mark("server_check");
    server_check = await rawHttpGet(url);
    if (!server_check.ok) {
      const err = new Error(server_check.error || "server_unreachable");
      err.anvilFailureKind = "probe_infrastructure_failed:server_unreachable";
      throw err;
    }
    mark("navigating");
    await gotoWithRetry(page, url);
    mark("surface_wait");
    const surface = page.locator("canvas, button, [role=button], input, select, textarea, [contenteditable='true'], [data-anvil-action], [data-anvil-state]").first();
    await surface.waitFor({ timeout: 10000 });
    steps.push("surface_visible");
    mark("observing");
    post_js_surface = await surfaceSnapshot(page);
    contract_hooks = await contractHookStatus(page);
    contract_hook_status = contract_hooks.status;
    action_hooks = contract_hooks.action_hooks || [];
    probe_mode = contract_hooks.usable ? "contract" : "heuristic";
    before_marker = await activeMarker(page, probe_mode);

    let initial_start_text = "";
    let transitionObserved = false;
    if (probe_mode === "contract") {
      const startControl = page.locator('[data-anvil-action="primary"]').first();
      if (await startControl.count()) {
        initial_start_text = await controlText(startControl);
        await startControl.click({ timeout: 5000 });
        after_marker = await markerAfterActiveChange(page, probe_mode, before_marker, 800);
        if (before_marker !== after_marker) {
          primary_transition_observed = true;
          transitionObserved = true;
          steps.push("start_transition");
        } else {
          steps.push("primary_start_transition_missing");
        }
      }
      if (!transitionObserved) {
        const fallback = await attemptRankedCandidateTransitions(page, probe_mode, true);
        if (fallback.observed) {
          after_marker = fallback.after;
          transitionObserved = true;
          steps.push("start_transition");
        }
      }
    } else {
      const fallback = await attemptRankedCandidateTransitions(page, probe_mode, false);
      if (fallback.observed) {
        before_marker = fallback.before;
        after_marker = fallback.after;
        transitionObserved = true;
        primary_transition_observed = candidate_table.length > 0 && candidate_table[0].changed;
        steps.push("start_transition");
        initial_start_text = fallback.candidate ? fallback.candidate.text_excerpt : "";
      } else {
        after_marker = await activeMarker(page, probe_mode);
        primary_transition_observed = false;
      }
    }

    if (transitionObserved) {
      await dispatchPostTransitionInputs(page, probe_mode);
    }

    if (!transitionObserved) {
      start_control_found = await hasStartLikeControl(page);
      if (!start_control_found) {
        await dispatchStartlessTextInput(page, probe_mode);
      }
    }

    if (transitionObserved && probe_mode === "contract") {
      await attemptContractRecoveryTransition(page, after_marker);
    } else {
      await attemptRecoveryTransition(page, initial_start_text, probe_mode);
    }
    if (!transitionObserved && steps.includes("recovery_transition")) {
      transitionObserved = true;
      steps.push("start_transition");
      after_marker = recovery_after_marker;
      await dispatchPostTransitionInputs(page, probe_mode);
    }

    await evaluatePersistenceReload(page, probe_mode);

    if (!primary_transition_observed && transitionObserved) {
      informational_failure_kinds.push("primary_start_transition_missing");
    }

    const inputStateChanged = steps.includes("input_state_change");
    const startlessInputObserved = !start_control_found && inputStateChanged;
    const ok = steps.includes("surface_visible")
      && inputStateChanged
      && ((transitionObserved && input_state_evaluated_after_start) || startlessInputObserved)
      && (!persistenceRequired || persistence_after_reload !== "reset");
    const recoveryObserved = steps.includes("recovery_transition");
    const failureKind = persistenceRequired && persistence_after_reload === "reset"
      ? "persistence_after_reload_reset"
      : interactionFailureKind(
      transitionObserved || startlessInputObserved,
      input_state_evaluated_after_start || startlessInputObserved,
      inputStateChanged
    );
    write({
      ok,
      status: ok ? "passed" : "failed",
      interaction_success: ok,
      interaction_performed: ok,
      input_event_observed: steps.includes("control_input_dispatched"),
      input_state_change: inputStateChanged,
      state_changed: inputStateChanged,
      visible_state_changed: inputStateChanged,
      recovery_transition: recoveryObserved,
      recovery_transition_status,
      start_transition: transitionObserved,
      start_control_found,
      primary_start_transition: primary_transition_observed,
      primary_start_transition_missing: !primary_transition_observed && transitionObserved,
      input_state_evaluated_after_start,
      probe_mode,
      contract_hook_status,
      contract_hooks,
      candidate_table,
      input_dispatches,
      state_dimensions_changed,
      persistence_after_reload,
      persistence_changed_dimensions,
      persistence_before_reload_marker,
      persistence_after_reload_marker,
      action_hooks,
      informational_failure_kinds,
      steps,
      stage,
      before_marker,
      after_marker,
      input_before_marker,
      input_after_marker,
      recovery_before_marker,
      recovery_after_marker,
      failure_kind: ok ? "" : failureKind,
      server_http_status: server_check.status,
      server_check,
      post_js_has_canvas: post_js_surface ? post_js_surface.has_canvas : false,
      post_js_canvas_count: post_js_surface ? post_js_surface.canvas_count : 0,
      post_js_interactive_control_count: post_js_surface ? post_js_surface.interactive_control_count : 0,
      has_canvas: post_js_surface ? post_js_surface.has_canvas : false,
      canvas_count: post_js_surface ? post_js_surface.canvas_count : 0,
      interactive_control_count: post_js_surface ? post_js_surface.interactive_control_count : 0,
      title_text_excerpt: post_js_surface ? post_js_surface.title_text_excerpt : "",
      duration_ms: Date.now() - started
    });
    await browser.close();
    process.exit(ok ? 0 : 1);
  } catch (err) {
    if (browser) {
      try { await browser.close(); } catch (_) {}
    }
    writeFailure({
      ok: false,
      status: "failed",
      steps,
      stage,
      before_marker,
      after_marker,
      input_before_marker,
      input_after_marker,
      recovery_before_marker,
      recovery_after_marker,
      input_state_change: steps.includes("input_state_change"),
      state_changed: steps.includes("input_state_change"),
      visible_state_changed: steps.includes("input_state_change"),
      recovery_transition: steps.includes("recovery_transition"),
      recovery_transition_status,
      start_transition: steps.includes("start_transition") || steps.includes("recovery_transition"),
      start_control_found,
      primary_start_transition: primary_transition_observed,
      primary_start_transition_missing: !primary_transition_observed && (steps.includes("start_transition") || steps.includes("recovery_transition")),
      input_state_evaluated_after_start,
      probe_mode,
      contract_hook_status,
      contract_hooks,
      candidate_table,
      input_dispatches,
      state_dimensions_changed,
      persistence_after_reload,
      persistence_changed_dimensions,
      persistence_before_reload_marker,
      persistence_after_reload_marker,
      action_hooks,
      informational_failure_kinds,
      failure_kind: err && err.anvilFailureKind ? err.anvilFailureKind : "probe_script_error",
      navigation_failure_kind: err && err.navigationFailureKind ? err.navigationFailureKind : "",
      error: err && err.message ? err.message : String(err),
      server_http_status: server_check.status,
      server_http_error: server_check.error || "",
      server_check,
      post_js_has_canvas: post_js_surface ? post_js_surface.has_canvas : null,
      post_js_canvas_count: post_js_surface ? post_js_surface.canvas_count : null,
      post_js_interactive_control_count: post_js_surface ? post_js_surface.interactive_control_count : null,
      has_canvas: post_js_surface ? post_js_surface.has_canvas : null,
      canvas_count: post_js_surface ? post_js_surface.canvas_count : null,
      interactive_control_count: post_js_surface ? post_js_surface.interactive_control_count : null,
      title_text_excerpt: post_js_surface ? post_js_surface.title_text_excerpt : "",
      duration_ms: Date.now() - started
    });
    process.exit(1);
  }
})();
"#
}

fn open_stdio_logs(run_dir: &Path) -> std::io::Result<(std::fs::File, std::fs::File)> {
    std::fs::create_dir_all(run_dir)?;
    Ok((
        std::fs::File::create(run_dir.join("browser-interaction.out"))?,
        std::fs::File::create(run_dir.join("browser-interaction.err"))?,
    ))
}

#[derive(Debug, Clone, Default)]
struct InteractionStdio {
    output_excerpt: String,
    stdout_raw: String,
    stdout_excerpt: String,
    stderr_excerpt: String,
    raw_stdout_excerpt: String,
}

fn interaction_stdio_logs(run_dir: &Path) -> InteractionStdio {
    let stdout =
        std::fs::read_to_string(run_dir.join("browser-interaction.out")).unwrap_or_default();
    let stderr =
        std::fs::read_to_string(run_dir.join("browser-interaction.err")).unwrap_or_default();
    let stdout_trimmed = stdout.trim();
    let stderr_tail = last_lines(&stderr, 20);
    InteractionStdio {
        output_excerpt: eval_events::body_snippet(format!("{stdout}\n{stderr}").trim()),
        stdout_raw: stdout.clone(),
        stdout_excerpt: eval_events::body_snippet(stdout_trimmed),
        stderr_excerpt: eval_events::body_snippet(stderr_tail.trim()),
        raw_stdout_excerpt: if stdout_failure_json(&stdout).is_some() {
            String::new()
        } else {
            eval_events::body_snippet(stdout_trimmed)
        },
    }
}

fn last_lines(text: &str, max_lines: usize) -> String {
    let mut lines = text.lines().rev().take(max_lines).collect::<Vec<_>>();
    lines.reverse();
    lines.join("\n")
}

fn stdout_failure_json(stdout: &str) -> Option<Value> {
    stdout
        .lines()
        .rev()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .find_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(Value::is_object)
}

fn merge_script_stdout_failure_value(mut value: Value, logs: &InteractionStdio) -> Value {
    if let Some(stdout_value) = stdout_failure_json(&logs.stdout_raw) {
        for key in [
            "error",
            "stage",
            "before_marker",
            "after_marker",
            "input_before_marker",
            "input_after_marker",
            "recovery_before_marker",
            "recovery_after_marker",
            "start_transition",
            "start_control_found",
            "primary_start_transition",
            "primary_start_transition_missing",
            "input_state_evaluated_after_start",
            "probe_mode",
            "contract_hook_status",
            "contract_hooks",
            "candidate_table",
            "input_dispatches",
            "state_dimensions_changed",
            "persistence_after_reload",
            "persistence_changed_dimensions",
            "persistence_before_reload_marker",
            "persistence_after_reload_marker",
            "action_hooks",
            "informational_failure_kinds",
            "failure_kind",
            "navigation_failure_kind",
            "server_http_status",
            "server_http_error",
            "server_check",
            "post_js_has_canvas",
            "post_js_canvas_count",
            "post_js_interactive_control_count",
            "has_canvas",
            "canvas_count",
            "interactive_control_count",
        ] {
            if let Some(field) = stdout_value.get(key) {
                value[key] = field.clone();
            }
        }
    } else if !logs.raw_stdout_excerpt.is_empty() {
        value["raw_stdout_excerpt"] = json!(logs.raw_stdout_excerpt);
    }
    if !logs.stderr_excerpt.is_empty() {
        value["stderr_excerpt"] = json!(logs.stderr_excerpt);
    }
    value
}

fn stage_from_probe_output(output: &str) -> Option<String> {
    output
        .lines()
        .rev()
        .filter_map(stage_from_probe_output_line)
        .next()
}

fn stage_from_probe_output_line(line: &str) -> Option<String> {
    serde_json::from_str::<Value>(line.trim())
        .ok()
        .and_then(|value| {
            value
                .get("stage")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|stage| !stage.is_empty())
}

fn read_interaction_value(path: &Path) -> Option<Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
}

#[allow(clippy::too_many_arguments)]
fn observation_from_value(
    evidence_path: &Path,
    script_path: &Path,
    url: &str,
    value: Value,
    started: Instant,
    output_excerpt: String,
    stdout_excerpt: String,
    stderr_excerpt: String,
    raw_stdout_excerpt: String,
    child_spawned: bool,
    child_reaped: bool,
    playwright_resolution: Option<PlaywrightResolution>,
) -> BrowserInteractionObservation {
    let raw_ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let raw_failure_kind = value
        .get("failure_kind")
        .or_else(|| value.get("browser_failure_kind"))
        .and_then(Value::as_str)
        .unwrap_or(if raw_ok {
            ""
        } else {
            "browser_interaction_failed"
        })
        .to_string();
    let stage = value
        .get("stage")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("probe")
                .and_then(|probe| probe.get("stage"))
                .and_then(Value::as_str)
        })
        .unwrap_or("")
        .to_string();
    let value_error = value
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let server_http_status = value
        .get("server_http_status")
        .or_else(|| {
            value
                .get("server_check")
                .and_then(|server_check| server_check.get("status"))
        })
        .and_then(Value::as_i64);
    let server_http_error = value
        .get("server_http_error")
        .or_else(|| {
            value
                .get("server_check")
                .and_then(|server_check| server_check.get("error"))
        })
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let navigation_failure_kind = value
        .get("navigation_failure_kind")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let duration_ms = value
        .get("duration_ms")
        .and_then(Value::as_u64)
        .map(u128::from)
        .unwrap_or_else(|| started.elapsed().as_millis());
    let steps = value
        .get("steps")
        .and_then(Value::as_array)
        .map(|steps| {
            steps
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let recovery_transition_observed = steps.iter().any(|step| step == "recovery_transition")
        || value
            .get("recovery_transition")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let start_transition_observed = steps.iter().any(|step| step == "start_transition")
        || value
            .get("start_transition")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        || recovery_transition_observed;
    let input_state_evaluated_after_start = value
        .get("input_state_evaluated_after_start")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            steps
                .iter()
                .any(|step| step == "input_state_evaluated_after_start")
                || (start_transition_observed
                    && value
                        .get("input_before_marker")
                        .and_then(Value::as_str)
                        .is_some_and(|marker| !marker.is_empty())
                    && value
                        .get("input_after_marker")
                        .and_then(Value::as_str)
                        .is_some_and(|marker| !marker.is_empty()))
        });
    let explicit_state_changed = value
        .get("input_state_change")
        .or_else(|| value.get("state_changed"))
        .or_else(|| value.get("visible_state_changed"))
        .and_then(Value::as_bool);
    let input_state_changed = steps.iter().any(|step| step == "input_state_change")
        || explicit_state_changed == Some(true);
    let recovery_transition_not_observed = steps
        .iter()
        .any(|step| step == "recovery_transition:not_observed")
        || value.get("recovery_transition").and_then(Value::as_bool) == Some(false)
        || value
            .get("recovery_transition_status")
            .and_then(Value::as_str)
            == Some("not_observed");
    let start_control_found = value
        .get("start_control_found")
        .or_else(|| value.get("start_control_present"))
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let surface_visible = steps.iter().any(|step| step == "surface_visible")
        || value.get("surface_visible").and_then(Value::as_bool) == Some(true)
        || value.get("interactive_surface").and_then(Value::as_bool) == Some(true);
    let startless_input_observed = !start_control_found && surface_visible && input_state_changed;
    let taxonomy_failure_kind = interaction_taxonomy_failure_kind(
        start_transition_observed || startless_input_observed,
        input_state_evaluated_after_start || startless_input_observed,
        input_state_changed,
    );
    let ok = raw_ok && taxonomy_failure_kind.is_empty();
    let effective_raw_failure_kind =
        effective_interaction_failure_kind(&raw_failure_kind, taxonomy_failure_kind);
    let failure_kind = normalized_interaction_failure_kind(
        ok,
        &stage,
        effective_raw_failure_kind,
        value_error,
        &output_excerpt,
        server_http_status,
        &server_http_error,
    );
    let remediation = interaction_failure_remediation(&failure_kind);
    let probe_mode = value
        .get("probe_mode")
        .and_then(Value::as_str)
        .unwrap_or("heuristic")
        .to_string();
    let contract_hook_status = value
        .get("contract_hook_status")
        .and_then(Value::as_str)
        .or_else(|| {
            value
                .get("contract_hooks")
                .and_then(|hooks| hooks.get("status"))
                .and_then(Value::as_str)
        })
        .unwrap_or("unknown")
        .to_string();
    let candidate_table = interaction_candidate_table(&value);
    let input_dispatches = string_array_field(&value, "input_dispatches");
    let state_dimensions_changed = string_array_field(&value, "state_dimensions_changed");
    let persistence_after_reload = value
        .get("persistence_after_reload")
        .and_then(Value::as_str)
        .unwrap_or("not_evaluated")
        .to_string();
    let persistence_changed_dimensions =
        string_array_field(&value, "persistence_changed_dimensions");
    let action_hooks = string_array_field(&value, "action_hooks")
        .into_iter()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let primary_transition_observed = value
        .get("primary_start_transition")
        .or_else(|| value.get("primary_transition_observed"))
        .and_then(Value::as_bool)
        .unwrap_or_else(|| {
            start_transition_observed
                && !steps
                    .iter()
                    .any(|step| step == "primary_start_transition_missing")
        });
    let informational_failure_kinds = string_array_field(&value, "informational_failure_kinds");
    BrowserInteractionObservation {
        ok,
        status: if ok { "passed" } else { "failed" }.to_string(),
        url: url.to_string(),
        evidence_path: evidence_path.to_path_buf(),
        script_path: script_path.to_path_buf(),
        steps,
        before_marker: value
            .get("before_marker")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        after_marker: value
            .get("after_marker")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        input_before_marker: value
            .get("input_before_marker")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        input_after_marker: value
            .get("input_after_marker")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        recovery_before_marker: value
            .get("recovery_before_marker")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        recovery_after_marker: value
            .get("recovery_after_marker")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        input_state_changed,
        input_state_evaluated_after_start,
        probe_mode,
        contract_hook_status,
        candidate_table,
        input_dispatches,
        state_dimensions_changed,
        persistence_after_reload,
        persistence_changed_dimensions,
        persistence_before_reload_marker: value
            .get("persistence_before_reload_marker")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        persistence_after_reload_marker: value
            .get("persistence_after_reload_marker")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string(),
        action_hooks,
        primary_transition_observed,
        start_control_found,
        informational_failure_kinds,
        recovery_transition_observed,
        recovery_transition_not_observed,
        failure_kind,
        stage,
        error: value_error.to_string(),
        remediation,
        duration_ms,
        output_excerpt: eval_events::body_snippet(&output_excerpt),
        stdout_excerpt: eval_events::body_snippet(&stdout_excerpt),
        stderr_excerpt: eval_events::body_snippet(&stderr_excerpt),
        raw_stdout_excerpt: eval_events::body_snippet(
            value
                .get("raw_stdout_excerpt")
                .and_then(Value::as_str)
                .unwrap_or(&raw_stdout_excerpt),
        ),
        child_spawned,
        child_reaped,
        playwright_resolution,
        server_http_status,
        server_http_error,
        navigation_failure_kind,
        has_canvas: bool_value(
            &value,
            &[
                "post_js_has_canvas",
                "has_canvas",
                "canvas_found",
                "canvas_available",
            ],
        ),
        interactive_control_count: usize_value(
            &value,
            &[
                "post_js_interactive_control_count",
                "interactive_control_count",
                "interactive_controls",
            ],
        ),
    }
}

fn bool_value(value: &Value, keys: &[&str]) -> Option<bool> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_bool))
}

fn usize_value(value: &Value, keys: &[&str]) -> Option<usize> {
    for key in keys {
        let Some(raw) = value.get(*key) else {
            continue;
        };
        if let Some(number) = raw.as_u64()
            && let Ok(value) = usize::try_from(number)
        {
            return Some(value);
        }
        if let Some(text) = raw.as_str()
            && let Ok(value) = text.parse::<usize>()
        {
            return Some(value);
        }
    }
    None
}

fn string_array_field(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

fn interaction_candidate_table(value: &Value) -> Vec<InteractionProbeCandidateEvidence> {
    value
        .get("candidate_table")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|candidate| InteractionProbeCandidateEvidence {
            rank: candidate
                .get("rank")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(0),
            index: candidate
                .get("index")
                .and_then(Value::as_u64)
                .and_then(|value| usize::try_from(value).ok())
                .unwrap_or(0),
            text_excerpt: candidate
                .get("text_excerpt")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
            changed: candidate
                .get("changed")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
        .collect()
}

fn interaction_taxonomy_failure_kind(
    transition_observed: bool,
    input_evaluated_after_start: bool,
    input_state_changed: bool,
) -> &'static str {
    if !transition_observed {
        "start_transition_missing"
    } else if !input_evaluated_after_start {
        "input_state_change_not_evaluated_after_start"
    } else if !input_state_changed {
        "input_state_change_missing_after_start"
    } else {
        ""
    }
}

fn effective_interaction_failure_kind<'a>(
    raw_failure_kind: &'a str,
    taxonomy_failure_kind: &'static str,
) -> &'a str {
    if taxonomy_failure_kind.is_empty() {
        return "";
    }
    match raw_failure_kind {
        ""
        | "browser_interaction_failed"
        | "start_transition_missing"
        | "interaction_state_change_missing"
        | "input_state_change_missing" => taxonomy_failure_kind,
        _ => raw_failure_kind,
    }
}

#[allow(clippy::too_many_arguments)]
fn failure_observation(
    root: &Path,
    evidence_path: &Path,
    script_path: &Path,
    port: u16,
    started: Instant,
    failure_kind: &str,
    output_excerpt: &str,
    stdout_raw: &str,
    stdout_excerpt: &str,
    stderr_excerpt: &str,
    raw_stdout_excerpt: &str,
    child_spawned: bool,
    child_reaped: bool,
    playwright_resolution: Option<PlaywrightResolution>,
) -> BrowserInteractionObservation {
    let url = format!("http://127.0.0.1:{port}/");
    if failure_kind == "probe_command_failed"
        && let Some(mut value) = stdout_failure_json(stdout_raw)
    {
        if value.get("stderr_excerpt").is_none() && !stderr_excerpt.is_empty() {
            value["stderr_excerpt"] = json!(stderr_excerpt);
        }
        let observation = observation_from_value(
            evidence_path,
            script_path,
            &url,
            value,
            started,
            output_excerpt.to_string(),
            stdout_excerpt.to_string(),
            stderr_excerpt.to_string(),
            raw_stdout_excerpt.to_string(),
            child_spawned,
            child_reaped,
            playwright_resolution,
        );
        mirror_interaction_observation(root, evidence_path, &observation);
        return observation;
    }
    let stage = stage_from_probe_output(output_excerpt).unwrap_or_default();
    let failure_kind = normalized_interaction_failure_kind(
        false,
        &stage,
        failure_kind,
        output_excerpt,
        output_excerpt,
        None,
        "",
    );
    let remediation = interaction_failure_remediation(&failure_kind);
    let observation = BrowserInteractionObservation {
        ok: false,
        status: "failed".to_string(),
        url: url.clone(),
        evidence_path: evidence_path.to_path_buf(),
        script_path: script_path.to_path_buf(),
        steps: Vec::new(),
        before_marker: String::new(),
        after_marker: String::new(),
        input_before_marker: String::new(),
        input_after_marker: String::new(),
        recovery_before_marker: String::new(),
        recovery_after_marker: String::new(),
        input_state_changed: false,
        input_state_evaluated_after_start: false,
        probe_mode: "heuristic".to_string(),
        contract_hook_status: "unknown".to_string(),
        candidate_table: Vec::new(),
        input_dispatches: Vec::new(),
        state_dimensions_changed: Vec::new(),
        persistence_after_reload: "not_evaluated".to_string(),
        persistence_changed_dimensions: Vec::new(),
        persistence_before_reload_marker: String::new(),
        persistence_after_reload_marker: String::new(),
        action_hooks: Vec::new(),
        primary_transition_observed: false,
        start_control_found: true,
        informational_failure_kinds: Vec::new(),
        recovery_transition_observed: false,
        recovery_transition_not_observed: false,
        failure_kind,
        stage,
        error: String::new(),
        remediation,
        duration_ms: started.elapsed().as_millis(),
        output_excerpt: eval_events::body_snippet(output_excerpt),
        stdout_excerpt: eval_events::body_snippet(stdout_excerpt),
        stderr_excerpt: eval_events::body_snippet(stderr_excerpt),
        raw_stdout_excerpt: eval_events::body_snippet(raw_stdout_excerpt),
        child_spawned,
        child_reaped,
        playwright_resolution,
        server_http_status: None,
        server_http_error: String::new(),
        navigation_failure_kind: String::new(),
        has_canvas: None,
        interactive_control_count: None,
    };
    mirror_interaction_observation(root, evidence_path, &observation);
    observation
}

fn interaction_failure_json(
    url: &str,
    failure_kind: &str,
    output_excerpt: &str,
    duration_ms: u128,
) -> Value {
    json!({
        "ok": false,
        "status": "failed",
        "url": url,
        "steps": [],
        "failure_kind": failure_kind,
        "browser_failure_kind": failure_kind,
        "duration_ms": duration_ms,
        "output_excerpt": eval_events::body_snippet(output_excerpt),
    })
}

fn normalized_interaction_failure_kind(
    ok: bool,
    stage: &str,
    raw_failure_kind: &str,
    error: &str,
    output_excerpt: &str,
    server_http_status: Option<i64>,
    server_http_error: &str,
) -> String {
    if ok {
        return String::new();
    }
    let combined = format!("{raw_failure_kind}\n{error}\n{output_excerpt}");
    if playwright_browser_binaries_missing(&combined) {
        return "probe_dependency_missing:browser_binaries_missing".to_string();
    }
    if playwright_module_missing(&combined) {
        return "probe_dependency_missing:playwright_module_missing".to_string();
    }
    if raw_failure_kind.starts_with("probe_dependency_missing")
        || raw_failure_kind.starts_with("probe_infrastructure_failed")
    {
        return raw_failure_kind.to_string();
    }
    if raw_failure_kind == "app_route_unresponsive" {
        return raw_failure_kind.to_string();
    }
    if raw_failure_kind == "interaction_state_change_missing"
        || raw_failure_kind == "input_state_change_missing"
    {
        return "input_state_change_missing_after_start".to_string();
    }
    if raw_failure_kind.starts_with("probe_navigation_failed") {
        if server_http_status.is_some() && server_http_error.trim().is_empty() {
            return "app_route_unresponsive".to_string();
        }
        return "probe_infrastructure_failed:server_unreachable".to_string();
    }
    if raw_failure_kind == "probe_script_error"
        && stage == "navigating"
        && server_http_status.is_some()
        && server_http_error.trim().is_empty()
        && looks_like_navigation_error(&combined)
    {
        return "app_route_unresponsive".to_string();
    }
    if probe_stage_before_observation(stage)
        || matches!(
            raw_failure_kind,
            "probe_script_write_failed"
                | "probe_stdio_open_failed"
                | "probe_spawn_failed"
                | "probe_status_unreadable"
                | "probe_timeout"
                | "probe_command_failed"
                | "probe_evidence_missing"
        )
    {
        let detail = if raw_failure_kind.trim().is_empty() {
            "unknown"
        } else {
            raw_failure_kind.trim()
        };
        return format!("probe_infrastructure_failed:{detail}");
    }
    raw_failure_kind.to_string()
}

fn probe_stage_before_observation(stage: &str) -> bool {
    matches!(stage, "resolving" | "launching" | "navigating")
}

fn looks_like_navigation_error(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("page.goto")
        || lower.contains("net::err_")
        || lower.contains("navigation")
        || lower.contains("timeout")
        || lower.contains("page crashed")
        || lower.contains("target closed")
}

fn playwright_module_missing(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("cannot find module 'playwright'")
        || lower.contains("cannot find module \"playwright\"")
        || lower.contains("module_not_found")
}

fn playwright_browser_binaries_missing(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("executable doesn't exist")
        || lower.contains("browser executable not found")
        || lower.contains("please run the following command to download new browsers")
        || lower.contains("playwright install")
}

fn interaction_failure_remediation(failure_kind: &str) -> String {
    if failure_kind == "probe_dependency_missing:browser_binaries_missing" {
        PLAYWRIGHT_BROWSER_BINARIES_REMEDIATION.to_string()
    } else if failure_kind == "probe_dependency_missing:playwright_module_missing" {
        INTERACTION_PROBE_SETUP_REMEDIATION.to_string()
    } else {
        String::new()
    }
}

fn interaction_failure_category(failure_kind: &str, stage: &str) -> &'static str {
    let _ = stage;
    if failure_kind.starts_with("probe_dependency_missing")
        || failure_kind.starts_with("probe_infrastructure_failed")
    {
        "infrastructure"
    } else if failure_kind.is_empty() {
        ""
    } else {
        "app"
    }
}

fn interaction_observation_json(observation: &BrowserInteractionObservation) -> Value {
    let mut value = json!({
        "ok": observation.ok,
        "status": observation.status,
        "url": observation.url,
        "interaction_success": observation.ok,
        "interaction_performed": observation.ok,
        "input_event_observed": observation.steps.iter().any(|step| step == "control_input_dispatched"),
        "input_state_change": observation.input_state_changed,
        "state_changed": observation.input_state_changed,
        "visible_state_changed": observation.input_state_changed,
        "start_transition": observation.steps.iter().any(|step| step == "start_transition")
            || observation.recovery_transition_observed,
        "input_state_evaluated_after_start": observation.input_state_evaluated_after_start,
        "probe_mode": observation.probe_mode.as_str(),
        "contract_hook_status": observation.contract_hook_status.as_str(),
        "candidate_table": &observation.candidate_table,
        "input_dispatches": &observation.input_dispatches,
        "state_dimensions_changed": &observation.state_dimensions_changed,
        "persistence_after_reload": observation.persistence_after_reload,
        "persistence_changed_dimensions": &observation.persistence_changed_dimensions,
        "persistence_before_reload_marker": observation.persistence_before_reload_marker,
        "persistence_after_reload_marker": observation.persistence_after_reload_marker,
        "action_hooks": &observation.action_hooks,
        "primary_start_transition": observation.primary_transition_observed,
        "start_control_found": observation.start_control_found,
        "primary_start_transition_missing": !observation.primary_transition_observed
            && observation.steps.iter().any(|step| step == "start_transition"),
        "informational_failure_kinds": &observation.informational_failure_kinds,
        "recovery_transition": observation.recovery_transition_observed,
        "recovery_transition_status": if observation.recovery_transition_observed {
            "observed"
        } else if observation.recovery_transition_not_observed {
            "not_observed"
        } else {
            "unknown"
        },
        "stage": observation.stage,
        "failure_category": interaction_failure_category(&observation.failure_kind, &observation.stage),
        "remediation": observation.remediation,
        "steps": observation.steps,
        "before_marker": observation.before_marker,
        "after_marker": observation.after_marker,
        "input_before_marker": observation.input_before_marker,
        "input_after_marker": observation.input_after_marker,
        "recovery_before_marker": observation.recovery_before_marker,
        "recovery_after_marker": observation.recovery_after_marker,
        "duration_ms": observation.duration_ms,
        "probe": {
            "script_path": observation.script_path.display().to_string(),
            "output_excerpt": observation.output_excerpt,
            "child_spawned": observation.child_spawned,
            "child_reaped": observation.child_reaped,
            "stage": observation.stage,
        }
    });
    if !observation.failure_kind.is_empty() {
        value["failure_kind"] = json!(observation.failure_kind);
        value["browser_failure_kind"] = json!(observation.failure_kind);
    }
    if !observation.output_excerpt.is_empty() {
        value["output_excerpt"] = json!(observation.output_excerpt);
    }
    if !observation.error.is_empty() {
        value["error"] = json!(observation.error);
        value["probe"]["error"] = json!(observation.error);
    }
    if !observation.stdout_excerpt.is_empty() {
        value["stdout_excerpt"] = json!(observation.stdout_excerpt);
        value["probe"]["stdout_excerpt"] = json!(observation.stdout_excerpt);
    }
    if !observation.stderr_excerpt.is_empty() {
        value["stderr_excerpt"] = json!(observation.stderr_excerpt);
        value["probe"]["stderr_excerpt"] = json!(observation.stderr_excerpt);
    }
    if !observation.raw_stdout_excerpt.is_empty() {
        value["raw_stdout_excerpt"] = json!(observation.raw_stdout_excerpt);
        value["probe"]["raw_stdout_excerpt"] = json!(observation.raw_stdout_excerpt);
    }
    if let Some(status) = observation.server_http_status {
        value["server_http_status"] = json!(status);
        value["server_check"] = json!({
            "ok": observation.server_http_error.is_empty(),
            "status": status,
            "error": observation.server_http_error.as_str(),
        });
    } else if !observation.server_http_error.is_empty() {
        value["server_http_error"] = json!(observation.server_http_error.as_str());
        value["server_check"] = json!({
            "ok": false,
            "status": Value::Null,
            "error": observation.server_http_error.as_str(),
        });
    }
    if !observation.navigation_failure_kind.is_empty() {
        value["navigation_failure_kind"] = json!(observation.navigation_failure_kind);
        value["probe"]["navigation_failure_kind"] = json!(observation.navigation_failure_kind);
    }
    if let Some(has_canvas) = observation.has_canvas {
        value["has_canvas"] = json!(has_canvas);
        value["post_js_has_canvas"] = json!(has_canvas);
        value["route_rendered_quality"] = json!(if has_canvas {
            "rendered"
        } else {
            "rendered_without_expected_surface"
        });
    }
    if let Some(count) = observation.interactive_control_count {
        value["interactive_control_count"] = json!(count);
        value["post_js_interactive_control_count"] = json!(count);
    }
    if let Some(resolution) = &observation.playwright_resolution {
        value["playwright_resolution"] = json!(resolution);
        value["playwright_resolution_location"] = json!(resolution.location);
        value["playwright_version"] = json!(resolution.version);
        value["probe"]["playwright_resolution_location"] = json!(resolution.location);
    }
    value
}

fn mirror_interaction_observation(
    root: &Path,
    evidence_path: &Path,
    observation: &BrowserInteractionObservation,
) {
    write_interaction_value(
        root,
        evidence_path,
        &interaction_observation_json(observation),
    );
}

fn write_interaction_value(root: &Path, evidence_path: &Path, value: &Value) {
    if let Ok(text) = serde_json::to_string_pretty(value) {
        write_text(evidence_path, &format!("{text}\n"));
        let workspace_path = browser_interaction_evidence_path(root);
        if workspace_path != evidence_path {
            write_text(&workspace_path, &format!("{text}\n"));
        }
    }
}

fn write_text(path: &Path, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(path, text);
}

fn terminate_child_group(child: &mut Child) {
    #[cfg(unix)]
    {
        if let Ok(pid) = i32::try_from(child.id()) {
            // SAFETY: the process-group id is derived from a child spawned by this probe.
            let _ = unsafe { libc::kill(-pid, libc::SIGTERM) };
            std::thread::sleep(Duration::from_millis(50));
            // SAFETY: the process-group id is derived from a child spawned by this probe.
            let _ = unsafe { libc::kill(-pid, libc::SIGKILL) };
        }
    }
    let _ = child.kill();
}

#[cfg(test)]
fn load_test_availability_override(root: &Path) -> Option<ProbeAvailability> {
    let path = root
        .join(".anvil")
        .join("evidence")
        .join("interaction-probe-availability.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    let available = value
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if available {
        Some(ProbeAvailability::Available(PlaywrightResolution {
            module_path: value
                .get("module_path")
                .and_then(Value::as_str)
                .unwrap_or("test/node_modules/playwright/index.js")
                .to_string(),
            module_dir: value
                .get("module_dir")
                .and_then(Value::as_str)
                .unwrap_or("test/node_modules/playwright")
                .to_string(),
            node_path: value
                .get("node_path")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            location: value
                .get("location")
                .and_then(Value::as_str)
                .unwrap_or("test_override")
                .to_string(),
            version: value
                .get("version")
                .and_then(Value::as_str)
                .unwrap_or("0.0.0-test")
                .to_string(),
        }))
    } else {
        Some(ProbeAvailability::Unavailable(
            value
                .get("reason")
                .and_then(Value::as_str)
                .unwrap_or("playwright_not_installed")
                .to_string(),
        ))
    }
}

#[cfg(test)]
fn load_test_result_override(root: &Path) -> Option<Value> {
    if let Some(value) = load_test_result_sequence_override(root) {
        return Some(value);
    }
    let path = root
        .join(".anvil")
        .join("evidence")
        .join("interaction-probe-result.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .filter(Value::is_object)
}

#[cfg(test)]
fn load_test_result_sequence_override(root: &Path) -> Option<Value> {
    let path = root
        .join(".anvil")
        .join("evidence")
        .join("interaction-probe-results.json");
    let text = std::fs::read_to_string(&path).ok()?;
    let mut values = serde_json::from_str::<Vec<Value>>(&text).ok()?;
    if values.is_empty() {
        return None;
    }
    let value = values.remove(0);
    write_text(
        &path,
        &format!("{}\n", serde_json::to_string_pretty(&values).ok()?),
    );
    value.is_object().then_some(value)
}

#[cfg(test)]
fn load_test_node_program_override(root: &Path) -> Option<PathBuf> {
    let path = root
        .join(".anvil")
        .join("evidence")
        .join("interaction-probe-node-program.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    value
        .get("node_program")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

#[cfg(test)]
pub fn write_test_availability_override(root: &Path, available: bool) {
    write_test_availability_override_with_resolution(root, available, None);
}

#[cfg(test)]
pub fn write_test_availability_override_with_resolution(
    root: &Path,
    available: bool,
    resolution: Option<&PlaywrightResolution>,
) {
    let path = root
        .join(".anvil")
        .join("evidence")
        .join("interaction-probe-availability.json");
    let mut value = json!({
        "available": available,
        "reason": if available { "" } else { "playwright_not_installed" },
    });
    if let Some(resolution) = resolution {
        value["module_path"] = json!(resolution.module_path);
        value["module_dir"] = json!(resolution.module_dir);
        value["node_path"] = json!(resolution.node_path.clone().unwrap_or_default());
        value["location"] = json!(resolution.location);
        value["version"] = json!(resolution.version);
    }
    write_text(
        &path,
        &format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
    );
}

#[cfg(test)]
pub fn write_test_result_override(root: &Path, value: &Value) {
    let path = root
        .join(".anvil")
        .join("evidence")
        .join("interaction-probe-result.json");
    write_text(
        &path,
        &format!("{}\n", serde_json::to_string_pretty(value).unwrap()),
    );
}

#[cfg(test)]
pub fn write_test_result_overrides(root: &Path, values: &[Value]) {
    let path = root
        .join(".anvil")
        .join("evidence")
        .join("interaction-probe-results.json");
    write_text(
        &path,
        &format!("{}\n", serde_json::to_string_pretty(values).unwrap()),
    );
}

#[cfg(test)]
pub fn write_test_node_program_override(root: &Path, node_program: &Path) {
    let path = root
        .join(".anvil")
        .join("evidence")
        .join("interaction-probe-node-program.json");
    write_text(
        &path,
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "node_program": node_program.display().to_string(),
            }))
            .unwrap()
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_playwright_has_no_evidence_side_effect() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            playwright_availability(dir.path()),
            ProbeAvailability::Unavailable("playwright_not_installed".to_string())
        );
        let path = browser_interaction_evidence_path(dir.path());
        let outcome = probe_browser_interaction_against_running_server(
            dir.path(),
            34001,
            dir.path(),
            &path,
            Duration::from_millis(10),
        );
        assert_eq!(
            outcome,
            InteractionProbeOutcome::Unavailable("playwright_not_installed".to_string())
        );
        assert!(!path.exists(), "unavailable probe must not write evidence");
    }

    #[test]
    fn fake_available_probe_writes_ok_evidence() {
        let dir = tempfile::tempdir().unwrap();
        write_test_availability_override(dir.path(), true);
        write_test_result_override(
            dir.path(),
            &json!({
                "ok": true,
                "status": "passed",
                "start_transition": true,
                "input_state_evaluated_after_start": true,
                "input_state_change": true,
                "state_changed": true,
                "visible_state_changed": true,
                "steps": ["surface_visible", "start_transition", "control_input_dispatched", "input_state_evaluated_after_start", "input_state_change"],
                "before_marker": "before",
                "after_marker": "after",
                "duration_ms": 17
            }),
        );
        let run_dir = dir.path().join(".anvil/runs/test");
        let path = run_dir.join("browser-interaction.json");
        let outcome = probe_browser_interaction_against_running_server(
            dir.path(),
            34001,
            &run_dir,
            &path,
            Duration::from_secs(1),
        );
        let observation = outcome.observation().expect("observation");
        assert!(observation.ok, "{observation:?}");
        assert!(path.is_file(), "run evidence");
        assert!(
            browser_interaction_evidence_path(dir.path()).is_file(),
            "workspace evidence mirror"
        );
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("start_transition"));
        assert!(text.contains("\"interaction_success\": true"));
    }

    #[test]
    fn fake_available_probe_writes_failure_evidence() {
        let dir = tempfile::tempdir().unwrap();
        write_test_availability_override(dir.path(), true);
        write_test_result_override(
            dir.path(),
            &json!({
                "ok": false,
                "status": "failed",
                "steps": ["surface_visible", "control_input_dispatched"],
                "before_marker": "same",
                "after_marker": "same",
                "failure_kind": "start_transition_missing",
                "duration_ms": 21
            }),
        );
        let path = dir.path().join(".anvil/runs/test/browser-interaction.json");
        let outcome = probe_browser_interaction_against_running_server(
            dir.path(),
            34001,
            path.parent().unwrap(),
            &path,
            Duration::from_secs(1),
        );
        let observation = outcome.observation().expect("observation");
        assert!(!observation.ok, "{observation:?}");
        assert_eq!(observation.failure_kind, "start_transition_missing");
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("\"ok\": false"));
        assert!(text.contains("start_transition_missing"));
    }

    fn observe_probe_value(value: Value) -> BrowserInteractionObservation {
        let dir = tempfile::tempdir().unwrap();
        observation_from_value(
            &dir.path().join("browser-interaction.json"),
            &dir.path().join("browser-interaction-probe.cjs"),
            "http://127.0.0.1:34001/",
            value,
            Instant::now(),
            String::new(),
            String::new(),
            String::new(),
            String::new(),
            false,
            true,
            None,
        )
    }

    #[test]
    fn taxonomy_no_control_transition_is_start_transition_missing() {
        let observation = observe_probe_value(json!({
            "ok": false,
            "status": "failed",
            "steps": ["surface_visible", "control_input_dispatched"],
            "state_changed": false,
            "failure_kind": "start_transition_missing"
        }));

        assert!(!observation.ok);
        assert_eq!(observation.failure_kind, "start_transition_missing");
    }

    #[test]
    fn taxonomy_input_missing_after_start_is_split_from_start_failure() {
        let observation = observe_probe_value(json!({
            "ok": false,
            "status": "failed",
            "start_transition": true,
            "input_state_evaluated_after_start": true,
            "input_state_change": false,
            "state_changed": false,
            "steps": [
                "surface_visible",
                "start_transition",
                "control_input_dispatched",
                "input_state_evaluated_after_start"
            ],
            "failure_kind": "start_transition_missing"
        }));

        assert!(!observation.ok);
        assert_eq!(
            observation.failure_kind,
            "input_state_change_missing_after_start"
        );
    }

    #[test]
    fn taxonomy_input_not_evaluated_after_start_is_distinct() {
        let observation = observe_probe_value(json!({
            "ok": false,
            "status": "failed",
            "start_transition": true,
            "input_state_evaluated_after_start": false,
            "steps": ["surface_visible", "start_transition"],
            "failure_kind": "browser_interaction_failed"
        }));

        assert!(!observation.ok);
        assert_eq!(
            observation.failure_kind,
            "input_state_change_not_evaluated_after_start"
        );
    }

    #[test]
    fn taxonomy_startless_input_state_change_passes_generic_interaction() {
        let observation = observe_probe_value(json!({
            "ok": true,
            "status": "passed",
            "start_transition": false,
            "start_control_found": false,
            "input_state_change": true,
            "state_changed": true,
            "surface_visible": true,
            "steps": [
                "surface_visible",
                "control_input_dispatched",
                "input_state_change"
            ],
            "input_dispatches": ["direct text input"],
            "input_before_marker": "draft:",
            "input_after_marker": "draft:anvil probe input"
        }));

        assert!(observation.ok, "{observation:?}");
        assert!(!observation.start_control_found);
        assert!(observation.input_state_changed);
        assert!(
            !observation
                .steps
                .iter()
                .any(|step| step == "start_transition")
        );
        assert_eq!(observation.failure_kind, "");
    }

    #[test]
    fn contract_mode_pass_uses_state_json_markers() {
        let observation = observe_probe_value(json!({
            "ok": true,
            "status": "passed",
            "probe_mode": "contract",
            "contract_hook_status": "usable",
            "start_transition": true,
            "input_state_evaluated_after_start": true,
            "input_state_change": true,
            "state_changed": true,
            "steps": [
                "surface_visible",
                "start_transition",
                "control_input_dispatched",
                "input_state_evaluated_after_start",
                "input_state_change"
            ],
            "before_marker": "{\"states\":[{\"state\":{\"screen\":\"menu\"}}]}",
            "after_marker": "{\"states\":[{\"state\":{\"screen\":\"running\"}}]}",
            "input_before_marker": "{\"states\":[{\"state\":{\"player\":20}}]}",
            "input_after_marker": "{\"states\":[{\"state\":{\"player\":15}}]}",
            "state_dimensions_changed": ["player"],
            "contract_hooks": {
                "usable": true,
                "primary_present": true,
                "restart_present": true,
                "valid_state_count": 1
            }
        }));

        assert!(observation.ok, "{observation:?}");
        assert_eq!(observation.probe_mode, "contract");
        assert_eq!(observation.contract_hook_status, "usable");
        assert!(observation.input_state_changed);
        assert_eq!(observation.state_dimensions_changed, vec!["player"]);
    }

    #[test]
    fn heuristic_hook_absent_fallback_records_candidate_table_and_primary_info() {
        let observation = observe_probe_value(json!({
            "ok": true,
            "status": "passed",
            "probe_mode": "heuristic",
            "contract_hook_status": "primary_missing",
            "start_transition": true,
            "primary_start_transition": false,
            "input_state_evaluated_after_start": true,
            "input_state_change": true,
            "state_changed": true,
            "candidate_table": [
                {"rank": 1, "index": 0, "text_excerpt": "", "changed": false},
                {"rank": 2, "index": 1, "text_excerpt": "Start", "changed": true}
            ],
            "input_dispatches": [
                "canvas/center click",
                "ArrowLeft keydown",
                "ArrowRight keydown",
                "Space keydown"
            ],
            "informational_failure_kinds": ["primary_start_transition_missing"],
            "steps": [
                "surface_visible",
                "start_transition",
                "control_input_dispatched",
                "input_state_evaluated_after_start",
                "input_state_change"
            ]
        }));

        assert!(observation.ok, "{observation:?}");
        assert_eq!(observation.probe_mode, "heuristic");
        assert_eq!(observation.contract_hook_status, "primary_missing");
        assert_eq!(observation.candidate_table.len(), 2);
        assert!(!observation.candidate_table[0].changed);
        assert!(observation.candidate_table[1].changed);
        assert_eq!(
            observation.informational_failure_kinds,
            vec!["primary_start_transition_missing".to_string()]
        );
    }

    #[test]
    fn todo_shaped_contract_fixture_passes_same_interaction_contract() {
        let observation = observe_probe_value(json!({
            "ok": true,
            "status": "passed",
            "probe_mode": "contract",
            "contract_hook_status": "usable",
            "start_transition": true,
            "input_state_evaluated_after_start": true,
            "input_state_change": true,
            "state_changed": true,
            "steps": [
                "surface_visible",
                "start_transition",
                "control_input_dispatched",
                "input_state_evaluated_after_start",
                "input_state_change"
            ],
            "before_marker": "{\"states\":[{\"state\":{\"todos\":0,\"filter\":\"all\"}}]}",
            "after_marker": "{\"states\":[{\"state\":{\"todos\":1,\"filter\":\"all\"}}]}",
            "input_before_marker": "{\"states\":[{\"state\":{\"todos\":1,\"filter\":\"all\"}}]}",
            "input_after_marker": "{\"states\":[{\"state\":{\"todos\":1,\"filter\":\"active\"}}]}"
        }));

        assert!(observation.ok, "{observation:?}");
        assert_eq!(observation.probe_mode, "contract");
        assert!(observation.input_state_changed);
    }

    #[test]
    #[cfg(unix)]
    fn setup_interaction_probe_provisions_managed_dir_and_is_idempotent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace");
        let home = dir.path().join("home");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let tool_dir = managed_interaction_probe_tool_dir(&home);
        let managed_node_modules = tool_dir.join("node_modules");
        let capture = dir.path().join("commands.log");

        let fake_npm = dir.path().join("fake-npm.sh");
        std::fs::write(
            &fake_npm,
            format!(
                r#"#!/bin/sh
echo "npm|$(pwd)|$*" >> "{capture}"
if [ "$1" = "install" ] && [ "$2" = "playwright" ]; then
  mkdir -p node_modules/playwright
  printf 'module.exports = {{}};\n' > node_modules/playwright/index.js
  printf '{{"version":"29.1.0"}}\n' > node_modules/playwright/package.json
  exit 0
fi
if [ "$1" = "root" ] && [ "$2" = "-g" ]; then
  echo "/not/used/global/node_modules"
  exit 0
fi
exit 2
"#,
                capture = capture.display(),
            ),
        )
        .unwrap();
        let fake_npx = dir.path().join("fake-npx.sh");
        std::fs::write(
            &fake_npx,
            format!(
                r#"#!/bin/sh
echo "npx|$(pwd)|$*" >> "{capture}"
if [ "$1" = "playwright" ] && [ "$2" = "install" ] && [ "$3" = "chromium" ]; then
  printf 'chromium\n' > chromium-installed
  exit 0
fi
exit 2
"#,
                capture = capture.display(),
            ),
        )
        .unwrap();
        let fake_node = dir.path().join("fake-node.sh");
        std::fs::write(
            &fake_node,
            format!(
                r#"#!/bin/sh
echo "node|$(pwd)|NODE_PATH=${{NODE_PATH}}|$*" >> "{capture}"
if [ "$1" = "-e" ] && [ -f "${{NODE_PATH}}/playwright/index.js" ]; then
  echo '{{"path":"'{managed}'/playwright/index.js","version":"29.1.0"}}'
  exit 0
fi
exit 1
"#,
                capture = capture.display(),
                managed = managed_node_modules.display(),
            ),
        )
        .unwrap();
        for path in [&fake_npm, &fake_npx, &fake_node] {
            let mut permissions = std::fs::metadata(path).unwrap().permissions();
            permissions.set_mode(0o755);
            std::fs::set_permissions(path, permissions).unwrap();
        }

        let mut progress = Vec::new();
        let report = setup_interaction_probe_with_programs(
            &root,
            fake_node.as_os_str(),
            fake_npm.as_os_str(),
            fake_npx.as_os_str(),
            &home,
            Duration::from_secs(5),
            |line| progress.push(line.to_string()),
        )
        .unwrap();

        assert!(report.installed, "{report:?}");
        assert_eq!(report.resolution.location, "managed_interaction_probe");
        assert_eq!(report.resolution.version, "29.1.0");
        assert!(tool_dir.join("package.json").is_file());
        assert!(tool_dir.join("chromium-installed").is_file());
        let canonical_tool_dir = std::fs::canonicalize(&tool_dir).unwrap();
        let capture_text = std::fs::read_to_string(&capture).unwrap();
        assert!(
            capture_text.contains(&format!(
                "npm|{}|install playwright",
                canonical_tool_dir.display()
            )),
            "{capture_text}"
        );
        assert!(
            capture_text.contains(&format!(
                "npx|{}|playwright install chromium",
                canonical_tool_dir.display()
            )),
            "{capture_text}"
        );
        assert!(
            progress
                .iter()
                .any(|line| line.contains("npm install playwright")),
            "{progress:?}"
        );

        let before_second_run = std::fs::read_to_string(&capture).unwrap();
        let second = setup_interaction_probe_with_programs(
            &root,
            fake_node.as_os_str(),
            fake_npm.as_os_str(),
            fake_npx.as_os_str(),
            &home,
            Duration::from_secs(5),
            |_| {},
        )
        .unwrap();
        assert!(!second.installed, "{second:?}");
        let after_second_run = std::fs::read_to_string(&capture).unwrap();
        assert_eq!(
            before_second_run.matches("npm|").count(),
            after_second_run.matches("npm|").count(),
            "{after_second_run}"
        );
        assert_eq!(
            before_second_run.matches("npx|").count(),
            after_second_run.matches("npx|").count(),
            "{after_second_run}"
        );

        let availability = playwright_availability_from_programs_with_home(
            &root,
            fake_node.as_os_str(),
            fake_npm.as_os_str(),
            None,
            Some(&home),
        );
        let ProbeAvailability::Available(resolution) = availability else {
            panic!("expected managed resolver after setup");
        };
        assert_eq!(resolution.location, "managed_interaction_probe");
        assert_eq!(
            resolution.node_path.as_deref(),
            Some(managed_node_modules.to_string_lossy().as_ref())
        );
    }

    #[test]
    fn failure_observation_preserves_latest_probe_stage_marker() {
        let dir = tempfile::tempdir().unwrap();
        let evidence_path = dir.path().join("browser-interaction.json");
        let script_path = dir.path().join("browser-interaction-probe.cjs");

        let observation = failure_observation(
            dir.path(),
            &evidence_path,
            &script_path,
            34001,
            Instant::now(),
            "probe_command_failed",
            "{\"stage\":\"resolving\"}\n{\"stage\":\"navigating\"}",
            "",
            "",
            "{\"stage\":\"resolving\"}\n{\"stage\":\"navigating\"}",
            "",
            true,
            true,
            None,
        );

        assert_eq!(observation.stage, "navigating");
        assert_eq!(
            observation.failure_kind,
            "probe_infrastructure_failed:probe_command_failed"
        );
    }

    #[test]
    #[cfg(unix)]
    fn script_exit_failure_merges_stdout_json_error_and_stderr_tail() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let fake_node = dir.path().join("fake-node.sh");
        std::fs::write(
            &fake_node,
            r#"#!/bin/sh
echo '{"stage":"resolving"}' >&2
echo '{"stage":"launching"}' >&2
echo '{"stage":"navigating"}' >&2
for i in $(seq 1 25); do echo "stderr line $i" >&2; done
printf '%s\n' '{"ok":false,"status":"failed","stage":"navigating","failure_kind":"probe_script_error","error":"page.goto: net::ERR_CONNECTION_REFUSED at http://127.0.0.1:34001/","before_marker":"menu","after_marker":"menu","duration_ms":13}'
exit 1
"#,
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_node).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_node, permissions).unwrap();

        write_test_availability_override(dir.path(), true);
        write_test_node_program_override(dir.path(), &fake_node);
        let run_dir = dir.path().join(".anvil/runs/stdout-json");
        let path = run_dir.join("browser-interaction.json");
        let outcome = probe_browser_interaction_against_running_server(
            dir.path(),
            34001,
            &run_dir,
            &path,
            Duration::from_secs(3),
        );
        let observation = outcome.observation().expect("observation");

        assert!(!observation.ok, "{observation:?}");
        assert!(
            observation.error.contains("net::ERR_CONNECTION_REFUSED"),
            "{observation:?}"
        );
        assert_eq!(observation.stage, "navigating");
        assert_eq!(observation.before_marker, "menu");
        let text = std::fs::read_to_string(&path).unwrap();
        let value = serde_json::from_str::<Value>(&text).unwrap();
        let stderr_excerpt = value
            .get("stderr_excerpt")
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(text.contains("net::ERR_CONNECTION_REFUSED"), "{text}");
        assert!(text.contains("\"stderr_excerpt\""), "{text}");
        assert!(stderr_excerpt.starts_with("stderr line 6"), "{text}");
        assert!(stderr_excerpt.contains("stderr line 25"), "{text}");
    }

    #[test]
    #[cfg(unix)]
    fn resolved_node_path_is_reused_by_probe_child() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let tool_dir = dir.path().join("tools-node-path");
        std::fs::create_dir_all(&tool_dir).unwrap();
        let capture = dir.path().join("node-env.log");
        let fake_node = dir.path().join("fake-node.sh");
        std::fs::write(
            &fake_node,
            format!(
                r#"#!/bin/sh
echo "NODE_PATH=${{NODE_PATH}}" >> "{capture}"
if [ "$1" = "-e" ]; then
  if [ "${{NODE_PATH}}" = "{tool_dir}" ]; then
    echo "{tool_dir}/playwright/index.js"
    exit 0
  fi
  exit 1
fi
cat > "$3" <<'JSON'
{{"ok":true,"status":"passed","interaction_success":true,"interaction_performed":true,"input_event_observed":true,"start_transition":true,"input_state_evaluated_after_start":true,"state_changed":true,"stage":"observing","steps":["surface_visible","start_transition","control_input_dispatched","input_state_evaluated_after_start","input_state_change"],"before_marker":"menu","after_marker":"running"}}
JSON
exit 0
"#,
                capture = capture.display(),
                tool_dir = tool_dir.display(),
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&fake_node).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&fake_node, permissions).unwrap();

        let availability = playwright_availability_from_programs(
            dir.path(),
            fake_node.as_os_str(),
            OsStr::new("/no/such/npm"),
            Some(tool_dir.clone()),
        );
        let ProbeAvailability::Available(resolution) = availability else {
            panic!("expected fake resolver to find playwright");
        };
        assert_eq!(
            resolution.node_path.as_deref(),
            Some(tool_dir.to_string_lossy().as_ref())
        );
        write_test_node_program_override(dir.path(), &fake_node);
        write_test_availability_override_with_resolution(dir.path(), true, Some(&resolution));

        let run_dir = dir.path().join(".anvil/runs/test");
        let path = run_dir.join("browser-interaction.json");
        let outcome = probe_browser_interaction_against_running_server(
            dir.path(),
            34001,
            &run_dir,
            &path,
            Duration::from_secs(1),
        );

        assert!(
            outcome
                .observation()
                .is_some_and(|observation| observation.ok)
        );
        let capture_text = std::fs::read_to_string(capture).unwrap();
        assert!(
            capture_text
                .lines()
                .any(|line| line == format!("NODE_PATH={}", tool_dir.display())),
            "{capture_text}"
        );
    }
}
