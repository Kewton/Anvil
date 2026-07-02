use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

fn main() {
    println!("cargo:rerun-if-env-changed=SOURCE_DATE_EPOCH");
    println!("cargo:rerun-if-env-changed=ANVIL_FORCE_BUILD_INFO");
    emit_git_rerun_paths();

    let commit =
        git_output(&["rev-parse", "--short", "HEAD"]).unwrap_or_else(|| "unknown".to_string());
    let dirty = git_dirty();
    let timestamp = build_timestamp();
    let dirty_suffix = if dirty { "+dirty" } else { "" };
    let version = format!(
        "{} {}{} {}",
        std::env::var("CARGO_PKG_VERSION").unwrap_or_else(|_| "0.0.0".to_string()),
        commit,
        dirty_suffix,
        timestamp
    );

    println!("cargo:rustc-env=ANVIL_BUILD_COMMIT={commit}");
    println!("cargo:rustc-env=ANVIL_BUILD_DIRTY={dirty}");
    println!("cargo:rustc-env=ANVIL_BUILD_TIMESTAMP={timestamp}");
    println!("cargo:rustc-env=ANVIL_VERSION={version}");
}

fn git_output(args: &[&str]) -> Option<String> {
    let output = Command::new("git").args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

fn emit_git_rerun_paths() {
    let Some(git_dir) = git_output(&["rev-parse", "--git-dir"]) else {
        return;
    };
    let git_dir = PathBuf::from(git_dir);
    let git_dir = if git_dir.is_absolute() {
        git_dir
    } else {
        PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string()))
            .join(git_dir)
    };
    println!("cargo:rerun-if-changed={}", git_dir.join("HEAD").display());
    println!("cargo:rerun-if-changed={}", git_dir.join("index").display());
}

fn git_dirty() -> bool {
    git_output(&["status", "--porcelain"])
        .map(|status| !status.trim().is_empty())
        .unwrap_or(false)
}

fn build_timestamp() -> String {
    if let Ok(epoch) = std::env::var("SOURCE_DATE_EPOCH")
        && let Ok(seconds) = epoch.parse::<u64>()
    {
        return format!("unix:{seconds}");
    }
    command_output("date", &["-u", "+%Y-%m-%dT%H:%M:%SZ"]).unwrap_or_else(|| {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or_default();
        format!("unix:{seconds}")
    })
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program).args(args).output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8(output.stdout).ok()?;
    let trimmed = text.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}
