# Changelog

## [Unreleased]

## [0.0.5] - 2026-03-21

### Added
- Repository search improvement with gitignore support and ranking (#116)
- Tool result handling improvement for long outputs (#117)
- Optional tools exposure in system prompt before first use (#115)
- Cache invalidation strengthening with per-entry manifest hashing (#118)
- Release skill update to use git worktree + commandmatedev

### Fixed
- Web search/fetch inclusion in system prompt for fresh sessions (#114)
- Clippy map_or → is_some_and style fix

## [0.0.4] - 2026-03-20

### Added
- Smart context compression with token-based eviction (#80)
- TUI elapsed_ms display and tool execution status improvement (#81)
- Model-based token estimation calibration (#79)
- Native reqwest HTTP client replacing curl subprocess (#78)
- DuckDuckGo web.search robustness improvement (#93)
- Current date/timezone injection into system prompt (#92)
- Confirm-class tool approval guidance in system prompt
- Dynamic system prompt generation (#73)
- @file context injection (#76)
- Tag-based tool call protocol for small LLMs (#72)
- Model management UI: /model list, switch, info (#77)
- Git tools: git.status, git.diff, git.log (#75)
- Regex search and ripgrep integration for file.search (#74)
- Named session management (#71)
- Batch approval with --trust mode (#70)
- Multi-file atomic edit with transaction rollback (#69)
- Undo/rollback with checkpoint stack (#68)
- Auto-detect context window from Ollama /api/show (#65)
- Inference performance metrics display (#66)
- Offline mode for complete local operation (#67)
- User-friendly error UX with guidance (#64)

### Fixed
- Duplicate [U] you > prompt in interactive CLI (#96)
- UTF-8 boundary panic with chars-based truncation (#94)
- ModelNotFound error propagation with user guidance (#86)
- No-double-confirm for already-approved tools (#95)

## [0.0.3] - 2026-03-19

### Added
- Sub-agent mechanism for parallel task execution (#24)
- Lifecycle hooks system (#25)
- Model Context Protocol (MCP) support with STDIO transport (#23)
- SKILL.md-based skills system - Phase 1 (#22)
- Non-interactive exec mode for CI/CD integration (#27)
- Multimodal image support for Vision APIs (#26)
- Diff preview for file.write/file.edit approval prompts (#21)
- --help and --version flags via clap crate (#20)
- Context overflow warning with /compact suggestion (#19)
- Parallel tool execution for ParallelSafe tools (#18)
- Security warnings for API keys in config file (#17)
- Graceful shutdown via signal handling (#14)
- Health check and retry mechanism for providers (#13)
- Startup configuration validation (#16)
- Atomic write for session persistence (#15)
- Token count accuracy improvement with CJK support (#11)
- Structured logging with tracing crate (#12)
- ANVIL.md project instructions file support (#10)
- file.edit tool for partial file editing (#9)
- shell.exec guide and command permission policy (#8)
- /uat command for user acceptance testing
- Orchestration commands (orchestrate, pr-merge-pipeline, uat-fix-loop)

### Fixed
- Test ETXTBSY issues on CI with create_test_script helper

## [0.0.2] - 2026-03-16

### Added
- web.search tool and GitHub Insights support (#6)
- web.fetch tool for HTTP content retrieval (#2)
- Release skills and GitHub Actions release workflow (#34)
- Issue-enhance, pm-auto-issue2dev, pm-auto-design2dev slash commands
- Port remaining slash commands from MyCodeBranchDesk

### Fixed
- Duplicate response output in assistant messages (#1)
- Exclude all messages from live-turn frames and skip intermediate frames (#1)

### Documentation
- Restructure README with user-focused quick start and developer section
- Add binary download instructions to README

## [0.0.1] - 2026-03-15

### Added
- Agentic tool-use loop with multi-turn LLM interaction
- Tool system: file.read, file.write, file.search, shell.exec
- Inline y/n approval prompt for Confirm-class tools
- Real-time streaming of shell.exec output to terminal
- Ollama and OpenAI-compatible provider backends
- Session persistence with dirty-flag batched writes
- Auto session compaction when message threshold exceeded
- Token count caching with Cell-based O(1) lookups
- Configurable context budget (removes hard 8K clamp)
- "Did you mean?" command suggestions for typos
- Actionable error guidance for startup failures
- Symlink sandbox escape prevention (canonicalize check)
- Dangerous command blocking (rm -rf, mkfs, dd, fork bombs)
- HTTP request timeout (ANVIL_HTTP_TIMEOUT, with ANVIL_CURL_TIMEOUT fallback)
- Configurable limits: max_agent_iterations, max_console_messages, auto_compact_threshold, tool_result_max_chars
- Graceful fallback when LLM produces unparseable follow-up output
- Interactive line editing with rustyline (arrow keys, history)
- Spinner with real-time token streaming
- Plan management (/plan, /plan-add, /plan-focus, /plan-clear)
- Repository search (/repo-find)
- Session timeline (/timeline)
- Session compaction (/compact)
- Issue-driven development infrastructure (CLAUDE.md, slash commands, agents, CI)

### Architecture
- app/mod.rs split into agentic.rs, plan.rs, cli.rs (-31%)
- provider/mod.rs split into ollama.rs, transport.rs, openai.rs (-70%)
- Consolidated pending_turn to single source of truth (SessionRecord)
- transition_with_context helper reducing state transition boilerplate
- Optimized file.search with line-based BufReader and directory skipping

### Quality
- 108 tests (unit + integration)
- 0 clippy warnings
- cargo fmt applied to all files
- CI workflow: fmt, clippy, test, build (parallel jobs)
