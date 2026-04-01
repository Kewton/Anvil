//! Tag-based tool call parser.
//!
//! Parses XML-like tag syntax into [`ToolInput`] variants using the
//! [`tag_spec`] table as the source of truth for tool names, attributes,
//! and child elements.

use crate::agent::tag_spec::{TOOL_TAG_SPECS, find_spec};
use crate::tooling::{AnchorEditParams, ToolInput};

/// Check whether a block appears to use tag-based tool call syntax.
///
/// Returns `true` if the block starts with `<tool ` or with a known tool
/// name tag (e.g., `<file.read`).
pub fn is_tag_format(block: &str) -> bool {
    let trimmed = block.trim();
    if trimmed.starts_with("<tool ") || trimmed.starts_with("<tool>") {
        return true;
    }
    // Check for direct tool name tags (exact boundary match to avoid false positives)
    for spec in TOOL_TAG_SPECS {
        let prefix = format!("<{}", spec.name);
        if trimmed.starts_with(&prefix) {
            // Ensure the next char after the tool name is a valid boundary
            let rest = &trimmed[prefix.len()..];
            if rest.is_empty()
                || rest.starts_with(' ')
                || rest.starts_with('>')
                || rest.starts_with('/')
            {
                return true;
            }
        }
    }
    false
}

/// Parse a tag-based tool call block into a tool name and [`ToolInput`].
///
/// Supports two syntaxes:
/// 1. `<tool name="tool_name" attr="value">children</tool>`
/// 2. `<tool_name attr="value">children</tool_name>` (direct name tag)
///
/// Self-closing tags (`<tool ... />`) are also supported.
pub fn parse_tag_tool_block(block: &str) -> Result<(String, ToolInput), String> {
    let trimmed = block.trim();

    // Determine tool name and the rest of the tag content
    let (tool_name, tag_body, children) = extract_tag_structure(trimmed)?;

    // Validate against tag_spec
    let spec =
        find_spec(&tool_name).ok_or_else(|| format!("unknown tool in tag format: {tool_name}"))?;

    // Extract attributes from the opening tag
    let attrs = extract_attributes(&tag_body);

    // Extract child elements
    let child_contents = extract_child_elements(&children, spec.child_elements);

    // Build ToolInput from attributes and children
    build_tool_input(&tool_name, &attrs, &child_contents)
}

/// Extract the tag structure: (tool_name, opening_tag_body, inner_content).
fn extract_tag_structure(block: &str) -> Result<(String, String, String), String> {
    // Pattern 1: <tool name="xxx" ...>...</tool> or <tool name="xxx" .../>
    if block.starts_with("<tool ") || block.starts_with("<tool>") {
        let (tag_body, rest) = split_opening_tag(block, "tool")?;

        // Extract tool name from name="..." attribute
        let tool_name = extract_attribute_value(&tag_body, "name")
            .ok_or_else(|| "missing name attribute in <tool> tag".to_string())?;

        // For self-closing tags, rest is empty
        return Ok((tool_name, tag_body, rest));
    }

    // Pattern 2: <tool_name attr="value">...</tool_name>
    for spec in TOOL_TAG_SPECS {
        let open_prefix = format!("<{}", spec.name);
        if block.starts_with(&open_prefix) {
            let (tag_body, rest) = split_opening_tag(block, spec.name)?;
            return Ok((spec.name.to_string(), tag_body, rest));
        }
    }

    Err("block does not start with a recognized tool tag".to_string())
}

/// Split an opening tag from the rest of the block.
///
/// Returns (opening_tag_attributes, inner_content).
/// Handles both `<tag ...>content</tag>` and `<tag .../>`
fn split_opening_tag(block: &str, tag_name: &str) -> Result<(String, String), String> {
    // Find the end of the opening tag
    let after_name = &block[tag_name.len() + 1..]; // skip `<tag_name`

    // Check for self-closing tag
    if let Some(close_pos) = after_name.find("/>") {
        let attrs = after_name[..close_pos].to_string();
        return Ok((attrs, String::new()));
    }

    // Find the closing `>`
    let gt_pos = after_name
        .find('>')
        .ok_or_else(|| format!("unclosed opening tag: <{tag_name}"))?;

    let attrs = after_name[..gt_pos].to_string();
    let after_gt = &after_name[gt_pos + 1..];

    // Find the closing tag
    let close_tag = format!("</{tag_name}>");
    if let Some(close_pos) = after_gt.rfind(&close_tag) {
        let inner = after_gt[..close_pos].to_string();
        Ok((attrs, inner))
    } else {
        // No closing tag — treat entire remainder as inner content
        Ok((attrs, after_gt.to_string()))
    }
}

