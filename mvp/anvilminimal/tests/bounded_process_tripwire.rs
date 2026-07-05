use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn runtime_processes_route_through_bounded_utility() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    collect_rust_files(&src, &mut files);

    let mut violations = Vec::new();
    for path in files {
        if path.ends_with("src/bounded_process.rs") {
            continue;
        }
        let text = fs::read_to_string(&path).unwrap();
        let lines = text.lines().collect::<Vec<_>>();
        let mut in_test_mod = false;
        for (index, line) in lines.iter().enumerate() {
            if raw_process_invocation(line) && !in_test_mod {
                violations.push(format!(
                    "{}:{}: {}",
                    path.strip_prefix(env!("CARGO_MANIFEST_DIR"))
                        .unwrap_or(&path)
                        .display(),
                    index + 1,
                    line.trim()
                ));
            }

            let trimmed = line.trim();
            if trimmed.starts_with("mod tests") || trimmed.starts_with("pub mod tests") {
                let previous = lines[..index]
                    .iter()
                    .rev()
                    .take_while(|previous| {
                        previous.trim().is_empty() || previous.trim().starts_with("#[")
                    })
                    .any(|previous| previous.trim() == "#[cfg(test)]");
                if previous && line.contains('{') {
                    in_test_mod = true;
                }
            }
        }
    }

    assert!(
        violations.is_empty(),
        "direct child-process invocation outside bounded_process.rs:\n{}",
        violations.join("\n")
    );
}

fn collect_rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

fn raw_process_invocation(line: &str) -> bool {
    if line.contains(".spawn(") && !line.contains(".spawn(move") {
        return true;
    }
    if line.contains(".output(") {
        return true;
    }
    line.contains(".status()") && !line.contains("response.status()")
}
