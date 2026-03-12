# vibe-local Analysis Memo

## Positioning

This document is an implementation-observation memo based on the codebase at `/Users/maenokota/share/work/github_kewton/vibe-local` as inspected on March 12, 2026.

It is intentionally split into three layers:

- `Facts`: directly observed from repository contents and implementation
- `Inferences`: reasoned conclusions derived from those facts
- `Implications for Anvil`: design consequences for this project

The goal is to keep code observation separate from evaluation and strategy.

## Facts

### Repository Shape

- The repository is physically small and centered on one dominant implementation file: `vibe-coder.py`.
- Top-level supporting files include `README.md`, `ROADMAP.md`, `RELEASE_NOTES.md`, platform launchers, tests, and an auxiliary `anthropic-ollama-proxy.py`.
- Most application logic is implemented in one Python process and one file rather than a multi-package structure.

### Runtime Entry and Startup

- `main()` creates `Config`, initializes `TUI`, checks Ollama connectivity, validates model availability, builds the system prompt, loads skills, optionally initializes RAG, registers tools, initializes MCP servers, creates `Agent`, and enters either one-shot or interactive mode.
- The startup path includes automatic Ollama start attempts on macOS and Linux when Ollama is not already running.
- If the configured model is missing, the app can prompt to pull it through Ollama.

### Configuration Model

- `Config` loads values from config file, environment variables, and CLI arguments in that order.
- `Config` auto-detects a default main model from installed Ollama models plus available RAM and VRAM.
- `Config` also selects a sidecar model when possible.
- `Config` maps known models to context window sizes.
- `Config` enforces localhost-only `OLLAMA_HOST`.
- `Config` creates config and state directories and migrates legacy `vibe-coder` state paths into `vibe-local`.

### System Prompt Construction

- The system prompt is assembled dynamically in `_build_system_prompt`.
- Prompt content includes core agent operating rules, environment metadata, OS-specific shell guidance, global instructions, and project instructions discovered from parent directories.
- Skill markdown files are loaded separately and appended into the system prompt at startup.
- Project instruction content is sanitized to strip tool-call-like XML patterns before inclusion.

### Ollama Integration

- `OllamaClient` talks to Ollama through native endpoints such as `/api/chat`, `/api/tags`, `/api/version`, `/api/pull`, and `/api/tokenize`.
- The client converts between Ollama-native message format and an OpenAI-style function-calling structure used internally by the rest of the app.
- The client detects whether tool streaming is supported by the installed Ollama version.
- Requests set `keep_alive = -1`.
- For tool-calling requests, the client lowers temperature to at most `0.3`.

### Session and Context Management

- `Session` persists history as JSONL files under the local state directory.
- The app auto-saves sessions after each interaction and on exit.
- `Session` maintains a project-to-session index keyed by a hash of the current working directory.
- Token accounting inside `Session` is primarily based on internal estimation logic with CJK-aware heuristics.
- `Session` supports compaction when context grows large.
- When a sidecar model is available, compaction can summarize older messages via the sidecar model.
- `Session` preserves tool call and tool result structure as separate messages.

### Terminal UI

- `TUI` uses ANSI escapes and standard input handling rather than an external TUI framework.
- The UI includes a banner, slash-command completion, token usage display, markdown-like rendering, permission prompts, and tool call/result displays.
- The app implements a fixed footer via a `ScrollRegion` abstraction using DECSTBM terminal scroll regions.
- The app supports ESC interrupt and type-ahead capture during streaming output.
- There are explicit tests for scroll-region behavior and PTY-level terminal behavior.

### Tool System

- Tools are represented as classes with schemas compatible with OpenAI-style function calling.
- Built-in tools include `Bash`, `Read`, `Write`, `Edit`, `Glob`, `Grep`, `WebFetch`, `WebSearch`, `NotebookEdit`, task tools, `AskUserQuestion`, `SubAgent`, and `ParallelAgents`.
- Built-in tool schemas are registered in `ToolRegistry`, then dynamic MCP tools are added at startup.
- Tool outputs are mostly string-based.
- `Read` has special handling for images, PDFs, and notebooks.
- `Bash` sanitizes environment variables and blocks several dangerous shell patterns.
- `Write` and `Edit` are treated as side-effecting operations and can trigger checkpointing and auto-tests.
- The runtime distinguishes between safe tools, confirmation-required tools, and network tools through `PermissionMgr`.
- Only a subset of tools are eligible for parallel execution, and side-effecting tools remain sequential.

### Tool Call Recovery and Agent Loop

- `Agent.run()` drives an iterative loop of model call, tool call parsing, tool execution, tool result insertion, and another model call.
- The loop has a maximum iteration limit.
- The app accepts both structured tool calls and XML-style fallback tool calls extracted from assistant text.
- It includes JSON repair and salvage paths for malformed tool arguments.
- Read-only safe tools may be executed in parallel.
- Side-effecting tools are executed sequentially.
- Tool-call repetition is tracked to stop obvious infinite repetition loops.

