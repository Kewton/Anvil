//! Shell command policy classification.
//!
//! Provides [`ShellPolicy`] enum and [`classify_shell_policy`] to split
//! shell.exec commands into ReadOnly / BuildTest / General categories.
//! Also provides [`is_network_command`] for offline mode enforcement.

/// Shell command execution policy classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellPolicy {
    /// Read-only commands (git log, ls, cat, etc.)
    /// PermissionClass: Safe (auto-approved)
    ReadOnly,
    /// Build/test commands (cargo test, cargo build, etc.)
    /// PermissionClass: Safe (auto-approved, maintains current UX)
    BuildTest,
    /// General commands (everything else)
    /// PermissionClass: Confirm (requires approval)
    General,
}

/// Read-only command prefixes (split from former SAFE_PREFIXES).
const READ_ONLY_PREFIXES: &[&str] = &[
    // Git read-only
    "git log",
    "git status",
    "git diff",
    "git branch",
    "git show ", // trailing space requires an argument (ref)
    "git remote -v",
    "git rev-parse",
    // GitHub CLI read-only
    "gh repo view",
    "gh pr list",
    "gh issue list",
    "gh pr view",
    "gh issue view",
    "gh auth status",
    // Environment inspection
    "which ",
    "uname",
    "node -v",
    "node --version",
    "rustc --version",
    "cargo --version",
    "python --version",
    "go version",
    // Process inspection
    "lsof -i",
];

/// Build/test command prefixes (split from former SAFE_PREFIXES).
const BUILD_TEST_PREFIXES: &[&str] = &[
    // Rust build/test/lint
    "cargo clippy",
    "cargo fmt --check",
    "cargo test",
    "cargo check",
    "cargo build",
    // Node.js build/test/lint
    "npm test",
    "npx jest ",
    "npx eslint ",
    "npx prettier --check",
    // Python test/lint
    "pytest",
    "ruff check ",
    "flake8",
    // Go build/test/lint
    "go test",
    "go vet",
    "golangci-lint",
    // Make build/test
    "make test",
    "make check",
];

/// Network command prefixes blocked in offline mode.
///
/// Scope (DR1-010): Only commands that establish network connections and
/// send/receive data. Local network state inspection commands (lsof -i, ss,
/// netstat) are NOT included.
///
/// Note: `git push`, `git fetch`, `git pull`, and `git clone` perform network
/// I/O but are NOT listed here. They are already classified as `General`
/// (not in `READ_ONLY_PREFIXES` or `BUILD_TEST_PREFIXES`), so they require
/// user confirmation before execution. Adding `git` sub-commands here would
/// require per-sub-command matching (simple prefix `"git"` would block
/// read-only git commands). This may be addressed in a future issue.
const NETWORK_COMMAND_PREFIXES: &[&str] = &[
    "curl",
    "wget",
    "nc",
    "ncat",
    "netcat",
    "ssh",
    "scp",
    "sftp",
    "rsync",
    "nslookup",
    "dig",
    "host",
    "ping",
    "traceroute",
    "telnet",
    "ftp",
];

/// Classify a shell command into a [`ShellPolicy`] category.
///
/// DR4-002: Prefix matching is case-insensitive (uses `to_ascii_lowercase()`).
///
/// Note: Unlike [`is_network_command`], this function intentionally does NOT
/// strip `sudo`/`env` prefixes via `extract_first_command()`. Commands like
/// `sudo git log` or `env cargo test` fall through to `General` (Confirm
/// required), which is a safe-side fallback. The prefix lists already contain
/// bare command names, so stripping wrappers is unnecessary for the common
/// case and would require a separate security review before adoption.
pub fn classify_shell_policy(command: &str) -> ShellPolicy {
    let trimmed = command.trim();
    let lower = trimmed.to_ascii_lowercase();

    // Reject command chaining / injection vectors → General
    if contains_injection_vectors(trimmed) {
        return ShellPolicy::General;
    }

    // gh api: GET-only is ReadOnly (DR2-010: preserve evaluation order)
    if is_safe_gh_api(&lower) {
        return ShellPolicy::ReadOnly;
    }

    // Category-based prefix matching (DR4-002: case-insensitive via lower)
    if matches_read_only_prefixes(&lower) {
        if has_dangerous_options(&lower) {
            return ShellPolicy::General;
        }
        return ShellPolicy::ReadOnly;
    }
    if matches_build_test_prefixes(&lower) {
        if has_dangerous_options(&lower) {
            return ShellPolicy::General;
        }
        return ShellPolicy::BuildTest;
    }

    ShellPolicy::General
}

