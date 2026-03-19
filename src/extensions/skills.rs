use std::path::{Path, PathBuf};

use super::{SlashCommandAction, SlashCommandSpec, normalize_command_name};

/// Scope indicating where a skill was loaded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkillScope {
    User,
    Project,
}

/// Parsed SKILL.md frontmatter fields.
#[derive(Debug, Clone)]
pub struct SkillFrontmatter {
    pub name: String,
    pub description: String,
    pub argument_hint: String,
    pub disable_auto_invocation: bool,
    pub user_invocable: bool,
}

/// A fully parsed skill ready for registration.
#[derive(Debug, Clone)]
pub struct ParsedSkill {
    pub frontmatter: SkillFrontmatter,
    pub content: String,
    pub skill_dir: PathBuf,
    pub scope: SkillScope,
}

/// Maximum file size for SKILL.md (100 KB).
const MAX_SKILL_FILE_SIZE: u64 = 100 * 1024;

/// Parse frontmatter from SKILL.md content.
///
/// Extracts the YAML block between `---` delimiters and parses key: value pairs.
/// Returns the parsed frontmatter and the remaining markdown body content.
pub fn parse_frontmatter(content: &str) -> Result<(SkillFrontmatter, String), String> {
    let lines: Vec<&str> = content.lines().collect();

    // First line must be ---
    if lines.is_empty() || lines[0].trim() != "---" {
        return Err("frontmatter must start with ---".to_string());
    }

    // Find closing ---
    let closing_index = lines
        .iter()
        .enumerate()
        .skip(1)
        .find(|(_, line)| line.trim() == "---")
        .map(|(i, _)| i);

    let closing_index = match closing_index {
        Some(i) => i,
        None => return Err("frontmatter closing --- not found".to_string()),
    };

    // Parse key: value pairs from frontmatter
    let mut name: Option<String> = None;
    let mut description: Option<String> = None;
    let mut argument_hint = String::new();
    let mut disable_auto_invocation = true;
    let mut user_invocable = true;

    for line in &lines[1..closing_index] {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            return Err(format!("invalid frontmatter line: {trimmed}"));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "name" => name = Some(value.to_string()),
            "description" => description = Some(value.to_string()),
            "argument-hint" => argument_hint = value.to_string(),
            "disable-auto-invocation" => {
                disable_auto_invocation = value == "true";
            }
            "user-invocable" => {
                user_invocable = value == "true";
            }
            _ => {} // Ignore unknown fields
        }
    }

    let name = name.ok_or_else(|| "missing required field: name".to_string())?;
    let description =
        description.ok_or_else(|| "missing required field: description".to_string())?;

    // Remaining content after frontmatter
    let body_lines = &lines[(closing_index + 1)..];
    let body = body_lines.join("\n");

    Ok((
        SkillFrontmatter {
            name,
            description,
            argument_hint,
            disable_auto_invocation,
            user_invocable,
        },
        body,
    ))
}

/// Parse a SKILL.md file from disk.
///
/// Validates file size, frontmatter, and name/directory consistency.
fn parse_skill_md(path: &Path, dir_name: &str, scope: SkillScope) -> Result<ParsedSkill, String> {
    // Check file size
    let metadata =
        std::fs::metadata(path).map_err(|e| format!("cannot read {}: {e}", path.display()))?;
    if metadata.len() > MAX_SKILL_FILE_SIZE {
        return Err(format!(
            "SKILL.md exceeds size limit ({}KB > {}KB)",
            metadata.len() / 1024,
            MAX_SKILL_FILE_SIZE / 1024
        ));
    }

    let content = std::fs::read_to_string(path)
        .map_err(|e| format!("cannot read {}: {e}", path.display()))?;

    let (frontmatter, body) = parse_frontmatter(&content)?;

    // Validate name matches directory name
    if frontmatter.name != dir_name {
        return Err(format!(
            "skill name '{}' does not match directory name '{}'",
            frontmatter.name, dir_name
        ));
    }

    // Validate skill name would be a valid command name
    let command_name = format!("/{}", frontmatter.name);
    if normalize_command_name(&command_name).is_none() {
        return Err(format!("invalid skill name: {}", frontmatter.name));
    }

    let skill_dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();

    Ok(ParsedSkill {
        frontmatter,
        content: body,
        skill_dir,
        scope,
    })
}

