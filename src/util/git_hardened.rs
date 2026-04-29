//! Hardened `git` subprocess invocation shared by `session::case_record` and
//! `repo_graph::fingerprint` (Issue #468 / DR1-001).
//!
//! Originally a private helper in `session::case_record`; promoted to
//! `util::*` so both layers can depend on it without `repo_graph -> session`
//! reverse dependency. `run_git` is `cfg(unix)`-only because the
//! wait-with-timeout loop relies on poll-style `try_wait`; on other platforms
//! it returns `None` and callers fall back to `git_rev = None`.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

const GIT_SUBPROCESS_TIMEOUT: Duration = Duration::from_secs(1);

/// Run `git` in `work_root` with hardened config: hooks / pager / gpg /
/// credential helper silenced, all `GIT_*` env vars removed. Returns trimmed
/// stdout on success, `None` on any failure or timeout.
pub(crate) fn run_git(work_root: &Path, args: &[&str]) -> Option<String> {
    if !cfg!(unix) {
        // wait_with_timeout below is unix-only; bail safely on other platforms.
        return None;
    }
    let hardened_prefix = [
        "-c",
        "core.sshCommand=",
        "-c",
        "credential.helper=",
        "-c",
        "core.pager=cat",
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "gpg.program=/dev/null",
    ];
    let mut full_args: Vec<&str> = hardened_prefix.to_vec();
    full_args.extend_from_slice(args);

    let mut child = Command::new("git")
        .args(&full_args)
        .current_dir(work_root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_CONFIG")
        .env_remove("GIT_CONFIG_GLOBAL")
        .env_remove("GIT_CONFIG_SYSTEM")
        .env_remove("GIT_SSH")
        .env_remove("GIT_SSH_COMMAND")
        .env_remove("GIT_TRACE")
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .stdin(Stdio::null())
        .spawn()
        .ok()?;

    let started = Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(status) => {
                if !status.success() {
                    return None;
                }
                let output = child.wait_with_output().ok()?;
                let s = String::from_utf8_lossy(&output.stdout).trim().to_string();
                if s.is_empty() {
                    return None;
                }
                return Some(s);
            }
            None => {
                if started.elapsed() >= GIT_SUBPROCESS_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command as Cmd;

    #[test]
    #[cfg(unix)]
    fn run_git_returns_some_in_a_real_git_repo() {
        let dir = tempdir();
        let _ = Cmd::new("git").arg("init").current_dir(&dir).output().ok();
        let _ = Cmd::new("git")
            .args(["config", "user.email", "t@t"])
            .current_dir(&dir)
            .output()
            .ok();
        let _ = Cmd::new("git")
            .args(["config", "user.name", "t"])
            .current_dir(&dir)
            .output()
            .ok();
        std::fs::write(dir.join("a.txt"), b"x").unwrap();
        let _ = Cmd::new("git")
            .args(["add", "."])
            .current_dir(&dir)
            .output()
            .ok();
        let _ = Cmd::new("git")
            .args(["commit", "-m", "init", "--no-gpg-sign"])
            .current_dir(&dir)
            .output()
            .ok();
        let head = run_git(&dir, &["rev-parse", "HEAD"]);
        assert!(head.is_some(), "expected HEAD in initialized repo");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_git_returns_none_outside_git_repo() {
        let dir = tempdir();
        let head = run_git(&dir, &["rev-parse", "HEAD"]);
        assert!(head.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tempdir() -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        let n = format!(
            "anvil-git-hardened-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        p.push(n);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
}
