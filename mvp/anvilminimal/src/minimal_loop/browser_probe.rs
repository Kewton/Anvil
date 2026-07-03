use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::{Value, json};

use crate::eval_events;
use crate::minimal_loop::build_verifier::{self, BuildVerifierStatus};
use crate::minimal_loop::dependency_setup::NodeDependencySetupAuthority;
use crate::minimal_loop::interaction_probe;
use crate::minimal_loop::verifier_env;

const DEFAULT_ROUTE: &str = "/";
const DEFAULT_NEXTJS_PORT: u16 = 3000;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const WALL_CLOCK_CAP: Duration = Duration::from_secs(60);
const POLL_INTERVAL: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_millis(500);
const MAX_HTTP_RESPONSE_BYTES: usize = 32_768;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct BrowserReadinessObservation {
    pub ok: bool,
    pub status: String,
    pub profile: String,
    pub port: u16,
    pub route: String,
    pub command: String,
    pub http_status: Option<i64>,
    pub failure_kind: String,
    pub evidence_path: PathBuf,
    pub elapsed_ms: u128,
    pub output_excerpt: String,
    pub child_spawned: bool,
    pub child_reaped: bool,
}

impl BrowserReadinessObservation {
    pub fn failure_reason(&self) -> Option<String> {
        (!self.ok && !self.status.starts_with("skipped")).then(|| {
            if self.failure_kind.is_empty() {
                "browser_check_failed".to_string()
            } else {
                self.failure_kind.clone()
            }
        })
    }
}

#[derive(Debug, Clone)]
struct ProbeCommand {
    program: String,
    args: Vec<String>,
    env: Vec<(String, String)>,
    display: String,
}

#[derive(Debug, Clone)]
struct ProbeSpec {
    command: ProbeCommand,
    port: u16,
    route: String,
}

#[derive(Debug, Clone)]
struct ProbeOptions {
    port: Option<u16>,
    timeout: Duration,
    offline: bool,
    require_build: bool,
    command_override: Option<ProbeCommand>,
}

pub fn probe_browser_readiness(
    root: &Path,
    profile: &str,
    port: Option<u16>,
    timeout: Duration,
) -> BrowserReadinessObservation {
    probe_browser_readiness_with_offline(root, profile, port, timeout, false)
}

pub fn probe_browser_readiness_with_offline(
    root: &Path,
    profile: &str,
    port: Option<u16>,
    timeout: Duration,
    offline: bool,
) -> BrowserReadinessObservation {
    #[cfg(not(test))]
    let options = ProbeOptions {
        port,
        timeout,
        offline,
        require_build: true,
        command_override: None,
    };
    #[cfg(test)]
    let mut options = ProbeOptions {
        port,
        timeout,
        offline,
        require_build: true,
        command_override: None,
    };
    #[cfg(test)]
    if let Some(override_command) = load_test_probe_command(root) {
        options.port = override_command.port.or(options.port);
        options.require_build = override_command.require_build;
        options.command_override = Some(override_command.command);
    }
    probe_browser_readiness_with_options(root, profile, options)
}

