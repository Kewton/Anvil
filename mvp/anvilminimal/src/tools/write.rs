use std::fs::{self, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, bail};

pub fn run(root: &Path, path: &Path, content: &str) -> anyhow::Result<String> {
    write_checked(root, path, content)?;
    Ok(format!("wrote {}", path.display()))
}

pub fn write_checked(root: &Path, path: &Path, content: &str) -> anyhow::Result<()> {
    ensure_mutation_allowed(root, path)?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create parent directory {}", parent.display()))?;
    }
    verify_existing_components_inside(root, path)?;
    reject_target_symlink(path)?;
    let mut file = open_for_truncate_no_follow(path)?;
    file.write_all(content.as_bytes())
        .with_context(|| format!("failed to write {}", path.display()))?;
    file.flush()
        .with_context(|| format!("failed to flush {}", path.display()))?;
    Ok(())
}

pub fn ensure_mutation_allowed(root: &Path, path: &Path) -> anyhow::Result<()> {
    reject_target_symlink(path)?;
    verify_existing_components_inside(root, path)
}

fn reject_target_symlink(path: &Path) -> anyhow::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!(
                "symlink_write_blocked: refusing to write through {}",
                path.display()
            )
        }
        Ok(_) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => {
            Err(err).with_context(|| format!("failed to inspect target {}", path.display()))
        }
    }
}

fn verify_existing_components_inside(root: &Path, path: &Path) -> anyhow::Result<()> {
    let raw_root = root.to_path_buf();
    let root = root
        .canonicalize()
        .with_context(|| format!("workspace root is not accessible: {}", root.display()))?;
    let candidate = if path.is_absolute() && path.starts_with(&raw_root) {
        root.join(
            path.strip_prefix(&raw_root)
                .with_context(|| format!("path escapes workspace: {}", path.display()))?,
        )
    } else if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    let relative = candidate
        .strip_prefix(&root)
        .with_context(|| format!("path escapes workspace: {}", candidate.display()))?;
    let mut current = PathBuf::from(&root);
    for component in relative.components() {
        let Component::Normal(part) = component else {
            bail!("path escapes workspace: {}", candidate.display());
        };
        current.push(part);
        match fs::symlink_metadata(&current) {
            Ok(_) => {
                let canonical = current.canonicalize().with_context(|| {
                    format!("path component is not accessible: {}", current.display())
                })?;
                if !canonical.starts_with(&root) {
                    bail!(
                        "path escapes workspace through existing component {}; use workspace-relative paths",
                        current.display()
                    );
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => break,
            Err(err) => {
                return Err(err).with_context(|| {
                    format!("failed to inspect path component {}", current.display())
                });
            }
        }
    }
    Ok(())
}

fn open_for_truncate_no_follow(path: &Path) -> anyhow::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    options
        .open(path)
        .with_context(|| format!("failed to open {} for writing", path.display()))
}
