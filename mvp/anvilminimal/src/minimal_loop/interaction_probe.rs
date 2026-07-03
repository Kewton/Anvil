use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::eval_events;
use crate::minimal_loop::verifier_env;

#[cfg(not(test))]
const AVAILABILITY_TIMEOUT: Duration = Duration::from_secs(10);
const INTERACTION_TIMEOUT: Duration = Duration::from_secs(20);
const PROBE_SCRIPT_NAME: &str = "browser-interaction-probe.cjs";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeAvailability {
    Available,
    Unavailable(String),
}

impl ProbeAvailability {
    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            Self::Available => None,
            Self::Unavailable(reason) => Some(reason),
        }
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
    pub failure_kind: String,
    pub duration_ms: u128,
    pub output_excerpt: String,
    pub child_spawned: bool,
    pub child_reaped: bool,
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
    {
        playwright_availability_from_command(root)
    }
}

#[cfg(not(test))]
fn playwright_availability_from_command(root: &Path) -> ProbeAvailability {
    let mut command = verifier_env::normalized_command_at_root("npx", root);
    command
        .args(["--no-install", "playwright", "--version"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return ProbeAvailability::Unavailable("playwright_not_installed".to_string()),
    };
    let deadline = Instant::now() + AVAILABILITY_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    ProbeAvailability::Available
                } else {
                    ProbeAvailability::Unavailable("playwright_not_installed".to_string())
                };
            }
            Ok(None) => {}
            Err(_) => {
                terminate_child_group(&mut child);
                let _ = child.wait();
                return ProbeAvailability::Unavailable("playwright_not_installed".to_string());
            }
        }
        if Instant::now() >= deadline {
            terminate_child_group(&mut child);
            let _ = child.wait();
            return ProbeAvailability::Unavailable("playwright_not_installed".to_string());
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
    if let ProbeAvailability::Unavailable(reason) = playwright_availability(root) {
        return InteractionProbeOutcome::Unavailable(reason);
    }
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
            );
            return InteractionProbeOutcome::Observation(Box::new(observation));
        }
    };

    let url = format!("http://127.0.0.1:{port}/");
    let mut command = verifier_env::normalized_command_at_root("node", root);
    command
        .arg(&script_path)
        .arg(&url)
        .arg(evidence_path)
        .current_dir(root)
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
const { chromium } = require("playwright");

const url = process.argv[2];
const outputPath = process.argv[3];
const started = Date.now();
const steps = [];
let before_marker = "";
let after_marker = "";

function write(value) {
  fs.mkdirSync(require("path").dirname(outputPath), { recursive: true });
  fs.writeFileSync(outputPath, JSON.stringify(value, null, 2) + "\n");
}

async function marker(page) {
  return await page.evaluate(() => {
    const buttons = Array.from(document.querySelectorAll("button,[role=button]"))
      .map((el) => (el.textContent || "").trim())
      .join("|");
    const body = (document.body && document.body.innerText ? document.body.innerText : "")
      .replace(/\s+/g, " ")
      .slice(0, 800);
    const element_count = document.querySelectorAll("*").length;
    return JSON.stringify({ buttons, body, element_count });
  });
}

(async () => {
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    const page = await browser.newPage();
    await page.goto(url, { waitUntil: "domcontentloaded", timeout: 15000 });
    const surface = page.locator("canvas, button, [role=button]").first();
    await surface.waitFor({ timeout: 10000 });
    steps.push("surface_visible");
    before_marker = await marker(page);

    const startControl = page.locator("button, [role=button]").first();
    if (await startControl.count()) {
      await startControl.click({ timeout: 5000 });
    }
    after_marker = await marker(page);
    if (before_marker !== after_marker) {
      steps.push("start_transition");
    }

    const canvas = page.locator("canvas").first();
    if (await canvas.count()) {
      const box = await canvas.boundingBox();
      if (box) {
        await page.mouse.click(box.x + box.width / 2, box.y + box.height / 2);
      }
    }
    await page.keyboard.press("ArrowLeft");
    await page.keyboard.press("Space");
    steps.push("control_input_dispatched");

    const ok = steps.includes("surface_visible") && steps.includes("start_transition");
    write({
      ok,
      status: ok ? "passed" : "failed",
      interaction_success: ok,
      interaction_performed: ok,
      input_event_observed: steps.includes("control_input_dispatched"),
      state_changed: ok,
      visible_state_changed: ok,
      steps,
      before_marker,
      after_marker,
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
      before_marker,
      after_marker,
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
) -> BrowserInteractionObservation {
    let ok = value.get("ok").and_then(Value::as_bool).unwrap_or(false);
    let failure_kind = value
        .get("failure_kind")
        .or_else(|| value.get("browser_failure_kind"))
        .and_then(Value::as_str)
        .unwrap_or(if ok { "" } else { "browser_interaction_failed" })
        .to_string();
    let duration_ms = value
        .get("duration_ms")
        .and_then(Value::as_u64)
        .map(u128::from)
        .unwrap_or_else(|| started.elapsed().as_millis());
    BrowserInteractionObservation {
        ok,
        status: if ok { "passed" } else { "failed" }.to_string(),
        url: url.to_string(),
        evidence_path: evidence_path.to_path_buf(),
        script_path: script_path.to_path_buf(),
        steps: value
            .get("steps")
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default(),
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
        failure_kind,
        duration_ms,
        output_excerpt: eval_events::body_snippet(&output_excerpt),
        child_spawned,
        child_reaped,
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
) -> BrowserInteractionObservation {
    let url = format!("http://127.0.0.1:{port}/");
    let observation = BrowserInteractionObservation {
        ok: false,
        status: "failed".to_string(),
        url: url.clone(),
        evidence_path: evidence_path.to_path_buf(),
        script_path: script_path.to_path_buf(),
        steps: Vec::new(),
        before_marker: String::new(),
        after_marker: String::new(),
        failure_kind: failure_kind.to_string(),
        duration_ms: started.elapsed().as_millis(),
        output_excerpt: eval_events::body_snippet(output_excerpt),
        child_spawned,
        child_reaped,
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

fn interaction_observation_json(observation: &BrowserInteractionObservation) -> Value {
    let mut value = json!({
        "ok": observation.ok,
        "status": observation.status,
        "url": observation.url,
        "interaction_success": observation.ok,
        "interaction_performed": observation.ok,
        "input_event_observed": observation.steps.iter().any(|step| step == "control_input_dispatched"),
        "state_changed": observation.ok,
        "visible_state_changed": observation.ok,
        "steps": observation.steps,
        "before_marker": observation.before_marker,
        "after_marker": observation.after_marker,
        "duration_ms": observation.duration_ms,
        "probe": {
            "script_path": observation.script_path.display().to_string(),
            "output_excerpt": observation.output_excerpt,
            "child_spawned": observation.child_spawned,
            "child_reaped": observation.child_reaped,
        }
    });
    if !observation.failure_kind.is_empty() {
        value["failure_kind"] = json!(observation.failure_kind);
        value["browser_failure_kind"] = json!(observation.failure_kind);
    }
    if !observation.output_excerpt.is_empty() {
        value["output_excerpt"] = json!(observation.output_excerpt);
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
        Some(ProbeAvailability::Available)
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
pub fn write_test_availability_override(root: &Path, available: bool) {
    let path = root
        .join(".anvil")
        .join("evidence")
        .join("interaction-probe-availability.json");
    write_text(
        &path,
        &format!(
            "{}\n",
            serde_json::to_string_pretty(&json!({
                "available": available,
                "reason": if available { "" } else { "playwright_not_installed" },
            }))
            .unwrap()
        ),
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
}