fn probe_browser_readiness_with_options(
    root: &Path,
    profile: &str,
    options: ProbeOptions,
) -> BrowserReadinessObservation {
    let evidence_path = browser_readiness_evidence_path(root);
    let started = Instant::now();
    let normalized_profile = profile.trim().to_ascii_lowercase();
    let timeout = normalized_timeout(options.timeout);
    let mut spec = resolve_probe_spec(root, options.port, options.command_override.as_ref());
    if !matches!(
        normalized_profile.as_str(),
        "nextjs" | "next-js" | "next.js"
    ) {
        return finish_without_spawn(
            root,
            started,
            &evidence_path,
            &mut spec,
            profile,
            "skipped_unsupported_profile",
            "skipped_unsupported_profile",
            "",
        );
    }
    if options.offline {
        return finish_without_spawn(
            root,
            started,
            &evidence_path,
            &mut spec,
            profile,
            "skipped_offline",
            "skipped_offline",
            "",
        );
    }
    if options.require_build {
        let build = observe_nextjs_build(root, options.offline);
        if build.final_status != BuildVerifierStatus::Passed {
            return finish_without_spawn(
                root,
                started,
                &evidence_path,
                &mut spec,
                profile,
                "failed",
                "build_verifier_failed",
                &build.final_reason,
            );
        }
    }
    if localhost_port_accepts_connection(spec.port) {
        return finish_without_spawn(
            root,
            started,
            &evidence_path,
            &mut spec,
            profile,
            "failed",
            "port_in_use",
            "",
        );
    }

    let mut command = verifier_env::normalized_command_at_root(&spec.command.program, root);
    command
        .args(&spec.command.args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("PORT", spec.port.to_string());
    for (key, value) in &spec.command.env {
        command.env(key, value);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let child = match command.spawn() {
        Ok(child) => child,
        Err(err) => {
            return finish_without_spawn(
                root,
                started,
                &evidence_path,
                &mut spec,
                profile,
                "failed",
                "start_spawn_failed",
                &err.to_string(),
            );
        }
    };
    let mut child = ChildGuard::new(child);
    let deadline = started + timeout;
    let mut last_error = String::new();
    while Instant::now() < deadline {
        if let Ok(Some(_status)) = child.try_wait() {
            let cleanup = child.finish();
            let output = if cleanup.output_excerpt.is_empty() {
                "start process exited before readiness".to_string()
            } else {
                cleanup.output_excerpt
            };
            return finish_with_cleanup(
                root,
                started,
                &evidence_path,
                &spec,
                profile,
                None,
                "start_exited",
                &output,
                true,
                cleanup.reaped,
            );
        }
        match http_get_local_route(spec.port, &spec.route) {
            Ok(response) => {
                let cleanup = child.finish();
                let output = first_non_empty(&cleanup.output_excerpt, &response.body_excerpt);
                if response.status == 200 {
                    let run_dir = evidence_path.parent().unwrap_or(root);
                    let interaction_path =
                        interaction_probe::browser_interaction_evidence_path(root);
                    let _ = interaction_probe::probe_browser_interaction_against_running_server(
                        root,
                        spec.port,
                        run_dir,
                        &interaction_path,
                        Duration::from_secs(20),
                    );
                    return finish_with_cleanup(
                        root,
                        started,
                        &evidence_path,
                        &spec,
                        profile,
                        Some(response.status),
                        "",
                        &output,
                        true,
                        cleanup.reaped,
                    );
                }
                return finish_with_cleanup(
                    root,
                    started,
                    &evidence_path,
                    &spec,
                    profile,
                    Some(response.status),
                    &format!("http_{}", response.status),
                    &output,
                    true,
                    cleanup.reaped,
                );
            }
            Err(err) => {
                last_error = err;
                std::thread::sleep(POLL_INTERVAL);
            }
        }
    }
    let cleanup = child.finish();
    let output = first_non_empty(&cleanup.output_excerpt, &last_error);
    finish_with_cleanup(
        root,
        started,
        &evidence_path,
        &spec,
        profile,
        None,
        "timeout",
        &output,
        true,
        cleanup.reaped,
    )
}

fn observe_nextjs_build(
    root: &Path,
    offline: bool,
) -> build_verifier::BuildVerifierLifecycleObservation {
    let requirement = build_verifier::requirement_from_deferred(
        "npm run build",
        Some("nextjs"),
        "browser readiness probe production build",
        "browser_probe",
        "required",
    )
    .expect("npm run build is a build verifier command");
    build_verifier::observe_requirement_lifecycle_with_offline(
        root,
        &requirement,
        NodeDependencySetupAuthority::None,
        offline,
    )
}

fn resolve_probe_spec(
    root: &Path,
    explicit_port: Option<u16>,
    command_override: Option<&ProbeCommand>,
) -> ProbeSpec {
    let manifest = read_package_json(root);
    let detected_port = explicit_port
        .or_else(|| manifest.as_ref().and_then(port_from_package_scripts))
        .unwrap_or(DEFAULT_NEXTJS_PORT);
    let command = command_override
        .cloned()
        .unwrap_or_else(|| start_command_for_package(manifest.as_ref(), detected_port));
    ProbeSpec {
        command,
        port: detected_port,
        route: DEFAULT_ROUTE.to_string(),
    }
}

fn read_package_json(root: &Path) -> Option<Value> {
    let text = std::fs::read_to_string(root.join("package.json")).ok()?;
    serde_json::from_str::<Value>(&text).ok()
}

fn port_from_package_scripts(package: &Value) -> Option<u16> {
    let scripts = package.get("scripts")?.as_object()?;
    scripts
        .get("start")
        .and_then(Value::as_str)
        .and_then(parse_script_port)
        .or_else(|| {
            scripts
                .get("dev")
                .and_then(Value::as_str)
                .and_then(parse_script_port)
        })
}

fn start_command_for_package(package: Option<&Value>, port: u16) -> ProbeCommand {
    let has_start = package
        .and_then(|value| value.get("scripts"))
        .and_then(Value::as_object)
        .and_then(|scripts| scripts.get("start"))
        .and_then(Value::as_str)
        .is_some_and(|script| !script.trim().is_empty());
    if has_start {
        return ProbeCommand {
            program: "npm".to_string(),
            args: vec!["run".to_string(), "start".to_string()],
            env: Vec::new(),
            display: "npm run start".to_string(),
        };
    }
    ProbeCommand {
        program: "npx".to_string(),
        args: vec![
            "--no-install".to_string(),
            "next".to_string(),
            "start".to_string(),
            "-p".to_string(),
            port.to_string(),
        ],
        env: Vec::new(),
        display: format!("npx --no-install next start -p {port}"),
    }
}

fn parse_script_port(script: &str) -> Option<u16> {
    let tokens = script.split_whitespace().collect::<Vec<_>>();
    for (index, token) in tokens.iter().enumerate() {
        if matches!(*token, "-p" | "--port")
            && let Some(raw) = tokens.get(index + 1)
            && let Ok(port) = raw.parse::<u16>()
        {
            return Some(port);
        }
        if let Some(raw) = token.strip_prefix("-p")
            && !raw.is_empty()
            && let Ok(port) = raw.parse::<u16>()
        {
            return Some(port);
        }
        if let Some(raw) = token.strip_prefix("--port=")
            && let Ok(port) = raw.parse::<u16>()
        {
            return Some(port);
        }
    }
    None
}

fn normalized_timeout(timeout: Duration) -> Duration {
    if timeout.is_zero() {
        DEFAULT_TIMEOUT
    } else {
        timeout.min(WALL_CLOCK_CAP)
    }
}

fn localhost_port_accepts_connection(port: u16) -> bool {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    TcpStream::connect_timeout(&addr, Duration::from_millis(150)).is_ok()
}

#[derive(Debug)]
struct HttpProbeResult {
    status: i64,
    body_excerpt: String,
}

fn http_get_local_route(port: u16, route: &str) -> Result<HttpProbeResult, String> {
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    let mut stream =
        TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT).map_err(|err| err.to_string())?;
    let _ = stream.set_read_timeout(Some(CONNECT_TIMEOUT));
    let _ = stream.set_write_timeout(Some(CONNECT_TIMEOUT));
    let path = if route.starts_with('/') {
        route.to_string()
    } else {
        format!("/{route}")
    };
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nUser-Agent: anvilminimal-browser-readiness-probe\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .map_err(|err| err.to_string())?;
    let mut response = Vec::new();
    let mut buffer = [0u8; 4096];
    loop {
        match stream.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => {
                response.extend_from_slice(&buffer[..n]);
                if response.len() >= MAX_HTTP_RESPONSE_BYTES {
                    break;
                }
            }
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                break;
            }
            Err(err) => return Err(err.to_string()),
        }
    }
    let response_text = String::from_utf8_lossy(&response).to_string();
    let status_line = response_text
        .lines()
        .next()
        .ok_or_else(|| "empty_http_response".to_string())?;
    let status = status_line
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| "http_status_missing".to_string())?
        .parse::<i64>()
        .map_err(|_| "http_status_invalid".to_string())?;
    let body = response_text
        .split_once("\r\n\r\n")
        .map(|(_, body)| body)
        .unwrap_or(&response_text);
    Ok(HttpProbeResult {
        status,
        body_excerpt: eval_events::body_snippet(body),
    })
}

