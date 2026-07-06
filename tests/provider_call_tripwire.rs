use std::path::{Path, PathBuf};

#[test]
fn provider_chat_calls_go_through_wrapper() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let src = root.join("src");
    let mut violations = Vec::new();
    collect_direct_chat_calls(&src, &mut violations);

    assert!(
        violations.is_empty(),
        "direct .chat( calls must stay inside src/provider_call.rs:\n{}",
        violations.join("\n")
    );
}

fn collect_direct_chat_calls(dir: &Path, violations: &mut Vec<String>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let entry = entry.unwrap();
        let path = entry.path();
        if path.is_dir() {
            collect_direct_chat_calls(&path, violations);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        if path.ends_with("provider_call.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&path).unwrap();
        for (index, line) in text.lines().enumerate() {
            if line.contains(".chat(") {
                let rel = path.strip_prefix(env!("CARGO_MANIFEST_DIR")).unwrap();
                violations.push(format!("{}:{}", rel.display(), index + 1));
            }
        }
    }
}
