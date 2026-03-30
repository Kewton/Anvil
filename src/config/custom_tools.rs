//! Custom tool definitions parsed from the `## tools` section of ANVIL.md.

/// Prefix for custom tool display names (e.g. "custom.before-change").
pub const CUSTOM_TOOL_PREFIX: &str = "custom.";

/// Maximum number of custom tools allowed.
pub const MAX_CUSTOM_TOOLS: usize = 20;

/// Built-in tool names that custom tools must not collide with.
const BUILTIN_TOOL_NAMES: &[&str] = &[
    "file.read",
    "file.write",
    "file.edit",
    "file.edit_anchor",
    "file.search",
    "shell.exec",
    "web.fetch",
    "web.search",
    "agent.explore",
    "agent.plan",
    "git.status",
    "git.diff",
    "git.log",
];

/// A custom tool definition parsed from ANVIL.md `## tools` section.
#[derive(Debug, Clone, PartialEq)]
pub struct CustomToolDef {
    pub name: String,
    pub description: String,
    pub command: String,
    pub attributes: Vec<String>,
}

/// Build the LLM-facing display name with the custom prefix.
pub fn custom_tool_display_name(name: &str) -> String {
    format!("{CUSTOM_TOOL_PREFIX}{name}")
}

/// Strip the custom prefix; returns `None` if the name lacks the prefix.
pub fn strip_custom_prefix(name: &str) -> Option<&str> {
    name.strip_prefix(CUSTOM_TOOL_PREFIX)
}

