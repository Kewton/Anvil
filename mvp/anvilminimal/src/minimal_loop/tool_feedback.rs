use super::edit_anchor_recovery::EditAnchorRecovery;

pub(crate) fn recoverable_tool_feedback(
    name: &str,
    err: &anyhow::Error,
    edit_anchor_recovery: Option<&EditAnchorRecovery>,
) -> String {
    let err_text = err.to_string();
    if err_text.contains("verify_command_policy_error") {
        return format!(
            "Tool call `{name}` was rejected by deterministic verify policy: {err_text}. Allowed alternatives: use one bounded verifier command such as `npm run build`, `cargo test`, `python -m compileall -q src`, or `test -f relative/path`; create files with the Write tool; keep verify to one deterministic command; python-cli behavior-probe fixture CSVs already exist when required; move dependency installation or dev-server startup to setup/runtime phases, not verify."
        );
    }
    if err_text.contains("bash_path_confinement_error") {
        let guidance = confinement_retry_guidance(&err_text)
            .unwrap_or_else(|| "workspace相対で再実行せよ".to_string());
        let guidance = (!err_text.contains(&guidance))
            .then_some(guidance)
            .map(|guidance| format!(" {guidance}."))
            .unwrap_or_default();
        return format!(
            "Tool call `{name}` used an absolute path outside the current workspace and was rejected: {err_text}.{guidance}"
        );
    }
    if err_text.contains("stale_absolute_path_recoverable") {
        return format!(
            "Tool call `{name}` used an absolute path outside the current workspace and was rejected: {err_text}. Retry with the workspace-relative path named in the error; do not use an absolute path from another workspace."
        );
    }
    if err_text.contains("tool_args_path_near_root_corruption") {
        return format!(
            "Tool call `{name}` used a path that appears to reconstruct the current workspace root with a digit variance and was rejected. Do not salvage or write across workspaces; retry with a workspace-relative path under the exact current root quoted in the error: {err_text}."
        );
    }
    if let Some(recovery) = edit_anchor_recovery {
        return super::edit_anchor_recovery::feedback(name, &err_text, recovery);
    }
    format!(
        "Tool call `{name}` was rejected with a recoverable validation error: {err}. Retry with the same tool or another available tool using a valid JSON object that matches the tool schema."
    )
}

fn confinement_retry_guidance(err_text: &str) -> Option<String> {
    let marker = "use workspace-relative path `";
    let nearest = err_text.split_once(marker)?.1.split('`').next()?;
    (!nearest.is_empty()).then(|| crate::tools::bash::workspace_relative_retry_guidance(nearest))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bash_confinement_feedback_includes_deterministic_relative_retry() {
        let err = anyhow::anyhow!(
            "bash_path_confinement_error: rejected absolute path `/other/pipeline/main.py`; use workspace-relative path `pipeline/main.py`"
        );

        let feedback = recoverable_tool_feedback("Bash", &err, None);

        assert!(
            feedback.contains("workspace相対で再実行せよ: pipeline/main.py"),
            "{feedback}"
        );
    }
}
