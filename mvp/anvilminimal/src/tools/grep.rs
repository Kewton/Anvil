use std::path::Path;

use anyhow::Context;
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
    let matcher = LineMatcher::new(pattern, case_sensitive);
    let globset = if let Some(glob) = glob {
        let mut builder = GlobSetBuilder::new();
        builder.add(Glob::new(glob).with_context(|| format!("invalid glob pattern: {glob}"))?);
        Some(builder.build().context("invalid glob pattern")?)
    } else {
        None
    };
    let mut hits = Vec::new();
    walk(root, root, &matcher, globset.as_ref(), policy, &mut hits)?;
    hits.sort();
    Ok(summarize_hits(&hits))
}

enum LineMatcher {
    Regex(regex::Regex),
    Substring {
        needle: String,
        case_sensitive: bool,
    },
}

impl LineMatcher {
    fn new(pattern: &str, case_sensitive: bool) -> Self {
        match RegexBuilder::new(pattern)
            .case_insensitive(!case_sensitive)
            .build()
        {
            Ok(regex) => Self::Regex(regex),
            Err(_) => Self::Substring {
                needle: if case_sensitive {
                    pattern.to_string()
                } else {
                    pattern.to_ascii_lowercase()
                },
                case_sensitive,
            },
        }
    }

    fn is_match(&self, line: &str) -> bool {
        match self {
            Self::Regex(regex) => regex.is_match(line),
            Self::Substring {
                needle,
                case_sensitive,
            } => {
                if *case_sensitive {
                    line.contains(needle)
                } else {
                    line.to_ascii_lowercase().contains(needle)
                }
            }
        }
    }
}

fn walk(
    root: &Path,
    dir: &Path,
    matcher: &LineMatcher,
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
            walk(root, &path, matcher, globset, policy, hits)?;
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
            if matcher.is_match(line) {
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

    #[test]
    fn invalid_regex_falls_back_to_literal_substring() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "literal [needle\n").unwrap();
        let output = run(
            dir.path(),
            "[needle",
            None,
            false,
            WorkspacePolicy::NormalTask,
        )
        .unwrap();
        assert!(output.contains("literal [needle"));
    }

    #[test]
    fn invalid_glob_is_reported_as_policy_error() {
        let dir = tempfile::tempdir().unwrap();
        let err = run(
            dir.path(),
            "needle",
            Some("["),
            false,
            WorkspacePolicy::NormalTask,
        )
        .unwrap_err();
        assert!(err.to_string().contains("invalid glob pattern"));
    }
}
