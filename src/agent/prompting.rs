use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use ignore::WalkBuilder;
use serde_json::Value;

use crate::session::store::ConversationMessage;

const MAX_TOOL_MESSAGE_CHARS: usize = 12_000;
const MAX_REPO_CONTEXT_CANDIDATES: usize = 4;
const MAX_REPO_CONTEXT_FILES: usize = 1_500;
const MAX_REPO_CONTEXT_FILE_BYTES: usize = 16_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolProtocol {
    Native,
    TaggedXml,
}

impl ToolProtocol {
    pub(crate) fn from_native_tools_enabled(native_tools_enabled: bool) -> Self {
        if native_tools_enabled {
            Self::Native
        } else {
            Self::TaggedXml
        }
    }

    pub(crate) fn native_tools_enabled(self) -> bool {
        matches!(self, Self::Native)
    }

    pub(crate) fn fallback_instruction(self) -> Option<String> {
        match self {
            Self::Native => None,
            Self::TaggedXml => Some(format!(
                "For this model, do not emit native tool_calls. When you need tools, output only {} with valid JSON arguments.",
                self.fallback_example()
            )),
        }
    }

    pub(crate) fn parser_downgrade_notice(self) -> String {
        match self {
            Self::Native => format!(
                "[Runtime Notice] Native tool calling is disabled for this session because the model/runtime parser rejected a tool-call response. Use only {} with valid JSON arguments.",
                Self::TaggedXml.fallback_example()
            ),
            Self::TaggedXml => format!(
                "[Runtime Notice] Continue using {} with valid JSON arguments.",
                self.fallback_example()
            ),
        }
    }

    pub(crate) fn fallback_example(self) -> &'static str {
        "<anvil_tool_call>{\"name\":\"Tool\",\"arguments\":{...}}</anvil_tool_call>"
    }
}

pub fn detect_language_hint(text: &str) -> &'static str {
    if text.chars().any(|ch| {
        ('\u{3040}'..='\u{30ff}').contains(&ch) || ('\u{4e00}'..='\u{9faf}').contains(&ch)
    }) {
        "Japanese"
    } else {
        "Match the user's language"
    }
}

pub(crate) fn runtime_context_messages(
    cwd: &Path,
    work_root: &Path,
    protocol: ToolProtocol,
) -> Vec<ConversationMessage> {
    let mut messages = Vec::new();
    if let Some(instruction) = protocol.fallback_instruction() {
        messages.push(ConversationMessage::system(instruction));
    }
    let _ = cwd;
    messages.push(ConversationMessage::system(format!(
        "Current project root is {}. All repository files live under this path. Use repository-relative paths (for example 'src/app/page.tsx') for Read, Write, and Edit. Never use absolute paths from other projects, user directories, or your memory such as '/Users/...' or '/home/...'.",
        work_root.display()
    )));
    messages
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RetrievalCandidate {
    path: String,
    score: usize,
    path_hits: Vec<String>,
    symbol_hits: Vec<String>,
    keyword_hits: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct QueryTerms {
    keywords: Vec<String>,
    symbols: Vec<String>,
}

pub(crate) fn repo_context_message(work_root: &Path, task: Option<&str>) -> Option<ConversationMessage> {
    let task = task?.trim();
    if task.is_empty() {
        return None;
    }
    let terms = extract_query_terms(task);
    if terms.keywords.is_empty() && terms.symbols.is_empty() {
        return None;
    }
    let candidates = rank_repo_candidates(work_root, &terms);
    if candidates.is_empty() {
        return None;
    }

    let mut lines = vec![
        "[Repo Context]".to_string(),
        "Likely relevant files for the current task:".to_string(),
    ];
    for candidate in candidates.into_iter().take(MAX_REPO_CONTEXT_CANDIDATES) {
        let mut reasons = Vec::new();
        if !candidate.path_hits.is_empty() {
            reasons.push(format!("path={}", candidate.path_hits.join(",")));
        }
        if !candidate.symbol_hits.is_empty() {
            reasons.push(format!("symbol={}", candidate.symbol_hits.join(",")));
        }
        if !candidate.keyword_hits.is_empty() {
            reasons.push(format!("keyword={}", candidate.keyword_hits.join(",")));
        }
        let suffix = if reasons.is_empty() {
            String::new()
        } else {
            format!(" | {}", reasons.join(" | "))
        };
        lines.push(format!("- {}{}", candidate.path, suffix));
    }
    lines.push(
        "Start with one of these files before broad Glob/Grep. Prefer a direct Read or Edit when one candidate clearly matches."
            .to_string(),
    );
    Some(ConversationMessage::system(lines.join("\n")))
}

fn rank_repo_candidates(work_root: &Path, terms: &QueryTerms) -> Vec<RetrievalCandidate> {
    let mut candidates = Vec::new();
    let walker = WalkBuilder::new(work_root)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build();
    let mut seen_files = 0usize;
    for entry in walker.filter_map(Result::ok) {
        let path = entry.path();
        if !entry.file_type().is_some_and(|kind| kind.is_file()) {
            continue;
        }
        if should_skip_retrieval_path(path) {
            continue;
        }
        seen_files += 1;
        if seen_files > MAX_REPO_CONTEXT_FILES {
            break;
        }
        let Some(relative) = path.strip_prefix(work_root).ok() else {
            continue;
        };
        let relative = relative.to_string_lossy().replace('\\', "/");
        if let Some(candidate) = score_candidate(path, &relative, terms) {
            candidates.push(candidate);
        }
    }
    candidates.sort_by(|lhs, rhs| {
        rhs.score
            .cmp(&lhs.score)
            .then_with(|| lhs.path.len().cmp(&rhs.path.len()))
            .then_with(|| lhs.path.cmp(&rhs.path))
    });
    candidates.truncate(MAX_REPO_CONTEXT_CANDIDATES);
    candidates
}

fn should_skip_retrieval_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(
            name.as_ref(),
            ".git" | ".anvil-state" | "target" | "node_modules" | "dist" | "build"
        )
    })
}

