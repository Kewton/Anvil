use std::path::Path;

use serde_json::Value;

use crate::planner::profile::profile_failure;
use crate::planner::verify::VerificationReport;

pub fn verify(root: &Path, goal: &str) -> VerificationReport {
    let package_path = root.join("package.json");
    let Ok(content) = std::fs::read_to_string(&package_path) else {
        return profile_failure("package.json missing");
    };
    let Ok(package): Result<Value, _> = serde_json::from_str(&content) else {
        return profile_failure("package.json invalid");
    };
    let deps = package.get("dependencies").and_then(Value::as_object);
    for dep in ["next", "react", "react-dom"] {
        if deps.is_none_or(|deps| !deps.contains_key(dep)) {
            return profile_failure(format!("dependency missing: {dep}"));
        }
    }
    let scripts = package.get("scripts").and_then(Value::as_object);
    let build = scripts
        .and_then(|scripts| scripts.get("build"))
        .and_then(Value::as_str);
    if build != Some("next build") {
        return profile_failure("scripts.build must be next build");
    }
    if goal.contains("3011") {
        let dev = scripts
            .and_then(|scripts| scripts.get("dev"))
            .and_then(Value::as_str)
            .unwrap_or("");
        if !(dev.contains("next dev") && (dev.contains("-p 3011") || dev.contains("--port 3011"))) {
            return profile_failure("dev script must run next dev on port 3011");
        }
    }
    let uses_alias = contains_in_files(root, "@/");
    if uses_alias {
        let Ok(tsconfig) = std::fs::read_to_string(root.join("tsconfig.json")) else {
            return profile_failure("tsconfig.json missing for @/* alias");
        };
        if !tsconfig.contains("\"@/*\"") {
            return profile_failure("tsconfig paths missing @/*");
        }
    }
    VerificationReport::pass()
}

fn contains_in_files(root: &Path, needle: &str) -> bool {
    for rel in ["app/page.tsx", "pages/index.tsx", "src/app/page.tsx"] {
        if std::fs::read_to_string(root.join(rel)).is_ok_and(|content| content.contains(needle)) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::planner::verify::VerifyStatus;

    #[test]
    fn nextjs_3011_port_required() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("package.json"),
            r#"{"dependencies":{"next":"x","react":"x","react-dom":"x"},"scripts":{"build":"next build","dev":"next dev"}}"#,
        )
        .unwrap();
        assert!(matches!(
            verify(dir.path(), "3011").status,
            VerifyStatus::ProfileContractFailed(_)
        ));
    }
}