/// Discover and load skills from user and project scopes.
///
/// Scans `~/.anvil/skills/` (user scope) and `{cwd}/.anvil/skills/` (project scope).
/// Returns a list of `SlashCommandSpec` entries for discovered skills.
/// Errors are logged as warnings and individual skills are skipped.
pub fn discover_and_load(
    cwd: &Path,
    home_dir: Option<&Path>,
    existing_commands: &[SlashCommandSpec],
) -> Vec<SlashCommandSpec> {
    let mut result: Vec<SlashCommandSpec> = Vec::new();

    // User scope: ~/.anvil/skills/*/SKILL.md
    if let Some(home) = home_dir {
        let user_skills_dir = home.join(".anvil").join("skills");
        load_skills_from_dir(
            &user_skills_dir,
            SkillScope::User,
            existing_commands,
            &mut result,
        );
    }

    // Project scope: {cwd}/.anvil/skills/*/SKILL.md
    let project_skills_dir = cwd.join(".anvil").join("skills");
    load_skills_from_dir(
        &project_skills_dir,
        SkillScope::Project,
        existing_commands,
        &mut result,
    );

    result
}

/// Load skills from a directory, applying collision and invocability checks.
fn load_skills_from_dir(
    skills_dir: &Path,
    scope: SkillScope,
    existing_commands: &[SlashCommandSpec],
    result: &mut Vec<SlashCommandSpec>,
) {
    let entries = match std::fs::read_dir(skills_dir) {
        Ok(entries) => entries,
        Err(_) => return, // Directory doesn't exist or isn't readable
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => continue,
        };

        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let dir_name = match path.file_name().and_then(|n| n.to_str()) {
            Some(name) => name.to_string(),
            None => continue,
        };

        let skill_md_path = path.join("SKILL.md");
        if !skill_md_path.exists() {
            continue;
        }

        let parsed = match parse_skill_md(&skill_md_path, &dir_name, scope) {
            Ok(skill) => skill,
            Err(e) => {
                tracing::warn!("skipping skill '{}': {}", dir_name, e);
                continue;
            }
        };

        // Skip non-user-invocable skills
        if !parsed.frontmatter.user_invocable {
            tracing::info!("skipping non-user-invocable skill '{}'", dir_name);
            continue;
        }

        let command_name = format!("/{}", parsed.frontmatter.name);

        // Check collision with existing builtin/custom commands
        if existing_commands
            .iter()
            .any(|spec| spec.name == command_name)
        {
            tracing::warn!(
                "skipping skill '{}': conflicts with existing command",
                dir_name
            );
            continue;
        }

        // Check collision with already-loaded skills (project scope overrides user scope)
        if let Some(idx) = result.iter().position(|spec| spec.name == command_name) {
            if scope == SkillScope::Project {
                // Project scope overrides user scope
                tracing::info!("project skill '{}' overrides user skill", dir_name);
                result.remove(idx);
            } else {
                // Same scope collision: skip
                tracing::warn!("skipping duplicate skill '{}'", dir_name);
                continue;
            }
        }

        let description = if parsed.frontmatter.argument_hint.is_empty() {
            parsed.frontmatter.description.clone()
        } else {
            format!(
                "{} ({})",
                parsed.frontmatter.description, parsed.frontmatter.argument_hint
            )
        };

        result.push(SlashCommandSpec {
            name: command_name,
            description,
            action: SlashCommandAction::Skill {
                name: parsed.frontmatter.name.clone(),
                args: String::new(),
                content: parsed.content,
                skill_dir: parsed.skill_dir,
            },
            scope: Some(scope),
        });
    }
}

/// Expand variables in skill content before execution.
///
/// Expansion order:
/// 1. `${ANVIL_SKILL_DIR}` -> skill directory path
/// 2. `${ARGUMENTS}` -> args string
/// 3. `$ARGUMENTS` -> args string
pub fn expand_variables(content: &str, args: &str, skill_dir: &Path) -> String {
    let skill_dir_str = skill_dir.display().to_string();
    // First expand ${...} forms, then $NAME forms
    let result = content.replace("${ANVIL_SKILL_DIR}", &skill_dir_str);
    let result = result.replace("${ARGUMENTS}", args);
    result.replace("$ARGUMENTS", args)
}

/// Parse a skill command with argument separation.
///
/// Given input like "/my-skill arg1 arg2", finds the matching Skill command
/// and returns a new SlashCommandSpec with the args field populated.
pub fn parse_skill_command(
    command: &str,
    commands: &[SlashCommandSpec],
) -> Option<SlashCommandSpec> {
    // No arguments means exact match handled elsewhere
    let (cmd_name, args) = command.split_once(' ')?;
    let args = args.trim();

    commands.iter().find_map(|spec| {
        if spec.name != cmd_name {
            return None;
        }
        match &spec.action {
            SlashCommandAction::Skill {
                name,
                content,
                skill_dir,
                ..
            } => Some(SlashCommandSpec {
                name: spec.name.clone(),
                description: spec.description.clone(),
                action: SlashCommandAction::Skill {
                    name: name.clone(),
                    args: args.to_string(),
                    content: content.clone(),
                    skill_dir: skill_dir.clone(),
                },
                scope: spec.scope,
            }),
            _ => None,
        }
    })
}
