use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::state::memory::MemoryStore;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomAction {
    MemoryAdd,
    MemoryShow,
    MemoryEdit,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomArgType {
    String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CustomArgDefinition {
    pub name: String,
    #[serde(rename = "type")]
    pub arg_type: CustomArgType,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CustomCommandDefinition {
    pub name: String,
    pub description: String,
    pub action: CustomAction,
    #[serde(default)]
    pub args: Vec<CustomArgDefinition>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomCommandInvocation {
    pub name: String,
    pub action: CustomAction,
    pub args: BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct CustomExecutionContext {
    pub memory_path: PathBuf,
}

impl CustomCommandDefinition {
    pub fn from_toml_str(input: &str) -> anyhow::Result<Self> {
        Ok(toml::from_str(input)?)
    }
}

impl CustomCommandInvocation {
    pub fn parse(def: &CustomCommandDefinition, input: &str) -> anyhow::Result<Option<Self>> {
        let tokens =
            shlex::split(input).ok_or_else(|| anyhow::anyhow!("invalid shell-like input"))?;
        let Some(first) = tokens.first() else {
            return Ok(None);
        };
        if first != &format!("/{}", def.name) {
            return Ok(None);
        }

        let allowed = def
            .args
            .iter()
            .map(|arg| arg.name.as_str())
            .collect::<BTreeSet<_>>();
        let mut args = BTreeMap::new();
        for token in tokens.iter().skip(1) {
            let Some((key, value)) = token.split_once('=') else {
                return Err(anyhow::anyhow!("arguments must use key=value form"));
            };
            if !allowed.contains(key) {
                return Err(anyhow::anyhow!("unknown argument: {key}"));
            }
            args.insert(key.to_string(), value.to_string());
        }

        for arg in &def.args {
            if arg.required && !args.contains_key(&arg.name) {
                return Err(anyhow::anyhow!("missing required argument: {}", arg.name));
            }
        }

        Ok(Some(Self {
            name: def.name.clone(),
            action: def.action.clone(),
            args,
        }))
    }

    pub fn execute(&self, cx: &CustomExecutionContext) -> anyhow::Result<String> {
        let store = MemoryStore::new(&cx.memory_path);
        match self.action {
            CustomAction::MemoryAdd => {
                store.add_entry(
                    self.args
                        .get("text")
                        .map(String::as_str)
                        .unwrap_or_default(),
                )?;
                Ok("memory updated".to_string())
            }
            CustomAction::MemoryShow => store.load(),
            CustomAction::MemoryEdit => {
                store.replace_all(
                    self.args
                        .get("text")
                        .map(String::as_str)
                        .unwrap_or_default(),
                )?;
                Ok("memory replaced".to_string())
            }
        }
    }
}

pub fn load_custom_commands(root: &Path) -> anyhow::Result<Vec<CustomCommandDefinition>> {
    let dir = root.join(".anvil/commands");
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut defs = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("toml") {
            continue;
        }
        defs.push(CustomCommandDefinition::from_toml_str(
            &std::fs::read_to_string(path)?,
        )?);
    }
    Ok(defs)
}