/// Determine whether a shell command targets a network endpoint.
///
/// DR4-002: Case-insensitive (handles CURL, Wget, etc.)
pub fn is_network_command(command: &str) -> bool {
    let trimmed = command.trim();
    let lower = trimmed.to_ascii_lowercase();
    let first_cmd = extract_first_command(&lower);
    NETWORK_COMMAND_PREFIXES
        .iter()
        .any(|prefix| first_cmd == *prefix || first_cmd.ends_with(&format!("/{prefix}")))
}

/// Detect pipe / chain / injection vectors (shared function).
///
/// DR1-002: Extracted from the former inline checks in `is_safe_shell_command()`.
fn contains_injection_vectors(command: &str) -> bool {
    command.contains('|')
        || command.contains(';')
        || command.contains('`')
        || command.contains("$(")
        || command.contains("${")
        || command.contains('\n')
        || command.contains("&&")
        || command.contains('>')
        || command.contains('<')
}

/// Detect dangerous options that may launch external processes (DR2-009).
///
/// Moved from the former inline check in `is_safe_shell_command()`.
fn has_dangerous_options(command: &str) -> bool {
    let dangerous_options = ["--web", "--browse"];
    command
        .split_whitespace()
        .any(|token| dangerous_options.contains(&token))
}

/// Check if a command matches read-only prefixes.
fn matches_read_only_prefixes(lower_command: &str) -> bool {
    READ_ONLY_PREFIXES
        .iter()
        .any(|prefix| lower_command.starts_with(prefix))
}

/// Check if a command matches build/test prefixes.
fn matches_build_test_prefixes(lower_command: &str) -> bool {
    BUILD_TEST_PREFIXES
        .iter()
        .any(|prefix| lower_command.starts_with(prefix))
}

/// Check if a `gh api` command is safe (GET-only).
///
/// Moved from the former inline logic in `is_safe_shell_command()`.
fn is_safe_gh_api(lower_command: &str) -> bool {
    if !lower_command.starts_with("gh api ") {
        return false;
    }

    let tokens: Vec<&str> = lower_command.split_whitespace().collect();

    // Flags that imply a mutating request by their mere presence.
    // Input is already lowercased, so `-F` becomes `-f` (no separate entry needed).
    const BODY_FLAGS: &[&str] = &["-f", "--field", "--raw-field", "--input"];

    // Combined flag=value forms that imply mutation.
    const MUTATION_COMBINED: &[&str] = &[
        "-xpost",
        "-xput",
        "-xpatch",
        "-xdelete",
        "--method=post",
        "--method=put",
        "--method=patch",
        "--method=delete",
        "--input=",
        "-f=",
        "--field=",
        "--raw-field=",
    ];

    for (i, token) in tokens.iter().enumerate() {
        // Body/field flags always imply mutation.
        if BODY_FLAGS.iter().any(|f| token == f) {
            return false;
        }

        // --method / -x followed by a mutating HTTP verb.
        if (*token == "--method" || *token == "-x")
            && let Some(next) = tokens.get(i + 1)
            && ["post", "put", "patch", "delete"].contains(next)
        {
            return false;
        }

        // Combined forms (e.g. -xpost, --method=post, --input=file)
        if MUTATION_COMBINED.iter().any(|f| token.starts_with(f)) {
            return false;
        }
    }
    true
}