#[allow(clippy::too_many_arguments)]
fn finish_without_spawn(
    root: &Path,
    started: Instant,
    evidence_path: &Path,
    spec: &mut ProbeSpec,
    profile: &str,
    status: &str,
    failure_kind: &str,
    output_excerpt: &str,
) -> BrowserReadinessObservation {
    spec.route = DEFAULT_ROUTE.to_string();
    let observation = BrowserReadinessObservation {
        ok: status == "ready",
        status: status.to_string(),
        profile: profile.to_string(),
        port: spec.port,
        route: spec.route.clone(),
        command: spec.command.display.clone(),
        http_status: None,
        failure_kind: failure_kind.to_string(),
        evidence_path: evidence_path.to_path_buf(),
        elapsed_ms: started.elapsed().as_millis(),
        output_excerpt: eval_events::body_snippet(output_excerpt),
        child_spawned: false,
        child_reaped: false,
    };
    write_browser_readiness_evidence(root, &observation);
    observation
}

#[allow(clippy::too_many_arguments)]
fn finish_with_cleanup(
    root: &Path,
    started: Instant,
    evidence_path: &Path,
    spec: &ProbeSpec,
    profile: &str,
    http_status: Option<i64>,
    failure_kind: &str,
    output_excerpt: &str,
    child_spawned: bool,
    child_reaped: bool,
) -> BrowserReadinessObservation {
    let ok = http_status == Some(200) && failure_kind.is_empty();
    let adjusted_failure_kind = browser_probe_failure_kind(failure_kind, output_excerpt);
    let adjusted_output_excerpt =
        browser_probe_output_excerpt(&adjusted_failure_kind, output_excerpt);
    let observation = BrowserReadinessObservation {
        ok,
        status: if ok { "ready" } else { "failed" }.to_string(),
        profile: profile.to_string(),
        port: spec.port,
        route: spec.route.clone(),
        command: spec.command.display.clone(),
        http_status,
        failure_kind: adjusted_failure_kind,
        evidence_path: evidence_path.to_path_buf(),
        elapsed_ms: started.elapsed().as_millis(),
        output_excerpt: eval_events::body_snippet(&adjusted_output_excerpt),
        child_spawned,
        child_reaped,
    };
    write_browser_readiness_evidence(root, &observation);
    observation
}