fn score_candidate(path: &Path, relative_path: &str, terms: &QueryTerms) -> Option<RetrievalCandidate> {
    let path_lower = relative_path.to_ascii_lowercase();
    let path_tokens = tokenize_fragments(relative_path, 2);
    let file_name_tokens = path
        .file_stem()
        .map(|stem| tokenize_fragments(&stem.to_string_lossy(), 2))
        .unwrap_or_default();
    let content = load_candidate_text(path);
    let content_lower = content.as_deref().map(str::to_ascii_lowercase);
    let content_tokens = content
        .as_deref()
        .map(|text| tokenize_fragments(text, 2))
        .unwrap_or_default();

    let mut score = 0usize;
    let mut path_hits = Vec::new();
    let mut symbol_hits = Vec::new();
    let mut keyword_hits = Vec::new();

    for keyword in &terms.keywords {
        if file_name_tokens.iter().any(|token| token == keyword) {
            score += 8;
            push_unique(&mut path_hits, keyword.clone());
            continue;
        }
        if path_tokens.iter().any(|token| token == keyword) {
            score += 5;
            push_unique(&mut path_hits, keyword.clone());
            continue;
        }
        if path_lower.contains(keyword) {
            score += 3;
            push_unique(&mut path_hits, keyword.clone());
        }
        if content_tokens.iter().any(|token| token == keyword) {
            score += 2;
            push_unique(&mut keyword_hits, keyword.clone());
        } else if content_lower
            .as_deref()
            .is_some_and(|body| body.contains(keyword))
        {
            score += 1;
            push_unique(&mut keyword_hits, keyword.clone());
        }
    }

    for symbol in &terms.symbols {
        if content_tokens.iter().any(|token| token == symbol)
            || content_lower
                .as_deref()
                .is_some_and(|body| body.contains(symbol))
        {
            score += 4;
            push_unique(&mut symbol_hits, symbol.clone());
        }
    }

    if path_hits.len() >= 2 && (!symbol_hits.is_empty() || !keyword_hits.is_empty()) {
        score += 3;
    }
    if score < 6 {
        return None;
    }

    Some(RetrievalCandidate {
        path: relative_path.to_string(),
        score,
        path_hits,
        symbol_hits,
        keyword_hits,
    })
}

fn load_candidate_text(path: &Path) -> Option<String> {
    let metadata = fs::metadata(path).ok()?;
    if metadata.len() > MAX_REPO_CONTEXT_FILE_BYTES as u64 {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    Some(content.chars().take(MAX_REPO_CONTEXT_FILE_BYTES).collect())
}

fn extract_query_terms(query: &str) -> QueryTerms {
    let mut symbol_terms = HashSet::new();
    for code_span in query.split('`').skip(1).step_by(2) {
        for token in tokenize_fragments(code_span, 3) {
            symbol_terms.insert(token);
        }
    }

    let mut keywords = HashSet::new();
    for token in tokenize_fragments(query, 3) {
        if !is_generic_query_word(&token) {
            keywords.insert(token);
        }
    }

    QueryTerms {
        keywords: sorted_terms(keywords),
        symbols: sorted_terms(symbol_terms),
    }
}

fn sorted_terms(values: HashSet<String>) -> Vec<String> {
    let mut terms = values.into_iter().collect::<Vec<_>>();
    terms.sort();
    terms
}

fn is_generic_query_word(token: &str) -> bool {
    matches!(
        token,
        "the"
            | "and"
            | "with"
            | "that"
            | "this"
            | "from"
            | "into"
            | "keep"
            | "rest"
            | "unchanged"
            | "available"
            | "tool"
            | "tools"
            | "file"
            | "files"
            | "current"
            | "project"
            | "root"
            | "reply"
            | "short"
            | "sentence"
            | "using"
            | "change"
            | "changes"
            | "update"
            | "edit"
            | "write"
            | "create"
            | "named"
            | "exactly"
            | "single"
            | "line"
            | "after"
            | "before"
            | "first"
            | "finish"
            | "minimal"
            | "directly"
            | "broad"
            | "prefer"
            | "start"
    )
}

fn tokenize_fragments(text: &str, min_len: usize) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_was_lower = false;
    for ch in text.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' {
            if ch.is_ascii_uppercase() && previous_was_lower && !current.is_empty() {
                push_token(&mut tokens, &mut current, min_len);
            }
            current.push(ch.to_ascii_lowercase());
            previous_was_lower = ch.is_ascii_lowercase();
        } else {
            push_token(&mut tokens, &mut current, min_len);
            previous_was_lower = false;
        }
    }
    push_token(&mut tokens, &mut current, min_len);
    tokens.sort();
    tokens.dedup();
    tokens
}

