use std::path::Path;

use crate::planner::verify::VerificationReport;

pub fn build_repair_prompt(step_id: &str, report: &VerificationReport) -> String {
    format!(
        "Repair step `{step_id}`. Verification failed: {:?}. Make the smallest bounded change and then stop.",
        report.status
    )
}

pub fn save_repair_report(
    root: &Path,
    step_id: &str,
    report: &VerificationReport,
) -> anyhow::Result<std::path::PathBuf> {
    let dir = root.join(".anvil").join("repairs");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("repair-{step_id}.md"));
    std::fs::write(
        &path,
        format!("# Repair exhausted\n\n{:?}\n", report.status),
    )?;
    Ok(path)
}
