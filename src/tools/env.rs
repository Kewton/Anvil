use std::path::PathBuf;

#[derive(Debug, Default)]
pub struct EnvTool;

impl EnvTool {
    pub fn inspect(&self) -> anyhow::Result<EnvSnapshot> {
        Ok(EnvSnapshot {
            cwd: std::env::current_dir()?,
            shell: std::env::var("SHELL").ok(),
        })
    }
}

#[derive(Debug, Clone)]
pub struct EnvSnapshot {
    pub cwd: PathBuf,
    pub shell: Option<String>,
}
