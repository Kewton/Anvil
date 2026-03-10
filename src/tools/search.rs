use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Default)]
pub struct SearchTool;

impl SearchTool {
    pub fn search(&self, root: &Path, needle: &str) -> anyhow::Result<Vec<SearchMatch>> {
        let mut matches = Vec::new();
        visit_dir(root, needle, &mut matches)?;
        Ok(matches)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub line_number: usize,
    pub line: String,
}

fn visit_dir(root: &Path, needle: &str, matches: &mut Vec<SearchMatch>) -> anyhow::Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?;

        if file_type.is_dir() {
            if path.file_name().and_then(|name| name.to_str()) == Some(".git") {
                continue;
            }
            visit_dir(&path, needle, matches)?;
            continue;
        }

        if !file_type.is_file() {
            continue;
        }

        if let Ok(contents) = fs::read_to_string(&path) {
            for (index, line) in contents.lines().enumerate() {
                if line.contains(needle) {
                    matches.push(SearchMatch {
                        path: path.clone(),
                        line_number: index + 1,
                        line: line.to_string(),
                    });
                }
            }
        }
    }

    Ok(())
}
