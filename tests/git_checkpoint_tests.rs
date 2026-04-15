use std::fs;
use std::process::Command;

use anvil::git::checkpoint::GitCheckpointManager;
use tempfile::tempdir;

#[test]
fn checkpoint_round_trip_restores_changes() {
    let dir = tempdir().unwrap();
    run(dir.path(), ["git", "init"]).unwrap();
    run(
        dir.path(),
        ["git", "config", "user.email", "test@example.com"],
    )
    .unwrap();
    run(dir.path(), ["git", "config", "user.name", "Test User"]).unwrap();

    fs::write(dir.path().join("file.txt"), "base\n").unwrap();
    run(dir.path(), ["git", "add", "."]).unwrap();
    run(dir.path(), ["git", "commit", "-m", "init"]).unwrap();

    fs::write(dir.path().join("file.txt"), "before-checkpoint\n").unwrap();
    let mut manager = GitCheckpointManager::new(dir.path(), Vec::new());
    manager.create_checkpoint("checkpoint").unwrap();

    fs::write(dir.path().join("file.txt"), "after\n").unwrap();
    manager.rollback_latest().unwrap();
    assert_eq!(
        fs::read_to_string(dir.path().join("file.txt")).unwrap(),
        "before-checkpoint\n"
    );
}

fn run<const N: usize>(cwd: &std::path::Path, args: [&str; N]) -> Result<(), String> {
    let status = Command::new(args[0])
        .args(&args[1..])
        .current_dir(cwd)
        .status()
        .map_err(|err| format!("failed to spawn {}: {err}", args[0]))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("command failed: {:?}", args))
    }
}