pub fn browser_readiness_evidence_path(root: &Path) -> PathBuf {
    root.join(".anvil")
        .join("evidence")
        .join("browser-readiness.json")
}

fn write_browser_readiness_evidence(root: &Path, observation: &BrowserReadinessObservation) {
    let path = browser_readiness_evidence_path(root);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let value = browser_readiness_evidence_json(observation);
    if let Ok(text) = serde_json::to_string_pretty(&value) {
        let _ = std::fs::write(&path, format!("{text}\n"));
    }
}

fn browser_readiness_evidence_json(observation: &BrowserReadinessObservation) -> Value {
    let mut value = json!({
        "status": observation.status,
        "route": observation.route,
        "route_rendered": observation.ok,
        "dev_server": {
            "profile": observation.profile,
            "port": observation.port,
            "route": observation.route,
            "command": observation.command,
            "elapsed_ms": observation.elapsed_ms,
            "output_excerpt": observation.output_excerpt,
            "child_spawned": observation.child_spawned,
            "child_reaped": observation.child_reaped,
        }
    });
    if observation.status != "skipped_offline"
        && observation.status != "skipped_unsupported_profile"
    {
        value["ok"] = json!(observation.ok);
    }
    if let Some(status) = observation.http_status {
        value["http_status"] = json!(status);
    }
    if !observation.failure_kind.is_empty() {
        value["failure_kind"] = json!(observation.failure_kind);
        value["browser_failure_kind"] = json!(observation.failure_kind);
    }
    if !observation.output_excerpt.is_empty() {
        value["output_excerpt"] = json!(observation.output_excerpt);
    }
    value
}

fn first_non_empty(primary: &str, fallback: &str) -> String {
    if primary.trim().is_empty() {
        fallback.to_string()
    } else {
        primary.to_string()
    }
}

fn browser_probe_failure_kind(failure_kind: &str, output_excerpt: &str) -> String {
    if !failure_kind.is_empty() && verifier_env::is_env_node_env_conflict_output(output_excerpt) {
        verifier_env::ENV_NODE_ENV_CONFLICT_KIND.to_string()
    } else {
        failure_kind.to_string()
    }
}

fn browser_probe_output_excerpt(failure_kind: &str, output_excerpt: &str) -> String {
    if failure_kind == verifier_env::ENV_NODE_ENV_CONFLICT_KIND {
        verifier_env::with_env_node_env_remediation(output_excerpt)
    } else {
        output_excerpt.to_string()
    }
}

#[derive(Debug)]
struct CleanupObservation {
    reaped: bool,
    output_excerpt: String,
}

struct ChildGuard {
    child: Option<Child>,
}

impl ChildGuard {
    fn new(child: Child) -> Self {
        Self { child: Some(child) }
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child
            .as_mut()
            .expect("child present until finish")
            .try_wait()
    }