### Permission and Safety Model

- `PermissionMgr` divides tools into safe, ask, and network categories.
- Safe tools are auto-approved unless session rules override them.
- Ask and network tools go through user confirmation unless `yes_mode` is enabled.
- Even in `yes_mode`, certain dangerous bash patterns still require confirmation.
- The code contains path sanitization, symlink checks, protected path checks, localhost restriction for Ollama host, and several shell-pattern blocks.
- Git checkpoint and rollback functionality is implemented with `git stash`.
- In plan mode, `Write` is restricted to the plans directory until the user switches to act mode.
- The agent pads missing tool results on interruption so assistant tool-call history remains structurally valid.

### Extended Capability Layers

- MCP servers are loaded from config and project-local JSON files, then exposed as tools.
- Skills are loaded from markdown files in global and project-level directories.
- File watching is implemented through polling.
- Auto-test can run syntax checks and inferred test commands after edits.
- Local RAG is implemented with Ollama embeddings plus SQLite storage and brute-force cosine similarity.
- Parallel multi-agent execution is implemented with threads and sub-agent instances.

### Tests and Validation Artifacts

- `tests/test_vibe_coder.py` contains many unit-level tests covering config, safety, parsing, and runtime behaviors.
- `test_tui_pty.py` validates terminal behavior through a minimal VT100 emulator and PTY interaction.
- `test_scroll_region.py` is a standalone diagnostic for the DECSTBM-based footer behavior.
- The test suite is especially concentrated around configuration behavior, safety checks, parsing fallbacks, and terminal footer correctness.
- Terminal rendering behavior is not only implemented but explicitly regression-tested, which is unusual for a small CLI codebase.

## Inferences

### Architectural Character

- `vibe-local` is functionally ambitious but physically centralized. `Evidence strength: strong`
- The main design pattern is to accumulate features inside one runtime rather than isolate them behind strong module boundaries. `Evidence strength: strong`
- The codebase is optimized more for distribution simplicity and inspectability than for long-term internal separation. `Evidence strength: moderate`

### Product Intent

- The repository appears intentionally optimized for low-friction local/offline use rather than for maximum architectural purity. `Evidence strength: strong`
- The implementation choices align with the README positioning around education, research, and offline operation. `Evidence strength: strong`
- Several "rough" design choices look deliberate rather than accidental, especially stdlib-only implementation and direct ANSI terminal control. `Evidence strength: moderate`

### Local-LLM Alignment

- The runtime is more aligned with local LLM constraints than many generic coding agents that merely support local providers. `Evidence strength: moderate`
- The code repeatedly assumes imperfect tool-calling behavior and includes explicit recovery paths for malformed outputs. `Evidence strength: strong`
- Sidecar summarization, context compaction, low dependency overhead, and direct Ollama control suggest the product is tuned for practical local usage rather than cloud-first assumptions. `Evidence strength: strong`

### UX Quality

- `vibe-local` invests meaningfully in terminal UX despite not using a rich TUI framework. `Evidence strength: strong`
- Features such as fixed footer, ESC interrupt, type-ahead, slash commands, and session resume indicate product attention beyond a barebones CLI. `Evidence strength: strong`
- The current UX is likely stronger than what the repository shape alone would suggest. `Evidence strength: moderate`

### Reliability Profile

- The code shows a pragmatic reliability strategy: permission prompts, shell blocking, rollback, auto-save, loop detection, and compaction. `Evidence strength: strong`
- This likely improves user experience with weaker or less predictable local models. `Evidence strength: moderate`
- Stability here appears to come from layered guardrails rather than from a deeply typed or formally structured architecture. `Evidence strength: strong`

### Performance Profile

- The implementation likely has low deployment friction and relatively low framework overhead. `Evidence strength: moderate`
- However, the Python runtime, monolithic process structure, heavy string handling, and text-based tool interfaces likely place a ceiling on raw runtime efficiency and future scaling. `Evidence strength: moderate`
- RAG and retrieval are functional for smaller scopes but likely not optimized for very large corpora or very large repositories. `Evidence strength: strong`

### Maintainability Profile

- The single-file core almost certainly increases coupling and makes future feature growth more expensive. `Evidence strength: strong`
- The conceptual subsystem count is high relative to the physical module separation. `Evidence strength: strong`
- This suggests feature velocity may eventually be constrained by coordination cost inside the monolith. `Evidence strength: moderate`

### Competitive Interpretation

- `vibe-local` looks strongest when evaluated as a local-first runtime wrapper around imperfect local models. `Evidence strength: moderate`
- Its strongest differentiation appears to be operational pragmatism rather than raw algorithmic novelty. `Evidence strength: moderate`
- It is reasonable to hypothesize that it can outperform more cloud-assumption-heavy agents in some local/offline workflows. `Evidence strength: weak`
- It is not justified from code inspection alone to claim category-wide No.1 status on speed, quality, or stability.
- Any such claim would require benchmark definitions and empirical comparison.

### Competitive Axes Hypothesis