fn push_token(tokens: &mut Vec<String>, current: &mut String, min_len: usize) {
    if current.len() >= min_len {
        tokens.push(std::mem::take(current));
    } else {
        current.clear();
    }
}

fn push_unique(values: &mut Vec<String>, value: String) {
    if !values.iter().any(|existing| existing == &value) {
        values.push(value);
    }
}

pub(crate) fn detect_scaffold_root(name: &str, arguments: &Value, result: &str) -> Option<PathBuf> {
    if name != "Bash" {
        return None;
    }
    arguments.get("command").and_then(Value::as_str)?;
    detect_created_project_root(result)
}

pub(crate) fn should_skip_system_note(messages: &[ConversationMessage], note: &str) -> bool {
    for message in messages.iter().rev() {
        if message.role == "user" {
            break;
        }
        if message.role == "system" && message.content == note {
            return true;
        }
    }
    false
}

pub(crate) fn compact_tool_result(name: &str, result: String) -> String {
    if result.chars().count() <= MAX_TOOL_MESSAGE_CHARS {
        return result;
    }

    let chars = result.chars().collect::<Vec<_>>();
    let total = chars.len();
    let head_len = if name == "Read" { 8_000 } else { 10_000 }.min(total);
    let tail_len = if name == "Read" { 2_000 } else { 1_000 }.min(total.saturating_sub(head_len));
    let head = chars[..head_len].iter().collect::<String>();
    let tail = if tail_len == 0 {
        String::new()
    } else {
        chars[total - tail_len..].iter().collect::<String>()
    };

    if tail.is_empty() {
        format!("{head}\n...[truncated {} chars]", total - head_len)
    } else {
        format!(
            "{head}\n...[truncated {} chars]...\n{tail}",
            total.saturating_sub(head_len + tail_len)
        )
    }
}

pub(crate) fn detect_created_project_root(tool_output: &str) -> Option<PathBuf> {
    for line in tool_output.lines() {
        let trimmed = line.trim();
        if let Some(path) = trimmed.strip_prefix("Success! Created ") {
            let (_, path) = path.rsplit_once(" at ")?;
            return Some(PathBuf::from(path.trim()));
        }
        if let Some(rest) = trimmed.strip_prefix("Creating a new ")
            && let Some((_, path)) = rest.rsplit_once(" in ")
        {
            return Some(PathBuf::from(path.trim_end_matches('.').trim()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::repo_context_message;
    use tempfile::tempdir;

    #[test]
    fn repo_context_prefers_path_and_symbol_matches() {
        let temp = tempdir().unwrap();
        std::fs::create_dir_all(temp.path().join("src/billing")).unwrap();
        std::fs::create_dir_all(temp.path().join("src/ui")).unwrap();
        std::fs::write(
            temp.path().join("src/billing/retry_policy.ts"),
            "export const paymentRetryDelayMs = 3000;\nexport const paymentRetryLimit = 4;\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("src/ui/retry_banner.ts"),
            "export const retryBannerDelayMs = 1200;\n",
        )
        .unwrap();
        std::fs::write(
            temp.path().join("README.md"),
            "payment retries are documented here\n",
        )
        .unwrap();

        let message = repo_context_message(
            temp.path(),
            Some("Update the billing retry policy so `paymentRetryDelayMs` becomes 7000."),
        )
        .unwrap();
        let mut bullet_lines = message
            .content
            .lines()
            .filter(|line| line.starts_with("- "))
            .collect::<Vec<_>>();
        assert!(!bullet_lines.is_empty());
        assert!(bullet_lines.remove(0).contains("src/billing/retry_policy.ts"));
        assert!(message.content.contains("symbol=delay,payment,retry"));
    }
}
