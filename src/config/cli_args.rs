//! CLI argument definitions using `clap` derive macros.
//!
//! [`CliArgs`] is parsed in `main.rs` and passed to
//! [`super::EffectiveConfig::load_with_args`] for configuration override.

use clap::Parser;
use std::path::PathBuf;

#[derive(Parser, Debug, Default)]
#[command(name = "anvil", version, about = "Local coding agent powered by LLM")]
pub struct CliArgs {
    /// Provider (ollama|openai)
    #[arg(short = 'p', long)]
    pub provider: Option<String>,

    /// LLM model name
    #[arg(short = 'm', long)]
    pub model: Option<String>,

    /// Provider URL
    #[arg(short = 'u', long = "provider-url")]
    pub provider_url: Option<String>,

    /// Sidecar model name
    #[arg(long = "sidecar-model")]
    pub sidecar_model: Option<String>,

    /// Context window size
    #[arg(long = "context-window")]
    pub context_window: Option<u32>,

    /// Context budget (token count)
    #[arg(long = "context-budget")]
    pub context_budget: Option<u32>,

    /// Max agent iterations
    #[arg(long = "max-iterations")]
    pub max_iterations: Option<usize>,

    /// Disable streaming
    #[arg(long = "no-stream")]
    pub no_stream: bool,

    /// Enable debug logging
    #[arg(long)]
    pub debug: bool,

    /// Skip tool execution approval
    #[arg(long = "no-approval")]
    pub no_approval: bool,

    /// Force new session
    #[arg(long = "fresh-session")]
    pub fresh_session: bool,

    /// Non-interactive mode (read prompt from stdin)
    #[arg(long, conflicts_with_all = ["exec", "exec_file"])]
    pub oneshot: bool,

    /// Execute a single prompt and exit (non-interactive)
    #[arg(long, conflicts_with_all = ["exec_file", "oneshot"])]
    pub exec: Option<String>,

    /// Execute prompt from a file and exit (non-interactive)
    #[arg(long = "exec-file", conflicts_with_all = ["exec", "oneshot"])]
    pub exec_file: Option<PathBuf>,

    /// Reasoning visibility level (hidden|summary)
    #[arg(long = "reasoning-visibility")]
    pub reasoning_visibility: Option<String>,

    /// Run in offline mode (disable web tools and MCP)
    #[arg(long)]
    pub offline: bool,

    /// Named session to use
    #[arg(long)]
    pub session: Option<String>,

    /// Auto-approve built-in tool execution (MCP tools require individual trust)
    #[arg(long)]
    pub trust: bool,

    /// Enable tag-based tool protocol (--tag-protocol).
    #[doc(hidden)]
    #[arg(long = "tag-protocol")]
    pub tag_protocol_flag: bool,

    /// Force JSON tool protocol (--no-tag-protocol).
    #[doc(hidden)]
    #[arg(long = "no-tag-protocol")]
    pub no_tag_protocol_flag: bool,

    /// Computed tag_protocol value: Some(true) for --tag-protocol,
    /// Some(false) for --no-tag-protocol, None when neither is specified.
    /// Not parsed from CLI args directly.
    #[arg(skip)]
    pub tag_protocol: Option<bool>,
}

impl CliArgs {
    /// Resolve the `tag_protocol` field from the raw CLI flags.
    /// Call this after `clap::Parser::parse()` to compute the final value.
    pub fn resolve_tag_protocol(&mut self) {
        if self.tag_protocol_flag {
            self.tag_protocol = Some(true);
        } else if self.no_tag_protocol_flag {
            self.tag_protocol = Some(false);
        }
        // Otherwise remains None (auto-detect)
    }
}
