# Changelog

## [Unreleased]

## [0.0.8] - 2026-03-31

### Added
- ANVIL.md custom tool registration with `## tools` section (#193)
- Sidecar model LLM-based context summarization (#195)
- Read repeat tracker to detect and warn repeated file.read (#185)
- Write repeat tracker to prevent file.edit→file.write loop (#184)
- Duplicate tool call deduplication in single turn (#186)
- Debug observability with turn summaries, file.edit details, and session metrics (#206)
- Cause-analysis and current-situation slash commands

### Fixed
- Raise thresholds to reduce CI-induced implementation suppression (#187)
- Use `min(context_window, context_budget)` for auto-compact threshold (#200)
- Use `config.runtime.context_budget` in `derive_context_budget()` (#198)
- Run auto-compact in non-interactive mode (`--exec-file`/`--exec`/`--oneshot`) (#202)
- Add `max_output_tokens` to prevent infinite LLM stream (#204)
- Use `context_budget` instead of `context_window` in turn summary budget display (#208)

### Changed
- Improve sidecar summarization prompt for coding tasks (#209)
- Enforce Codex reviewer via `--agent codex` in multi-stage review commands

## [0.0.7] - 2026-03-27

### Added
- File read cache with mtime-based invalidation and LRU eviction (#142, #174)
- Loop detection and self-correction with ring buffer and staged response (#145)
- ANVIL_FINAL early-fire guard to prevent plan-only outputs (#144)
- File.edit error recovery with 3-level fallback and context hints (#143)
- `--timeout` option for external timeout configuration (#146)
- Large file write protection with configurable threshold (#156)
- File.write consecutive failure retry limit (#161)
- Context reset state carry-over with working memory injection (#157)
- Unified edit/write fallback strategy with configurable thresholds (#158)
- File.edit diff feedback to LLM after successful edits (#155)
- Model-independent phase control with fallback completion detection (#159)
- UI language stability with configurable language code (#162)
- File.search loop detection and max tool calls limit (#172)
- File.search root path handling improvement (#175)
- Large file edit fallback with size-aware thresholds (#176)
- Post-ANVIL_FINAL tool call filtering to prevent continuation (#160, #173)

### Changed
- `max_agent_iterations` / `subagent_max_iterations` default values increased (#147)

## [0.0.6] - 2026-03-21

### Added
- Prompt tiering and model capability classification (#132)
- Multi-tier tool protocol and resilient editing (#128)
- Structured working memory for long-session context retention (#130)
- Sub-agent redesign for structured exploration payload (#129)
- ShellPolicy classification and offline network command blocking (#131)
- Retrieval scoring upgrade with 2-pass, keyword split, and boost for large repos (#133)

### Changed
- Codex code review phase added between TDD and acceptance test
- Delegate 2nd-round reviews to Codex via commandmatedev

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
