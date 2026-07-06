use std::path::Path;

#[test]
fn provider_calls_are_routed_through_wrapper() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders = Vec::new();
    collect_direct_chat_calls(&src, &src, &mut offenders);
    assert!(
        offenders.is_empty(),
        "direct .chat( calls must go through src/provider_call.rs:\n{}",
        offenders.join("\n")
    );
}

fn collect_direct_chat_calls(root: &Path, path: &Path, offenders: &mut Vec<String>) {
    let entries = std::fs::read_dir(path).unwrap_or_else(|err| {
        panic!("failed to read {}: {err}", path.display());
    });
    for entry in entries {
        let entry = entry.expect("directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_direct_chat_calls(root, &path, offenders);
            continue;
        }
        if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
            continue;
        }
        let relative = path.strip_prefix(root).expect("relative source path");
        if relative == Path::new("provider_call.rs") {
            continue;
        }
        let source = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!("failed to read {}: {err}", path.display());
        });
        for (line_index, line) in source.lines().enumerate() {
            if line.contains(".chat(") {
                offenders.push(format!("{}:{}", relative.display(), line_index + 1));
            }
        }
    }
}
