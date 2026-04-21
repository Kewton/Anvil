use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
pub struct McpServerConfig {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct McpConfig {
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
}

#[derive(Debug, Clone)]
pub struct McpRegistry {
    path: PathBuf,
}

impl McpRegistry {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn load(&self) -> Result<McpConfig, String> {
        if !self.path.exists() {
            return Ok(McpConfig::default());
        }
        let content = fs::read_to_string(&self.path)
            .map_err(|err| format!("failed to read {}: {err}", self.path.display()))?;
        serde_json::from_str(&content)
            .map_err(|err| format!("failed to parse MCP config {}: {err}", self.path.display()))
    }

    pub fn status_lines(&self) -> Result<Vec<String>, String> {
        let config = self.load()?;
        if config.servers.is_empty() {
            return Ok(vec!["no MCP servers configured".to_string()]);
        }
        Ok(config
            .servers
            .into_iter()
            .map(|server| {
                format!(
                    "{} -> {} {}",
                    server.name,
                    server.command,
                    server.args.join(" ")
                )
            })
            .collect())
    }
}