/// Validate a custom tool name: must be `[a-zA-Z0-9-]+` and not collide with builtins.
fn validate_tool_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("custom tool name must not be empty".to_string());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(format!(
            "custom tool name '{name}' contains invalid characters (only a-zA-Z0-9 and - allowed)"
        ));
    }
    if BUILTIN_TOOL_NAMES.contains(&name) {
        return Err(format!(
            "custom tool name '{name}' conflicts with a built-in tool"
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// ANVIL.md `## tools` parser
// ---------------------------------------------------------------------------

/// Parse the `## tools` section out of ANVIL.md content.
///
/// Returns `(remaining_content, custom_tool_defs)`.
/// - `remaining_content` has the `## tools` section removed.
/// - Tools beyond [`MAX_CUSTOM_TOOLS`] are silently dropped with a warning.
pub fn parse_tools_section(content: &str) -> (String, Vec<CustomToolDef>) {
    // Locate `## tools` header (case-insensitive match for the word "tools").
    let header_start = content
        .lines()
        .enumerate()
        .find(|(_, line)| {
            let trimmed = line.trim();
            trimmed == "## tools" || trimmed == "## Tools"
        })
        .map(|(idx, _)| idx);

    let header_start = match header_start {
        Some(idx) => idx,
        None => return (content.to_string(), Vec::new()),
    };

    let lines: Vec<&str> = content.lines().collect();

    // Find end of tools section (next `## ` header or EOF).
    let section_end = lines
        .iter()
        .enumerate()
        .skip(header_start + 1)
        .find(|(_, line)| line.starts_with("## "))
        .map(|(idx, _)| idx)
        .unwrap_or(lines.len());

    // Extract section body lines.
    let section_lines = &lines[header_start + 1..section_end];

    // Parse tool definitions.
    let mut tools = Vec::new();
    let mut current: Option<ToolBuilder> = None;

    for line in section_lines {
        let trimmed = line.trim();
        if trimmed.starts_with("- name:") {
            // Flush previous tool.
            if let Some(builder) = current.take()
                && let Some(tool) = builder.build()
            {
                tools.push(tool);
            }
            let name = trimmed.trim_start_matches("- name:").trim().to_string();
            current = Some(ToolBuilder::new(name));
        } else if let Some(ref mut builder) = current {
            if trimmed.starts_with("description:") {
                builder.description = Some(
                    trimmed
                        .trim_start_matches("description:")
                        .trim()
                        .to_string(),
                );
            } else if trimmed.starts_with("command:") {
                builder.command = Some(trimmed.trim_start_matches("command:").trim().to_string());
            } else if trimmed.starts_with("attributes:") {
                let attrs_str = trimmed.trim_start_matches("attributes:").trim();
                builder.attributes = attrs_str
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
            }
        }
    }
    // Flush last tool.
    if let Some(builder) = current.take()
        && let Some(tool) = builder.build()
    {
        tools.push(tool);
    }

    // Enforce limit.
    if tools.len() > MAX_CUSTOM_TOOLS {
        eprintln!(
            "Warning: {} custom tools defined, only first {} will be used",
            tools.len(),
            MAX_CUSTOM_TOOLS
        );
        tools.truncate(MAX_CUSTOM_TOOLS);
    }

    // Validate names and filter out invalid ones.
    let tools: Vec<CustomToolDef> = tools
        .into_iter()
        .filter(|t| match validate_tool_name(&t.name) {
            Ok(()) => true,
            Err(e) => {
                eprintln!("Warning: skipping custom tool: {e}");
                false
            }
        })
        .collect();

    // Build remaining content (tools section removed).
    let mut remaining_lines: Vec<&str> = Vec::new();
    remaining_lines.extend_from_slice(&lines[..header_start]);
    remaining_lines.extend_from_slice(&lines[section_end..]);
    let remaining = remaining_lines.join("\n");

    (remaining, tools)
}

struct ToolBuilder {
    name: String,
    description: Option<String>,
    command: Option<String>,
    attributes: Vec<String>,
}

impl ToolBuilder {
    fn new(name: String) -> Self {
        Self {
            name,
            description: None,
            command: None,
            attributes: Vec::new(),
        }
    }

    fn build(self) -> Option<CustomToolDef> {
        let description = self.description?;
        let command = self.command?;
        Some(CustomToolDef {
            name: self.name,
            description,
            command,
            attributes: self.attributes,
        })
    }
}

// ---------------------------------------------------------------------------
// Template expansion
// ---------------------------------------------------------------------------

/// Shell-escape a string using single-quote wrapping.
///
/// Rejects NUL bytes and strips control characters.
pub fn shell_escape(s: &str) -> Result<String, String> {
    if s.contains('\0') {
        return Err("attribute value contains NUL byte".to_string());
    }
    // Strip control characters (0x00-0x1F, 0x7F) except common whitespace.
    let cleaned: String = s
        .chars()
        .filter(|c| !c.is_ascii_control() || *c == '\n' || *c == '\t')
        .collect();
    Ok(format!("'{}'", cleaned.replace('\'', "'\\''")))
}

/// Expand a command template by replacing `{attr}` placeholders with
/// shell-escaped parameter values.  Single-pass expansion (no double
/// expansion risk).
pub fn expand_command_template(
    template: &str,
    params: &[(String, String)],
) -> Result<String, String> {
    let mut result = template.to_string();
    for (key, value) in params {
        let placeholder = format!("{{{key}}}");
        let escaped = shell_escape(value)?;
        result = result.replace(&placeholder, &escaped);
    }
    Ok(result)
}

/// Convert a `serde_json::Value` (object) into a list of `(key, value)` pairs.
pub fn json_value_to_params(value: &serde_json::Value) -> Result<Vec<(String, String)>, String> {
    match value.as_object() {
        Some(obj) => {
            let mut params = Vec::new();
            for (k, v) in obj {
                let s = match v {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                };
                params.push((k.clone(), s));
            }
            Ok(params)
        }
        None => Ok(Vec::new()),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // -- parse_tools_section ------------------------------------------------

    #[test]
    fn parse_empty_content() {
        let (remaining, tools) = parse_tools_section("");
        assert_eq!(remaining, "");
        assert!(tools.is_empty());
    }

    #[test]
    fn parse_no_tools_section() {
        let content = "# Project\nSome instructions here.\n## notes\nNotes.";
        let (remaining, tools) = parse_tools_section(content);
        assert_eq!(remaining, content);
        assert!(tools.is_empty());
    }

    #[test]
    fn parse_single_tool() {
        let content = "\
## tools

- name: before-change
  description: Check constraints before editing
  command: cmd before-change {file} --format llm
  attributes: file";
        let (remaining, tools) = parse_tools_section(content);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].name, "before-change");
        assert_eq!(tools[0].description, "Check constraints before editing");
        assert_eq!(tools[0].command, "cmd before-change {file} --format llm");
        assert_eq!(tools[0].attributes, vec!["file"]);
        assert!(!remaining.contains("## tools"));
    }

    #[test]
    fn parse_multiple_tools() {
        let content = "\
## intro
Some text.
## tools

- name: lint
  description: Run linter
  command: lint {path}
  attributes: path

- name: deploy
  description: Deploy app
  command: deploy --env {env}
  attributes: env
## notes
End.";
        let (remaining, tools) = parse_tools_section(content);
        assert_eq!(tools.len(), 2);
        assert_eq!(tools[0].name, "lint");
        assert_eq!(tools[1].name, "deploy");
        assert!(remaining.contains("## intro"));
        assert!(remaining.contains("## notes"));
        assert!(!remaining.contains("## tools"));
    }

    #[test]
    fn parse_multiple_attributes() {
        let content = "\
## tools

- name: check
  description: Check
  command: check {file} {mode}
  attributes: file, mode";
        let (_, tools) = parse_tools_section(content);
        assert_eq!(tools[0].attributes, vec!["file", "mode"]);
    }

    #[test]
    fn parse_tool_missing_command_skipped() {
        let content = "\
## tools

- name: incomplete
  description: No command field";
        let (_, tools) = parse_tools_section(content);
        assert!(tools.is_empty());
    }

    #[test]
    fn parse_tool_name_validation_rejects_builtin() {
        let content = "\
## tools

- name: shell.exec
  description: Evil
  command: evil
  attributes: x";
        let (_, tools) = parse_tools_section(content);
        assert!(tools.is_empty());
    }

    #[test]
    fn parse_tool_name_validation_rejects_special_chars() {
        let content = "\
## tools

- name: my;tool
  description: Bad
  command: bad
  attributes: x";
        let (_, tools) = parse_tools_section(content);
        assert!(tools.is_empty());
    }

    // -- shell_escape -------------------------------------------------------

    #[test]
    fn shell_escape_basic() {
        assert_eq!(shell_escape("hello").unwrap(), "'hello'");
    }

    #[test]
    fn shell_escape_single_quote() {
        assert_eq!(shell_escape("it's").unwrap(), "'it'\\''s'");
    }

    #[test]
    fn shell_escape_rejects_nul() {
        assert!(shell_escape("ab\0cd").is_err());
    }

    #[test]
    fn shell_escape_strips_control_chars() {
        let result = shell_escape("a\x01b\x7fc").unwrap();
        assert_eq!(result, "'abc'");
    }

    // -- expand_command_template --------------------------------------------

    #[test]
    fn expand_template_basic() {
        let result = expand_command_template(
            "cmd {file} --flag",
            &[("file".to_string(), "src/main.rs".to_string())],
        )
        .unwrap();
        assert_eq!(result, "cmd 'src/main.rs' --flag");
    }

    #[test]
    fn expand_template_multiple_params() {
        let result = expand_command_template(
            "cmd {a} {b}",
            &[
                ("a".to_string(), "x".to_string()),
                ("b".to_string(), "y".to_string()),
            ],
        )
        .unwrap();
        assert_eq!(result, "cmd 'x' 'y'");
    }

    #[test]
    fn expand_template_escapes_injection() {
        let result = expand_command_template(
            "cmd {file}",
            &[("file".to_string(), "'; rm -rf /; echo '".to_string())],
        )
        .unwrap();
        // The malicious value is safely quoted.
        assert!(result.contains("'\\''"));
    }

    // -- display name helpers -----------------------------------------------

    #[test]
    fn display_name_prefix() {
        assert_eq!(custom_tool_display_name("lint"), "custom.lint");
    }

    #[test]
    fn strip_prefix_works() {
        assert_eq!(strip_custom_prefix("custom.lint"), Some("lint"));
        assert_eq!(strip_custom_prefix("file.read"), None);
    }

    // -- json_value_to_params -----------------------------------------------

    #[test]
    fn json_to_params_object() {
        let val = serde_json::json!({"file": "a.rs", "mode": "strict"});
        let params = json_value_to_params(&val).unwrap();
        assert!(params.contains(&("file".to_string(), "a.rs".to_string())));
        assert!(params.contains(&("mode".to_string(), "strict".to_string())));
    }

    #[test]
    fn json_to_params_non_object() {
        let val = serde_json::json!("just a string");
        let params = json_value_to_params(&val).unwrap();
        assert!(params.is_empty());
    }

    // -- validate_tool_name -------------------------------------------------

    #[test]
    fn validate_name_ok() {
        assert!(validate_tool_name("my-tool").is_ok());
        assert!(validate_tool_name("lint123").is_ok());
    }

    #[test]
    fn validate_name_empty() {
        assert!(validate_tool_name("").is_err());
    }

    #[test]
    fn validate_name_builtin_collision() {
        assert!(validate_tool_name("file.read").is_err());
    }
}