/// Extract attribute values from a tag's attribute string.
///
/// Parses `key="value"` pairs, returning them as a Vec of (key, value).
fn extract_attributes(attrs_str: &str) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut remaining = attrs_str.trim();

    while !remaining.is_empty() {
        // Skip whitespace
        remaining = remaining.trim_start();
        if remaining.is_empty() {
            break;
        }

        // Find key=
        let eq_pos = match remaining.find('=') {
            Some(pos) => pos,
            None => break,
        };

        let key = remaining[..eq_pos].trim().to_string();
        remaining = &remaining[eq_pos + 1..];
        remaining = remaining.trim_start();

        // Extract quoted value
        if remaining.starts_with('"') {
            remaining = &remaining[1..];
            let end_quote = match remaining.find('"') {
                Some(pos) => pos,
                None => break,
            };
            let value = remaining[..end_quote].to_string();
            remaining = &remaining[end_quote + 1..];
            result.push((key, value));
        } else if remaining.starts_with('\'') {
            remaining = &remaining[1..];
            let end_quote = match remaining.find('\'') {
                Some(pos) => pos,
                None => break,
            };
            let value = remaining[..end_quote].to_string();
            remaining = &remaining[end_quote + 1..];
            result.push((key, value));
        } else {
            // Unquoted value — take until whitespace
            let end = remaining
                .find(char::is_whitespace)
                .unwrap_or(remaining.len());
            let value = remaining[..end].to_string();
            remaining = &remaining[end..];
            result.push((key, value));
        }
    }

    result
}

/// Extract the text content of child elements.
fn extract_child_elements(inner: &str, element_names: &[&str]) -> Vec<(String, String)> {
    let mut result = Vec::new();

    for &name in element_names {
        let open_tag = format!("<{name}>");
        let close_tag = format!("</{name}>");

        if let Some(start) = inner.find(&open_tag) {
            let content_start = start + open_tag.len();
            if let Some(end) = inner[content_start..].find(&close_tag) {
                let content = inner[content_start..content_start + end].to_string();
                result.push((name.to_string(), content));
            }
        }
    }

    result
}

/// Extract a specific attribute value from an attribute string.
fn extract_attribute_value(attrs_str: &str, key: &str) -> Option<String> {
    let attrs = extract_attributes(attrs_str);
    attrs.into_iter().find(|(k, _)| k == key).map(|(_, v)| v)
}