The implementation suggests that `vibe-local` is most competitive on these axes:

- time-to-first-usable-session on a local machine
- resilience to malformed or weak tool-calling behavior
- safety for interactive local execution with weaker models
- terminal UX quality relative to its dependency footprint

The implementation suggests it is less likely to lead on these axes:

- raw runtime throughput
- large-repository retrieval sophistication
- long-term architectural extensibility
- richer structured UX beyond the current terminal model

### Strength Hypothesis

Based on the implementation, the strongest case for `vibe-local` is:

- high usability per unit of local-model quality
- strong resilience to malformed tool-calling
- low setup and low operational friction
- better-than-expected terminal interaction quality for an offline local agent

### Weakness Hypothesis

Based on the implementation, the strongest weakness hypotheses are:

- maintainability ceiling from the monolithic structure
- raw performance ceiling from the combination of Python, string-heavy tool transport, single-process coordination, ANSI-state-heavy UI handling, and brute-force retrieval
- limited architectural headroom for richer UX and more typed state management
- limited retrieval sophistication for large-scale long-context codebase work

## Implications for Anvil

### Priority 1: Preserve or Exceed

Priority rule:

- `Priority 1` means either fundamental to local-agent usefulness or expensive to retrofit later.
- `Priority 2` means important, but deferrable if the initial architecture leaves a clean insertion point.

- Keep a local-first assumption set rather than treating local inference as an optional backend.
- Design explicitly for imperfect tool-calling and malformed structured output.
- Preserve session persistence, rollback, permission gating, and context hygiene as first-class capabilities.
- Use model-aware routing and sidecar models where they improve cost, latency, or context management.

### Priority 1: Improve Immediately

- Replace the single-file monolith with explicit subsystem boundaries.
- Replace string-heavy internal interfaces with typed events, typed tool payloads, and typed results.
- Improve raw runtime efficiency through Rust process management, concurrency control, and lower-overhead streaming.
- Build the terminal architecture so user input, agent output, status, and commands are clearly separated from day one.

### Priority 2: Improve Early

- Improve long-context and retrieval handling beyond brute-force SQLite-based local RAG.
- Build a more extensible terminal architecture so richer UX can be added without terminal-state fragility dominating the design.
- Treat execution policy as a first-class subsystem: safe/ask/network classes, parallel-safe classes, plan-mode restrictions, and interruption recovery should be explicit in the design.

### Architectural Targets Suggested by This Analysis

- A provider layer that still supports tight local-runtime control like `vibe-local`, but behind a cleaner abstraction.
- An agent loop designed around recovery from malformed model outputs, not only ideal tool-calling behavior.
- A session layer that separates persistence, compaction, summarization, and message shaping cleanly.
- A tool system with strong schemas and typed internal contracts.
- A TUI layer that preserves immediacy but supports clearer separation of user input, agent output, status, and commands.
- An extension model for slash commands, skills, MCP, and future UX features that does not require prompt growth as the main scaling mechanism.

### Competitive Goal Framing for Anvil

If Anvil aims to beat `vibe-local` for local LLM coding use, the likely winning path is:

- match or exceed its speed of local setup and first-use experience
- match or exceed its resilience to weak local model behavior
- exceed it on runtime efficiency
- exceed it on architectural extensibility
- exceed it on structured UX clarity
- exceed it on large-context and large-repository handling

Suggested measurable comparison axes:

- first-launch setup effort and time-to-first-usable-session
- first prompt latency and steady-state iteration latency
- malformed tool-call recovery success rate
- interruption safety and session survival rate
- context reuse effectiveness across long sessions
- repo-scale retrieval quality and latency

### Advantage-to-Response Mapping

| vibe-local advantage or weakness | Why it matters | Anvil response |
|---|---|---|
| Fast local onboarding and low setup friction | Determines adoption and first impression | Keep install/runtime path simple even with Rust architecture |
| Strong recovery from malformed tool-calling | Critical for imperfect local models | Make malformed-output recovery a first-class agent-loop concern |
| Practical safety model with approval, rollback, and interruption handling | Improves trust and reduces blast radius | Implement execution policy, rollback, and interruption recovery as core subsystems |
| Surprisingly capable terminal UX with minimal stack | Raises usability without cloud dependencies | Build a clearer and more extensible terminal UX while preserving immediacy |
| Monolithic structure | Limits maintainability and future growth | Enforce subsystem boundaries early |
| String-heavy tool transport | Adds parsing and correctness overhead | Use typed tool payloads and typed internal events |
| Basic brute-force local RAG | Caps large-repo reasoning quality and scale | Build stronger long-context and retrieval architecture |

### Practical Conclusion

`vibe-local` should be treated as a serious baseline for local-first coding agents.
Its strongest lesson is that the surrounding runtime can materially improve the usefulness of imperfect local models.

Anvil should not copy its physical architecture.
It should copy the local-first assumptions, safety posture, and resilience patterns, then reimplement those ideas in a more explicit and extensible Rust architecture.