/// Extract the first actual command name from a command string (DR1-007).
///
/// Handles:
/// 1. VAR=value environment variable prefixes (e.g. `FOO=bar curl` -> `curl`)
/// 2. `sudo` / `env` command prefixes (e.g. `sudo curl` -> `curl`)
/// 3. `env` option arguments (`-i`, `-u`, etc.) skipping (DR4-001)
/// 4. Absolute path basename extraction (e.g. `/usr/bin/curl` -> `curl`)
fn extract_first_command(command: &str) -> &str {
    let mut words = command.split_whitespace();
    let mut after_env = false;
    loop {
        match words.next() {
            None => return "",
            Some(word) => {
                // Skip VAR=value patterns
                if word.contains('=') && !word.starts_with('-') {
                    continue;
                }
                // Skip env option arguments (DR4-001)
                if after_env && word.starts_with('-') {
                    continue;
                }
                // Skip sudo / env
                let base = word.rsplit('/').next().unwrap_or(word);
                if base == "sudo" {
                    continue;
                }
                if base == "env" {
                    after_env = true;
                    continue;
                }
                // Return basename
                return base;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- classify_shell_policy tests ---

    #[test]
    fn classify_read_only_git_log() {
        assert_eq!(
            classify_shell_policy("git log --oneline"),
            ShellPolicy::ReadOnly
        );
    }

    #[test]
    fn classify_read_only_git_status() {
        assert_eq!(classify_shell_policy("git status"), ShellPolicy::ReadOnly);
    }

    #[test]
    fn classify_read_only_gh_api_get() {
        assert_eq!(
            classify_shell_policy("gh api repos/o/r/stats"),
            ShellPolicy::ReadOnly
        );
    }

    #[test]
    fn classify_build_test_cargo_test() {
        assert_eq!(classify_shell_policy("cargo test"), ShellPolicy::BuildTest);
    }

    #[test]
    fn classify_build_test_cargo_build() {
        assert_eq!(classify_shell_policy("cargo build"), ShellPolicy::BuildTest);
    }

    #[test]
    fn classify_build_test_npm_test() {
        assert_eq!(classify_shell_policy("npm test"), ShellPolicy::BuildTest);
    }

    #[test]
    fn classify_general_curl() {
        assert_eq!(
            classify_shell_policy("curl https://example.com"),
            ShellPolicy::General
        );
    }

    #[test]
    fn classify_general_unknown() {
        assert_eq!(
            classify_shell_policy("some-unknown-command"),
            ShellPolicy::General
        );
    }

    #[test]
    fn classify_pipe_fallback() {
        assert_eq!(
            classify_shell_policy("git log | head -5"),
            ShellPolicy::General
        );
    }

    #[test]
    fn classify_chain_fallback() {
        assert_eq!(
            classify_shell_policy("cargo test && rm -rf /"),
            ShellPolicy::General
        );
    }

    #[test]
    fn classify_case_insensitive_git() {
        assert_eq!(
            classify_shell_policy("GIT LOG --oneline"),
            ShellPolicy::ReadOnly
        );
    }

    #[test]
    fn classify_case_insensitive_cargo() {
        assert_eq!(classify_shell_policy("Cargo Test"), ShellPolicy::BuildTest);
    }

    #[test]
    fn classify_dangerous_options_web() {
        assert_eq!(
            classify_shell_policy("gh repo view --web"),
            ShellPolicy::General
        );
    }

    #[test]
    fn classify_dangerous_options_browse() {
        assert_eq!(
            classify_shell_policy("gh issue list --browse"),
            ShellPolicy::General
        );
    }

    #[test]
    fn classify_gh_api_post_is_general() {
        assert_eq!(
            classify_shell_policy("gh api --method POST repos/o/r/issues"),
            ShellPolicy::General
        );
    }

    // --- is_network_command tests ---

    #[test]
    fn network_curl() {
        assert!(is_network_command("curl https://example.com"));
    }

    #[test]
    fn network_wget() {
        assert!(is_network_command("wget https://example.com"));
    }

    #[test]
    fn network_ssh() {
        assert!(is_network_command("ssh user@host"));
    }

    #[test]
    fn network_ping() {
        assert!(is_network_command("ping 8.8.8.8"));
    }

    #[test]
    fn not_network_ls() {
        assert!(!is_network_command("ls -la"));
    }

    #[test]
    fn not_network_git() {
        assert!(!is_network_command("git log"));
    }

    #[test]
    fn not_network_cargo() {
        assert!(!is_network_command("cargo test"));
    }

    #[test]
    fn network_case_insensitive_curl() {
        assert!(is_network_command("CURL https://example.com"));
    }

    #[test]
    fn network_case_insensitive_wget() {
        assert!(is_network_command("Wget https://example.com"));
    }

    // --- extract_first_command tests ---

    #[test]
    fn extract_simple() {
        assert_eq!(extract_first_command("curl https://example.com"), "curl");
    }

    #[test]
    fn extract_env_prefix() {
        assert_eq!(
            extract_first_command("FOO=bar curl https://example.com"),
            "curl"
        );
    }

    #[test]
    fn extract_sudo() {
        assert_eq!(
            extract_first_command("sudo curl https://example.com"),
            "curl"
        );
    }

    #[test]
    fn extract_env_command() {
        assert_eq!(
            extract_first_command("env curl https://example.com"),
            "curl"
        );
    }

    #[test]
    fn extract_env_options() {
        assert_eq!(
            extract_first_command("env -i curl https://example.com"),
            "curl"
        );
    }

    #[test]
    fn extract_env_u_option() {
        // Known limitation (DR4-001): `-u VAR` is an option with argument.
        // Only the `-u` token (starting with `-`) is skipped; `VAR` is returned
        // as the command. This is a false extraction but safe: the command won't
        // match any prefix list and will fall to General (Confirm required).
        assert_eq!(
            extract_first_command("env -u VAR curl https://example.com"),
            "VAR"
        );
    }

    #[test]
    fn extract_absolute_path() {
        assert_eq!(
            extract_first_command("/usr/bin/curl https://example.com"),
            "curl"
        );
    }

    #[test]
    fn extract_empty() {
        assert_eq!(extract_first_command(""), "");
    }

    #[test]
    fn extract_sudo_env_combined() {
        assert_eq!(
            extract_first_command("sudo env -i curl https://example.com"),
            "curl"
        );
    }

    // --- contains_injection_vectors tests ---

    #[test]
    fn injection_pipe() {
        assert!(contains_injection_vectors("ls | grep foo"));
    }

    #[test]
    fn injection_semicolon() {
        assert!(contains_injection_vectors("ls; rm -rf /"));
    }

    #[test]
    fn injection_backtick() {
        assert!(contains_injection_vectors("echo `whoami`"));
    }

    #[test]
    fn injection_dollar_paren() {
        assert!(contains_injection_vectors("echo $(whoami)"));
    }

    #[test]
    fn injection_dollar_brace() {
        assert!(contains_injection_vectors("echo ${HOME}"));
    }

    #[test]
    fn injection_newline() {
        assert!(contains_injection_vectors("ls\nrm -rf /"));
    }

    #[test]
    fn injection_and_chain() {
        assert!(contains_injection_vectors("ls && rm -rf /"));
    }

    #[test]
    fn injection_redirect_out() {
        assert!(contains_injection_vectors("echo foo > file"));
    }

    #[test]
    fn injection_redirect_in() {
        assert!(contains_injection_vectors("cat < file"));
    }

    #[test]
    fn no_injection_simple() {
        assert!(!contains_injection_vectors("git log --oneline"));
    }

    // --- has_dangerous_options tests ---

    #[test]
    fn dangerous_web() {
        assert!(has_dangerous_options("gh repo view --web"));
    }

    #[test]
    fn dangerous_browse() {
        assert!(has_dangerous_options("gh issue list --browse"));
    }

    #[test]
    fn not_dangerous_json() {
        assert!(!has_dangerous_options("gh repo view --json owner"));
    }

    // --- network command with sudo/env ---

    #[test]
    fn network_sudo_curl() {
        assert!(is_network_command("sudo curl https://example.com"));
    }

    #[test]
    fn network_env_wget() {
        assert!(is_network_command("env wget https://example.com"));
    }

    #[test]
    fn network_absolute_path_curl() {
        assert!(is_network_command("/usr/bin/curl https://example.com"));
    }
}