/// Build a [`ToolInput`] from parsed attributes and child elements.
fn build_tool_input(
    tool_name: &str,
    attrs: &[(String, String)],
    children: &[(String, String)],
) -> Result<(String, ToolInput), String> {
    let get_attr = |key: &str| -> Option<String> {
        attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
    };
    let get_child = |key: &str| -> Option<String> {
        children
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.clone())
    };

    let input = match tool_name {
        "file.read" => ToolInput::FileRead {
            path: get_attr("path")
                .ok_or_else(|| "missing path attribute for file.read".to_string())?,
        },
        "file.write" => ToolInput::FileWrite {
            path: get_attr("path")
                .ok_or_else(|| "missing path attribute for file.write".to_string())?,
            content: get_child("content")
                .ok_or_else(|| "missing content element for file.write".to_string())?,
        },
        "file.edit" => {
            let path = get_attr("path")
                .ok_or_else(|| "missing path attribute for file.edit".to_string())?;
            let old_string = get_child("old_string")
                .ok_or_else(|| "missing old_string element for file.edit".to_string())?;
            let new_string = get_child("new_string").unwrap_or_default();
            ToolInput::FileEdit {
                path,
                old_string,
                new_string,
            }
        }
        "file.edit_anchor" => {
            let path = get_attr("path")
                .ok_or_else(|| "missing path attribute for file.edit_anchor".to_string())?;
            let old_content = get_child("old_content")
                .ok_or_else(|| "missing old_content element for file.edit_anchor".to_string())?;
            let new_content = get_child("new_content").unwrap_or_default();
            ToolInput::FileEditAnchor {
                path,
                params: AnchorEditParams {
                    old_content,
                    new_content,
                },
            }
        }
        "file.search" => ToolInput::FileSearch {
            root: get_attr("root")
                .unwrap_or_else(|| crate::tooling::DEFAULT_SEARCH_ROOT.to_string()),
            pattern: get_attr("pattern")
                .ok_or_else(|| "missing pattern attribute for file.search".to_string())?,
            regex: false,
            context_lines: 0,
        },
        "shell.exec" => ToolInput::ShellExec {
            command: get_attr("command")
                .ok_or_else(|| "missing command attribute for shell.exec".to_string())?,
        },
        "web.fetch" => ToolInput::WebFetch {
            url: get_attr("url")
                .ok_or_else(|| "missing url attribute for web.fetch".to_string())?,
        },
        "web.search" => ToolInput::WebSearch {
            query: get_attr("query")
                .ok_or_else(|| "missing query attribute for web.search".to_string())?,
        },
        "agent.explore" => ToolInput::AgentExplore {
            prompt: get_child("prompt")
                .or_else(|| get_child("query"))
                .ok_or_else(|| "missing prompt element for agent.explore".to_string())?,
            scope: get_attr("scope"),
        },
        "agent.plan" => ToolInput::AgentPlan {
            prompt: get_child("prompt")
                .or_else(|| get_child("query"))
                .ok_or_else(|| "missing prompt element for agent.plan".to_string())?,
            scope: get_attr("scope"),
        },
        _ => return Err(format!("unsupported tool in tag format: {tool_name}")),
    };

    Ok((tool_name.to_string(), input))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_tag_format_detects_tool_tag() {
        assert!(is_tag_format(r#"<tool name="file.read" path="./src"/>"#));
        assert!(is_tag_format(r#"<file.read path="./src"/>"#));
        assert!(is_tag_format(
            r#"<tool name="file.edit" path="./a.rs"><old_string>x</old_string><new_string>y</new_string></tool>"#
        ));
    }

    #[test]
    fn is_tag_format_rejects_non_tag() {
        assert!(!is_tag_format(r#"{"tool":"file.read","path":"./src"}"#));
        assert!(!is_tag_format("plain text"));
        assert!(!is_tag_format("<div>html</div>"));
    }

    #[test]
    fn parse_self_closing_file_read() {
        let block = r#"<tool name="file.read" path="./src/main.rs"/>"#;
        let (name, input) = parse_tag_tool_block(block).unwrap();
        assert_eq!(name, "file.read");
        assert_eq!(
            input,
            ToolInput::FileRead {
                path: "./src/main.rs".to_string()
            }
        );
    }

    #[test]
    fn parse_direct_name_tag_file_read() {
        let block = r#"<file.read path="./src/main.rs"/>"#;
        let (name, input) = parse_tag_tool_block(block).unwrap();
        assert_eq!(name, "file.read");
        assert_eq!(
            input,
            ToolInput::FileRead {
                path: "./src/main.rs".to_string()
            }
        );
    }

    #[test]
    fn parse_file_edit_with_children() {
        let block = r#"<tool name="file.edit" path="./src/main.rs"><old_string>fn old()</old_string><new_string>fn new()</new_string></tool>"#;
        let (name, input) = parse_tag_tool_block(block).unwrap();
        assert_eq!(name, "file.edit");
        assert_eq!(
            input,
            ToolInput::FileEdit {
                path: "./src/main.rs".to_string(),
                old_string: "fn old()".to_string(),
                new_string: "fn new()".to_string(),
            }
        );
    }

    #[test]
    fn parse_file_edit_anchor_with_children() {
        let block = r#"<tool name="file.edit_anchor" path="./src/main.rs"><old_content>fn old()</old_content><new_content>fn new()</new_content></tool>"#;
        let (name, input) = parse_tag_tool_block(block).unwrap();
        assert_eq!(name, "file.edit_anchor");
        assert_eq!(
            input,
            ToolInput::FileEditAnchor {
                path: "./src/main.rs".to_string(),
                params: AnchorEditParams {
                    old_content: "fn old()".to_string(),
                    new_content: "fn new()".to_string(),
                },
            }
        );
    }

    #[test]
    fn parse_shell_exec() {
        let block = r#"<tool name="shell.exec" command="ls -la"/>"#;
        let (name, input) = parse_tag_tool_block(block).unwrap();
        assert_eq!(name, "shell.exec");
        assert_eq!(
            input,
            ToolInput::ShellExec {
                command: "ls -la".to_string()
            }
        );
    }

    #[test]
    fn parse_unknown_tool_rejected() {
        let block = r#"<tool name="unknown.tool" arg="val"/>"#;
        let result = parse_tag_tool_block(block);
        assert!(result.is_err());
    }

    #[test]
    fn parse_malformed_tag_rejected() {
        let block = "<tool";
        let result = parse_tag_tool_block(block);
        assert!(result.is_err());
    }

    #[test]
    fn parse_file_write_with_content() {
        let block =
            r#"<tool name="file.write" path="./out.txt"><content>hello world</content></tool>"#;
        let (name, input) = parse_tag_tool_block(block).unwrap();
        assert_eq!(name, "file.write");
        assert_eq!(
            input,
            ToolInput::FileWrite {
                path: "./out.txt".to_string(),
                content: "hello world".to_string(),
            }
        );
    }
}
