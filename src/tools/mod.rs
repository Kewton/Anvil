use std::path::{Path, PathBuf};
use std::process::Command;

use glob::glob;

#[derive(Debug, Clone)]
pub struct GeneratedFile {
    pub path: PathBuf,
    pub content: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExecOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub line_number: usize,
    pub line: String,
}

pub fn write_files(base: &Path, files: &[GeneratedFile]) -> anyhow::Result<Vec<PathBuf>> {
    let mut written = Vec::new();
    for file in files {
        let dest = base.join(&file.path);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&dest, &file.content)?;
        written.push(dest);
    }
    Ok(written)
}

pub fn read_file(path: &Path) -> anyhow::Result<String> {
    Ok(std::fs::read_to_string(path)?)
}

pub fn write_file(path: &Path, content: &str) -> anyhow::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)?;
    Ok(())
}

pub fn edit_file(path: &Path, from: &str, to: &str) -> anyhow::Result<()> {
    let current = std::fs::read_to_string(path)?;
    let updated = current.replace(from, to);
    if updated == current {
        anyhow::bail!("edit target was not found")
    }
    std::fs::write(path, updated)?;
    Ok(())
}

pub fn exec_in_dir(cwd: &Path, argv: &[String]) -> anyhow::Result<ExecOutput> {
    let Some(program) = argv.first() else {
        anyhow::bail!("argv must not be empty");
    };
    let output = Command::new(program)
        .args(&argv[1..])
        .current_dir(cwd)
        .output()?;
    Ok(ExecOutput {
        status: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8(output.stdout)?,
        stderr: String::from_utf8(output.stderr)?,
    })
}

pub fn glob_paths(base: &Path, pattern: &str) -> anyhow::Result<Vec<PathBuf>> {
    let joined = base.join(pattern);
    let pattern = joined.to_string_lossy().to_string();
    let mut paths = glob(&pattern)?.filter_map(Result::ok).collect::<Vec<_>>();
    paths.sort();
    Ok(paths)
}

pub fn search_in_files(base: &Path, needle: &str) -> anyhow::Result<Vec<SearchMatch>> {
    let mut matches = Vec::new();
    for path in glob_paths(base, "**/*")? {
        if !path.is_file() {
            continue;
        }
        let Ok(content) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (idx, line) in content.lines().enumerate() {
            if line.contains(needle) {
                matches.push(SearchMatch {
                    path: path.clone(),
                    line_number: idx + 1,
                    line: line.to_string(),
                });
            }
        }
    }
    Ok(matches)
}

pub fn unified_diff(before: &str, after: &str) -> String {
    let before_lines = before.lines().collect::<Vec<_>>();
    let after_lines = after.lines().collect::<Vec<_>>();
    let mut out = String::from("--- before\n+++ after\n");
    for line in &before_lines {
        if !after_lines.contains(line) {
            out.push('-');
            out.push_str(line);
            out.push('\n');
        }
    }
    for line in &after_lines {
        if !before_lines.contains(line) {
            out.push('+');
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}
