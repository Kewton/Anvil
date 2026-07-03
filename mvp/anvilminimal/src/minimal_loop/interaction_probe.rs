use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Context, bail};
use serde::Serialize;
use serde_json::{Value, json};

use crate::eval_events;
use crate::minimal_loop::verifier_env;

const AVAILABILITY_TIMEOUT: Duration = Duration::from_secs(10);
const INTERACTION_TIMEOUT: Duration = Duration::from_secs(20);
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
pub struct BrowserInteractionObservation {
    pub ok: bool,
    pub status: String,
    pub url: String,
    pub evidence_path: PathBuf,
    pub script_path: PathBuf,
    pub steps: Vec<String>,
    pub before_marker: String,
    pub after_marker: String,
    pub input_state_changed: bool,
    pub recovery_transition_observed: bool,
    pub recovery_transition_not_observed: bool,
    pub failure_kind: String,
    pub stage: String,
    pub remediation: String,
    pub duration_ms: u128,
    pub output_excerpt: String,
    pub child_spawned: bool,
    pub child_reaped: bool,
    pub playwright_resolution: Option<PlaywrightResolution>,
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
        return ProbeAvailability::Unavailable("playwright_not_installed".to_string());
    }
    #[cfg(not(test))]
    return playwright_availability_from_command(root);
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
                let output_excerpt = interaction_stdio_excerpt(run_dir);
                if status.success() {
                    let value = read_interaction_value(evidence_path).unwrap_or_else(|| {
                        interaction_failure_json(
                            &url,
                            "probe_evidence_missing",
                            &output_excerpt,
                            started.elapsed().as_millis(),
                        )
                    });
                    let observation = observation_from_value(
                        evidence_path,
                        &script_path,
                        &url,
                        value,
                        started,
                        output_excerpt,
                        true,
                        reaped,
                        Some(resolution.clone()),
                    );
                    mirror_interaction_observation(root, evidence_path, &observation);
                    return InteractionProbeOutcome::Observation(Box::new(observation));
                }
                if let Some(value) = read_interaction_value(evidence_path) {
                    let observation = observation_from_value(
                        evidence_path,
                        &script_path,
                        &url,
                        value,
                        started,
                        output_excerpt,
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
                    &output_excerpt,
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
            let output_excerpt = interaction_stdio_excerpt(run_dir);
            let observation = failure_observation(
                root,
                evidence_path,
                &script_path,
                port,
                started,
                "probe_timeout",
                &output_excerpt,
                true,
                reaped,
                Some(resolution.clone()),
            );
            return InteractionProbeOutcome::Observation(Box::new(observation));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn write_probe_script(path: &Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, interaction_probe_script())
}

fn interaction_probe_script() -> &'static str {
    r#"const fs = require("fs");

const url = process.argv[2];
const outputPath = process.argv[3];
const started = Date.now();
const steps = [];
let stage = "resolving";
let before_marker = "";
let after_marker = "";
let input_before_marker = "";
let input_after_marker = "";
let recovery_before_marker = "";
let recovery_after_marker = "";
let recovery_transition_status = "unknown";

function write(value) {
  fs.mkdirSync(require("path").dirname(outputPath), { recursive: true });
  fs.writeFileSync(outputPath, JSON.stringify(value, null, 2) + "\n");
}

function mark(nextStage) {
  stage = nextStage;
  try {
    process.stderr.write(JSON.stringify({ stage }) + "\n");
  } catch (_) {}
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

async function attemptRecoveryTransition(page, initialStartText) {
  const deadline = Date.now() + 1200;
  while (Date.now() < deadline) {
    const index = await recoveryCandidateIndex(page, initialStartText);
    if (index >= 0) {
      const candidate = page.locator("button,[role=button]").nth(index);
      recovery_before_marker = await marker(page);
      try {
        await candidate.click({ timeout: 1000 });
        recovery_after_marker = await markerAfterChange(page, recovery_before_marker, 700);
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

(async () => {
  let browser;
  try {
    mark("resolving");
    const { chromium } = require("playwright");
    mark("launching");
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage();
    mark("navigating");
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: 15000 });
    const surface = page.locator("canvas, button, [role=button]").first();
    await surface.waitFor({ timeout: 10000 });
    steps.push("surface_visible");
    mark("observing");
    before_marker = await marker(page);

    const startControl = page.locator("button, [role=button]").first();
    let initial_start_text = "";
    if (await startControl.count()) {
      initial_start_text = await controlText(startControl);
      await startControl.click({ timeout: 5000 });
    }
    after_marker = await markerAfterChange(page, before_marker, 800);
    if (before_marker !== after_marker) {
      steps.push("start_transition");
    }

    input_before_marker = await marker(page);
    const canvas = page.locator("canvas").first();
    if (await canvas.count()) {
      const box = await canvas.boundingBox();
      if (box) {
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
      }
    }
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Space");
    steps.push("control_input_dispatched");
    input_after_marker = await markerAfterChange(page, input_before_marker, 800);
    if (input_before_marker !== input_after_marker) {
      steps.push("input_state_change");
    }

    await attemptRecoveryTransition(page, initial_start_text);

    const ok = steps.includes("surface_visible") && steps.includes("start_transition");
    const inputStateChanged = steps.includes("input_state_change");
    const recoveryObserved = steps.includes("recovery_transition");
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
      steps,
      stage,
      before_marker,
      after_marker,
      input_before_marker,
      input_after_marker,
      recovery_before_marker,
      recovery_after_marker,
      failure_kind: ok ? "" : "start_transition_missing",
      duration_ms: Date.now() - started
    });
    await browser.close();
    process.exit(ok ? 0 : 1);
  } catch (err) {
    if (browser) {
      try { await browser.close(); } catch (_) {}
    }
    write({
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
      failure_kind: "probe_script_error",
      error: err && err.message ? err.message : String(err),
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

fn interaction_stdio_excerpt(run_dir: &Path) -> String {
    let stdout =
        std::fs::read_to_string(run_dir.join("browser-interaction.out")).unwrap_or_default();
    let stderr =
        std::fs::read_to_string(run_dir.join("browser-interaction.err")).unwrap_or_default();
    eval_events::body_snippet(format!("{stdout}\n{stderr}").trim())
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
    child_spawned: bool,
    child_reaped: bool,
    playwright_resolution: Option<PlaywrightResolution>,
) -> BrowserInteractionObservation {
    let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let raw_failure_kind = value
        .get("failure_kind")
        .or_else(|| value.get("browser_failure_kind"))
        .and_then(Value::as_str)
        .unwrap_or(if ok { "" } else { "browser_interaction_failed" })
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
    let failure_kind = normalized_interaction_failure_kind(
        ok,
        &stage,
        &raw_failure_kind,
        value_error,
        &output_excerpt,
    );
    let remediation = interaction_failure_remediation(&failure_kind);
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
    let input_event_observed = value
        .get("input_event_observed")
        .and_then(Value::as_bool)
        .unwrap_or_else(|| steps.iter().any(|step| step == "control_input_dispatched"));
    let explicit_state_changed = value
        .get("input_state_change")
        .or_else(|| value.get("state_changed"))
        .or_else(|| value.get("visible_state_changed"))
        .and_then(Value::as_bool);
    let input_state_changed = steps.iter().any(|step| step == "input_state_change")
        || explicit_state_changed == Some(true)
        || (ok && input_event_observed && explicit_state_changed.is_none());
    let recovery_transition_observed = steps.iter().any(|step| step == "recovery_transition")
        || value
            .get("recovery_transition")
            .and_then(Value::as_bool)
            .unwrap_or(false);
    let recovery_transition_not_observed = steps
        .iter()
        .any(|step| step == "recovery_transition:not_observed")
        || value.get("recovery_transition").and_then(Value::as_bool) == Some(false)
        || value
            .get("recovery_transition_status")
            .and_then(Value::as_str)
            == Some("not_observed");
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
        input_state_changed,
        recovery_transition_observed,
        recovery_transition_not_observed,
        failure_kind,
        stage,
        remediation,
        duration_ms,
        output_excerpt: eval_events::body_snippet(&output_excerpt),
        child_spawned,
        child_reaped,
        playwright_resolution,
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
    child_spawned: bool,
    child_reaped: bool,
    playwright_resolution: Option<PlaywrightResolution>,
) -> BrowserInteractionObservation {
    let url = format!("http://127.0.0.1:{port}/");
    let stage = stage_from_probe_output(output_excerpt).unwrap_or_default();
    let failure_kind = normalized_interaction_failure_kind(
        false,
        &stage,
        failure_kind,
        output_excerpt,
        output_excerpt,
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
        input_state_changed: false,
        recovery_transition_observed: false,
        recovery_transition_not_observed: false,
        failure_kind,
        stage,
        remediation,
        duration_ms: started.elapsed().as_millis(),
        output_excerpt: eval_events::body_snippet(output_excerpt),
        child_spawned,
        child_reaped,
        playwright_resolution,
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
    if failure_kind.starts_with("probe_dependency_missing")
        || failure_kind.starts_with("probe_infrastructure_failed")
        || probe_stage_before_observation(stage)
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
                "steps": ["surface_visible", "start_transition", "control_input_dispatched"],
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
            Duration::from_secs(2),
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
            Duration::from_secs(2),
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
{{"ok":true,"status":"passed","interaction_success":true,"interaction_performed":true,"input_event_observed":true,"state_changed":true,"stage":"observing","steps":["surface_visible","start_transition","control_input_dispatched","input_state_change"],"before_marker":"menu","after_marker":"running"}}
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
