# Changelog

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
- Curl request timeout (ANVIL_CURL_TIMEOUT)
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
