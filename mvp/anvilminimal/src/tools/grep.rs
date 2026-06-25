use std::path::Path;

use globset::{Glob, GlobSetBuilder};
use regex::RegexBuilder;

use super::workspace_policy::{WorkspacePolicy, should_skip_path};

const MAX_GREP_HITS: usize = 80;
const MAX_GREP_BYTES: usize = 24_000;

pub fn run(
    root: &Path,
    pattern: &str,
    glob: Option<&str>,
    case_sensitive: bool,
    policy: WorkspacePolicy,
) -> anyhow::Result<String> {
    let regex = RegexBuilder::new(pattern)
        .case_insensitive(!case_sensitive)
        .build()?;
    let globset = if let Some(glob) = glob {
        let mut builder = GlobSetBuilder::new();
        builder.add(Glob::new(glob)?);
        Some(builder.build()?)
    } else {
        None
    };
    let mut hits = Vec::new();
    walk(root, root, &regex, globset.as_ref(), policy, &mut hits)?;
    hits.sort();
    Ok(summarize_hits(&hits))
}

fn walk(
    root: &Path,
    dir: &Path,
    regex: &regex::Regex,
    globset: Option<&globset::GlobSet>,
    policy: WorkspacePolicy,
    hits: &mut Vec<String>,
) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if should_skip_path(root, &path, policy) {
            continue;
        }
        if path.is_dir() {
            walk(root, &path, regex, globset, policy, hits)?;
            continue;
        }
        if hits.len() >= MAX_GREP_HITS {
            continue;
        }
        let rel = path.strip_prefix(root).unwrap_or(&path);
        if globset.is_some_and(|set| !set.is_match(rel)) {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if regex.is_match(line) {
                hits.push(format!(
                    "{}:{}:{}",
                    rel.to_string_lossy().replace('\\', "/"),
                    idx + 1,
                    line
                ));
                if hits.len() >= MAX_GREP_HITS {
                    break;
                }
            }
        }
    }
    Ok(())
}

fn summarize_hits(hits: &[String]) -> String {
    let joined = hits.join("\n");
    if joined.len() <= MAX_GREP_BYTES && hits.len() < MAX_GREP_HITS {
        return joined;
    }
    let mut end = MAX_GREP_BYTES.min(joined.len());
    while !joined.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n[anvilminimal: grep output truncated; showing at most {} hits / {} bytes]",
        &joined[..end],
        MAX_GREP_HITS,
        MAX_GREP_BYTES
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grep_large_result_is_summarized() {
        let dir = tempfile::tempdir().unwrap();
        let mut content = String::new();
        for index in 0..120 {
            content.push_str(&format!("needle line {index}\n"));
        }
        std::fs::write(dir.path().join("large.txt"), content).unwrap();
        let output = run(
            dir.path(),
            "needle",
            None,
            false,
            WorkspacePolicy::NormalTask,
        )
        .unwrap();
        assert!(output.contains("grep output truncated"));
    }

    #[test]
    fn normal_workspace_policy_blocks_target_grep() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("target")).unwrap();
        std::fs::write(dir.path().join("target/log.txt"), "needle").unwrap();
        std::fs::write(dir.path().join("src.txt"), "other").unwrap();
        let output = run(
            dir.path(),
            "needle",
            None,
            false,
            WorkspacePolicy::NormalTask,
        )
        .unwrap();
        assert!(!output.contains("target/log.txt"));
    }
}