    fn finish(mut self) -> CleanupObservation {
        let Some(mut child) = self.child.take() else {
            return CleanupObservation {
                reaped: true,
                output_excerpt: String::new(),
            };
        };
        terminate_child(&mut child);
        match child.wait_with_output() {
            Ok(output) => CleanupObservation {
                reaped: true,
                output_excerpt: output_excerpt(&output),
            },
            Err(err) => CleanupObservation {
                reaped: false,
                output_excerpt: err.to_string(),
            },
        }
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            terminate_child(child);
            let _ = child.wait();
        }
    }
}

fn terminate_child(child: &mut Child) {
    #[cfg(unix)]
    {
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(format!("-{}", child.id()))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
}

fn output_excerpt(output: &std::process::Output) -> String {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    eval_events::body_snippet(&format!("{stdout}\n{stderr}"))
}

#[cfg(test)]
#[derive(Debug)]
struct TestProbeCommandOverride {
    command: ProbeCommand,
    port: Option<u16>,
    require_build: bool,
}

#[cfg(test)]
fn load_test_probe_command(root: &Path) -> Option<TestProbeCommandOverride> {
    let path = root
        .join(".anvil")
        .join("evidence")
        .join("browser-probe-command.json");
    let text = std::fs::read_to_string(path).ok()?;
    let value = serde_json::from_str::<Value>(&text).ok()?;
    let program = value.get("program")?.as_str()?.to_string();
    let args = value
        .get("args")
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let env = value
        .get("env")
        .and_then(Value::as_object)
        .map(|values| {
            values
                .iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let display = value
        .get("display")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| {
            std::iter::once(program.as_str())
                .chain(args.iter().map(String::as_str))
                .collect::<Vec<_>>()
                .join(" ")
        });
    let port = value
        .get("port")
        .and_then(Value::as_u64)
        .and_then(|port| u16::try_from(port).ok());
    let require_build = value
        .get("require_build")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    Some(TestProbeCommandOverride {
        command: ProbeCommand {
            program,
            args,
            env,
            display,
        },
        port,
        require_build,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn child_that_responds_200_writes_evidence_and_is_reaped() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        let observation = probe_with_mock_child(dir.path(), port, "200", 0, Duration::from_secs(5));
        assert!(observation.ok, "{observation:?}");
        assert_eq!(observation.http_status, Some(200));
        assert!(observation.child_spawned);
        assert!(observation.child_reaped);
        let evidence =
            std::fs::read_to_string(browser_readiness_evidence_path(dir.path())).unwrap();
        assert!(evidence.contains("\"ok\": true"));
        assert!(evidence.contains("\"http_status\": 200"));
    }

    #[test]
    fn child_that_responds_500_reports_http_failure() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        let observation = probe_with_mock_child(dir.path(), port, "500", 0, Duration::from_secs(5));
        assert!(!observation.ok, "{observation:?}");
        assert_eq!(observation.http_status, Some(500));
        assert_eq!(observation.failure_kind, "http_500");
        let evidence =
            std::fs::read_to_string(browser_readiness_evidence_path(dir.path())).unwrap();
        assert!(evidence.contains("\"failure_kind\": \"http_500\""));
    }

    #[test]
    fn child_with_no_response_times_out_and_is_reaped() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        let observation =
            probe_with_mock_child(dir.path(), port, "hang", 0, Duration::from_millis(900));
        assert!(!observation.ok, "{observation:?}");
        assert_eq!(observation.failure_kind, "timeout");
        assert!(observation.child_spawned);
        assert!(observation.child_reaped);
    }

    #[test]
    fn prebound_port_reports_port_in_use_without_spawn() {
        let dir = tempfile::tempdir().unwrap();
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        let observation = probe_with_mock_child(dir.path(), port, "200", 0, Duration::from_secs(5));
        assert!(!observation.ok, "{observation:?}");
        assert_eq!(observation.failure_kind, "port_in_use");
        assert!(!observation.child_spawned);
    }

    #[test]
    fn mock_server_succeeds_with_parent_node_env_production() {
        let status = run_ignored_browser_probe_harness(
            "minimal_loop::browser_probe::tests::browser_probe_normalized_env_harness",
        );
        assert!(status.success(), "{status}");
    }

    #[test]
    #[ignore]
    fn browser_probe_normalized_env_harness() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        let observation =
            probe_with_mock_child(dir.path(), port, "env-sensitive", 0, Duration::from_secs(5));
        assert!(observation.ok, "{observation:?}");
        assert_eq!(observation.http_status, Some(200));
    }

    #[test]
    fn next_node_env_marker_with_contamination_reports_env_conflict() {
        let status = run_ignored_browser_probe_harness(
            "minimal_loop::browser_probe::tests::browser_probe_env_conflict_harness",
        );
        assert!(status.success(), "{status}");
    }

    #[test]
    #[ignore]
    fn browser_probe_env_conflict_harness() {
        let dir = tempfile::tempdir().unwrap();
        let port = free_port();
        let observation = probe_with_mock_child(
            dir.path(),
            port,
            "node-env-marker",
            0,
            Duration::from_secs(5),
        );
        assert!(!observation.ok, "{observation:?}");
        assert_eq!(
            observation.failure_kind,
            verifier_env::ENV_NODE_ENV_CONFLICT_KIND
        );
        assert!(
            observation
                .output_excerpt
                .contains(verifier_env::ENV_NODE_ENV_REMEDIATION),
            "{observation:?}"
        );
    }

    #[test]
    #[ignore]
    fn browser_probe_mock_server_child() {
        if std::env::var("ANVIL_BROWSER_PROBE_MOCK_CHILD")
            .ok()
            .as_deref()
            != Some("1")
        {
            return;
        }
        let port = std::env::var("ANVIL_BROWSER_PROBE_MOCK_PORT")
            .unwrap()
            .parse::<u16>()
            .unwrap();
        let status = std::env::var("ANVIL_BROWSER_PROBE_MOCK_STATUS").unwrap();
        let startup_delay = std::env::var("ANVIL_BROWSER_PROBE_MOCK_DELAY_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        if startup_delay > 0 {
            std::thread::sleep(Duration::from_millis(startup_delay));
        }
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        if status == "hang" {
            std::thread::sleep(Duration::from_secs(30));
            return;
        }
        let (mut stream, _) = listener.accept().unwrap();
        let mut request = [0u8; 512];
        let _ = stream.read(&mut request);
        let env_sensitive_failure = status == "env-sensitive"
            && (std::env::var_os("NODE_ENV").is_some()
                || std::env::var_os("NODE_OPTIONS").is_some());
        let code = if status == "500" || status == "node-env-marker" || env_sensitive_failure {
            500
        } else {
            200
        };
        if status == "node-env-marker" {
            eprintln!("Next.js detected a non-standard \"NODE_ENV\" value.");
        } else if code == 500 {
            eprintln!("Module parse failed: Unexpected character '@' (1:0)");
        }
        let body = if status == "node-env-marker" {
            "Next.js detected a non-standard \"NODE_ENV\" value."
        } else if code == 500 {
            "Module parse failed: Unexpected character '@'"
        } else {
            "ok"
        };
        let response = format!(
            "HTTP/1.1 {code} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(response.as_bytes()).unwrap();
    }

    fn probe_with_mock_child(
        root: &Path,
        port: u16,
        status: &str,
        startup_delay_ms: u64,
        timeout: Duration,
    ) -> BrowserReadinessObservation {
        let exe = std::env::current_exe().unwrap();
        let command = ProbeCommand {
            program: exe.display().to_string(),
            args: vec![
                "--ignored".to_string(),
                "--exact".to_string(),
                "minimal_loop::browser_probe::tests::browser_probe_mock_server_child".to_string(),
                "--nocapture".to_string(),
            ],
            env: vec![
                (
                    "ANVIL_BROWSER_PROBE_MOCK_CHILD".to_string(),
                    "1".to_string(),
                ),
                (
                    "ANVIL_BROWSER_PROBE_MOCK_PORT".to_string(),
                    port.to_string(),
                ),
                (
                    "ANVIL_BROWSER_PROBE_MOCK_STATUS".to_string(),
                    status.to_string(),
                ),
                (
                    "ANVIL_BROWSER_PROBE_MOCK_DELAY_MS".to_string(),
                    startup_delay_ms.to_string(),
                ),
            ],
            display: "mock browser probe child".to_string(),
        };
        probe_browser_readiness_with_options(
            root,
            "nextjs",
            ProbeOptions {
                port: Some(port),
                timeout,
                offline: false,
                require_build: false,
                command_override: Some(command),
            },
        )
    }

    fn free_port() -> u16 {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    }

    fn run_ignored_browser_probe_harness(test_name: &str) -> std::process::ExitStatus {
        let exe = std::env::current_exe().unwrap();
        Command::new(exe)
            .args(["--ignored", "--exact", test_name, "--nocapture"])
            .env("NODE_ENV", "production")
            .env("NODE_OPTIONS", "--require ./host-hook.js")
            .status()
            .unwrap()
    }
}
