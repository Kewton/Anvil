use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::bounded_process::{self, BoundedProcessOutcomeKind};
use crate::minimal_loop::verifier_env;

pub const PIPELINE_RUN_EVIDENCE_PATH: &str = "evidence/pipeline-run.json";
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
const DEFAULT_MAX_STREAM_BYTES: usize = 24_000;
const DEFAULT_MAX_ARTIFACT_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_MAX_ARTIFACTS: usize = 256;

#[derive(Debug, Clone)]
pub struct PipelineProbeConfig {
    entry: PathBuf,
    timeout: Duration,
    max_stream_bytes: usize,
    max_artifact_bytes: u64,
    max_artifacts: usize,
}

impl PipelineProbeConfig {
    pub fn new(entry: impl Into<PathBuf>) -> Self {
        Self {
            entry: entry.into(),
            timeout: DEFAULT_TIMEOUT,
            max_stream_bytes: DEFAULT_MAX_STREAM_BYTES,
            max_artifact_bytes: DEFAULT_MAX_ARTIFACT_BYTES,
            max_artifacts: DEFAULT_MAX_ARTIFACTS,
        }
    }

    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    pub fn with_max_stream_bytes(mut self, max_stream_bytes: usize) -> Self {
        self.max_stream_bytes = max_stream_bytes;
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipelineOutcome {
    Exited,
    TimedOut,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StreamCapture {
    pub text: String,
    pub captured_bytes: usize,
    pub total_bytes: u64,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactCapture {
    pub path: String,
    pub bytes: u64,
    pub fnv1a64: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineIsolation {
    pub level: String,
    pub workspace_cwd: bool,
    pub environment_allowlist: bool,
    pub process_group: bool,
    pub bounded_timeout_ms: u64,
    pub offline_policy_applied: bool,
    pub network_namespace_enforced: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineProbeReport {
    pub capability_id: String,
    pub status: String,
    pub ok: bool,
    pub outcome: PipelineOutcome,
    pub command: Vec<String>,
    pub duration_ms: u64,
    pub exit_code: Option<i32>,
    pub stdout: StreamCapture,
    pub stderr: StreamCapture,
    pub artifacts: Vec<ArtifactCapture>,
    pub isolation: PipelineIsolation,
    pub failure_kinds: Vec<String>,
    pub capture_warnings: Vec<String>,
}

pub fn run(root: &Path, config: PipelineProbeConfig) -> anyhow::Result<PipelineProbeReport> {
    validate_config(&config)?;
    let _entry = validate_entry(root, &config.entry)?;
    let entry_display = config.entry.to_string_lossy().replace('\\', "/");
    let command_parts = vec![
        "python3".to_string(),
        "-B".to_string(),
        entry_display.clone(),
    ];
    let command_text = command_parts.join(" ");
    if let Some(reason) = crate::tools::bash::blocked_reason(&command_text, true) {
        bail!("pipeline entry blocked by offline command policy: {reason}");
    }

    let mut command = verifier_env::normalized_command_at_root("python3", root);
    command
        .arg("-B")
        .arg(&entry_display)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = bounded_process::spawn_child(&mut command)
        .with_context(|| format!("failed to spawn pipeline entry `{entry_display}`"))?;
    let stdout_reader = child
        .stdout
        .take()
        .context("pipeline stdout pipe missing")?;
    let stderr_reader = child
        .stderr
        .take()
        .context("pipeline stderr pipe missing")?;
    let stdout_handle = capture_stream(stdout_reader, config.max_stream_bytes);
    let stderr_handle = capture_stream(stderr_reader, config.max_stream_bytes);
    let bounded = bounded_process::wait_with_timeout(child, config.timeout);
    let stdout = join_capture(stdout_handle, "stdout")?;
    let stderr = join_capture(stderr_handle, "stderr")?;
    let bounded = bounded.context("failed while waiting for pipeline entry")?;

    let outcome = match bounded.kind {
        BoundedProcessOutcomeKind::Exited => PipelineOutcome::Exited,
        BoundedProcessOutcomeKind::TimedOut
        | BoundedProcessOutcomeKind::Cancelled
        | BoundedProcessOutcomeKind::CommandAbortedByUser => PipelineOutcome::TimedOut,
    };
    let exit_code = bounded.status.and_then(|status| status.code());
    let mut failure_kinds = Vec::new();
    if outcome == PipelineOutcome::TimedOut {
        failure_kinds.push("pipeline_timeout".to_string());
    } else if exit_code != Some(0) {
        failure_kinds.push("pipeline_exit_nonzero".to_string());
    }
    let (artifacts, mut artifact_failures) = capture_artifacts(root, &config);
    failure_kinds.append(&mut artifact_failures);
    let mut capture_warnings = Vec::new();
    if stdout.truncated {
        capture_warnings.push("stdout_truncated".to_string());
    }
    if stderr.truncated {
        capture_warnings.push("stderr_truncated".to_string());
    }
    let ok = failure_kinds.is_empty();
    let report = PipelineProbeReport {
        capability_id: "pipeline_probe".to_string(),
        status: if ok { "pass" } else { "failed" }.to_string(),
        ok,
        outcome,
        command: command_parts,
        duration_ms: millis_u64(bounded.elapsed),
        exit_code,
        stdout,
        stderr,
        artifacts,
        isolation: PipelineIsolation {
            level: "workspace_cwd_env_allowlist_bounded_offline_policy".to_string(),
            workspace_cwd: true,
            environment_allowlist: true,
            process_group: true,
            bounded_timeout_ms: millis_u64(config.timeout),
            offline_policy_applied: true,
            network_namespace_enforced: false,
        },
        failure_kinds,
        capture_warnings,
    };
    write_evidence(root, &report)?;
    Ok(report)
}

fn validate_config(config: &PipelineProbeConfig) -> anyhow::Result<()> {
    if config.timeout.is_zero() {
        bail!("pipeline timeout must be greater than zero");
    }
    if config.max_stream_bytes == 0 {
        bail!("pipeline stream limit must be greater than zero");
    }
    Ok(())
}

fn validate_entry(root: &Path, entry: &Path) -> anyhow::Result<PathBuf> {
    let entry_text = entry.to_string_lossy();
    crate::tools::path_guard::validate_workspace_relative(&entry_text)
        .context("invalid pipeline entry")?;
    let mut components = entry.components();
    if components.next() != Some(Component::Normal("pipeline".as_ref()))
        || components.any(|component| !matches!(component, Component::Normal(_)))
        || entry.extension().and_then(|extension| extension.to_str()) != Some("py")
    {
        bail!("pipeline entry must be a .py file under pipeline/");
    }
    let resolved = crate::tools::path_guard::resolve_existing(root, &entry_text)
        .context("pipeline entry is not an accessible workspace file")?;
    let pipeline = root
        .join("pipeline")
        .canonicalize()
        .context("pipeline entry parent is not accessible")?;
    if !resolved.starts_with(&pipeline) || !resolved.is_file() {
        bail!("pipeline entry must resolve to a file under pipeline/");
    }
    Ok(resolved)
}

fn capture_stream<R>(mut reader: R, max_bytes: usize) -> JoinHandle<std::io::Result<StreamCapture>>
where
    R: Read + Send + 'static,
{
    std::thread::spawn(move || {
        let mut captured = Vec::with_capacity(max_bytes.min(8192));
        let mut total_bytes = 0u64;
        let mut buffer = [0u8; 8192];
        loop {
            let read = reader.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            total_bytes = total_bytes.saturating_add(read as u64);
            let remaining = max_bytes.saturating_sub(captured.len());
            captured.extend_from_slice(&buffer[..read.min(remaining)]);
        }
        let captured_bytes = captured.len();
        Ok(StreamCapture {
            text: String::from_utf8_lossy(&captured).to_string(),
            captured_bytes,
            total_bytes,
            truncated: total_bytes > captured_bytes as u64,
        })
    })
}

fn join_capture(
    handle: JoinHandle<std::io::Result<StreamCapture>>,
    stream: &str,
) -> anyhow::Result<StreamCapture> {
    handle
        .join()
        .map_err(|_| anyhow::anyhow!("pipeline {stream} capture thread panicked"))?
        .with_context(|| format!("failed to capture pipeline {stream}"))
}

fn capture_artifacts(
    root: &Path,
    config: &PipelineProbeConfig,
) -> (Vec<ArtifactCapture>, Vec<String>) {
    let mut paths = Vec::new();
    let mut failures = Vec::new();
    collect_artifact_paths(root, &root.join("output"), &mut paths, &mut failures);
    paths.sort();
    if paths.len() > config.max_artifacts {
        failures.push("output_artifact_count_limit_exceeded".to_string());
        paths.truncate(config.max_artifacts);
    }
    let mut total_bytes = 0u64;
    let mut artifacts = Vec::new();
    for path in paths {
        let display = crate::tools::path_guard::relative_display(root, &path);
        let Ok(metadata) = path.metadata() else {
            failures.push(format!("output_artifact_unreadable:{display}"));
            continue;
        };
        total_bytes = total_bytes.saturating_add(metadata.len());
        if total_bytes > config.max_artifact_bytes {
            failures.push("output_artifact_bytes_limit_exceeded".to_string());
            break;
        }
        match fnv1a64_file(&path) {
            Ok(hash) => artifacts.push(ArtifactCapture {
                path: display,
                bytes: metadata.len(),
                fnv1a64: format!("{hash:016x}"),
            }),
            Err(_) => failures.push(format!("output_artifact_unreadable:{display}")),
        }
    }
    failures.sort();
    failures.dedup();
    (artifacts, failures)
}

fn collect_artifact_paths(
    root: &Path,
    path: &Path,
    paths: &mut Vec<PathBuf>,
    failures: &mut Vec<String>,
) {
    let Ok(metadata) = path.symlink_metadata() else {
        return;
    };
    let display = crate::tools::path_guard::relative_display(root, path);
    if metadata.file_type().is_symlink() {
        failures.push(format!("output_symlink_not_allowed:{display}"));
    } else if metadata.is_file() {
        paths.push(path.to_path_buf());
    } else if metadata.is_dir() {
        let Ok(entries) = std::fs::read_dir(path) else {
            failures.push(format!("output_artifact_unreadable:{display}"));
            return;
        };
        let mut children = Vec::new();
        for entry in entries {
            match entry {
                Ok(entry) => children.push(entry.path()),
                Err(_) => failures.push(format!("output_artifact_unreadable:{display}")),
            }
        }
        children.sort();
        for child in children {
            collect_artifact_paths(root, &child, paths, failures);
        }
    }
}

fn fnv1a64_file(path: &Path) -> std::io::Result<u64> {
    let mut file = std::fs::File::open(path)?;
    let mut hash = 0xcbf29ce484222325u64;
    let mut buffer = [0u8; 8192];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            return Ok(hash);
        }
        for byte in &buffer[..read] {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
}

fn write_evidence(root: &Path, report: &PipelineProbeReport) -> anyhow::Result<()> {
    let path =
        crate::tools::path_guard::resolve_optional_existing(root, PIPELINE_RUN_EVIDENCE_PATH)
            .context("pipeline evidence path escapes workspace")?;
    let parent = path.parent().context("pipeline evidence parent missing")?;
    std::fs::create_dir_all(parent)?;
    let mut file = std::fs::File::create(path)?;
    serde_json::to_writer_pretty(&mut file, report)?;
    file.write_all(b"\n")?;
    Ok(())
}

fn millis_u64(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_pipeline(root: &Path, body: &str) {
        std::fs::create_dir_all(root.join("pipeline")).unwrap();
        std::fs::write(root.join("pipeline/main.py"), body).unwrap();
    }

    #[test]
    fn runs_python3_and_records_generated_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        write_pipeline(
            dir.path(),
            r#"import json
from pathlib import Path
Path("output").mkdir(exist_ok=True)
Path("output/results.json").write_text(json.dumps({"ok": True}))
Path("output/report.md").write_text("total: 12.5")
print("pipeline stdout")
"#,
        );

        let report = run(dir.path(), PipelineProbeConfig::new("pipeline/main.py")).unwrap();

        assert!(report.ok, "{report:?}");
        assert_eq!(report.command, ["python3", "-B", "pipeline/main.py"]);
        assert!(report.stdout.text.contains("pipeline stdout"));
        assert_eq!(
            report
                .artifacts
                .iter()
                .map(|artifact| artifact.path.as_str())
                .collect::<Vec<_>>(),
            ["output/report.md", "output/results.json"]
        );
        let evidence: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(PIPELINE_RUN_EVIDENCE_PATH)).unwrap(),
        )
        .unwrap();
        assert_eq!(evidence["status"], "pass");
        assert_eq!(evidence["isolation"]["offline_policy_applied"], true);
        assert_eq!(evidence["isolation"]["network_namespace_enforced"], false);
    }

    #[test]
    fn times_out_hanging_pipeline_and_still_writes_evidence() {
        let dir = tempfile::tempdir().unwrap();
        write_pipeline(dir.path(), "while True:\n    pass\n");
        let config =
            PipelineProbeConfig::new("pipeline/main.py").with_timeout(Duration::from_millis(40));

        let report = run(dir.path(), config).unwrap();

        assert!(!report.ok);
        assert_eq!(report.outcome, PipelineOutcome::TimedOut);
        assert!(
            report
                .failure_kinds
                .contains(&"pipeline_timeout".to_string())
        );
        assert!(dir.path().join(PIPELINE_RUN_EVIDENCE_PATH).is_file());
    }

    #[test]
    fn drains_but_caps_large_stdout() {
        let dir = tempfile::tempdir().unwrap();
        write_pipeline(dir.path(), "print('x' * 100000)\n");
        let config = PipelineProbeConfig::new("pipeline/main.py")
            .with_timeout(Duration::from_secs(2))
            .with_max_stream_bytes(1024);

        let report = run(dir.path(), config).unwrap();

        assert!(report.ok, "{report:?}");
        assert!(report.stdout.truncated);
        assert_eq!(report.stdout.captured_bytes, 1024);
        assert!(report.stdout.total_bytes > report.stdout.captured_bytes as u64);
    }

    #[test]
    fn entry_must_be_a_real_python_file_under_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("outside.py"), "print('no')").unwrap();

        for entry in ["outside.py", "pipeline/../outside.py", "/tmp/outside.py"] {
            let error = run(dir.path(), PipelineProbeConfig::new(entry))
                .unwrap_err()
                .to_string();
            assert!(error.contains("pipeline entry"), "{error}");
        }
    }
}
