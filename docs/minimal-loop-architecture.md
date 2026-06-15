# Minimal Loop Architecture

This document records the current minimal-loop architecture and the design
intent behind it. It is descriptive, not a new roadmap. The operating rules for
triage, mechanism admission, ablation, and Phase 4 remain in
`docs/minimal-loop-operating-discipline.md`.

## Intent

Minimal loop is the strangler replacement for the legacy `loop_run` controller.
The goal is not to rebuild every legacy guard in a smaller syntax. The goal is
to keep the core agent loop simple enough that local Ollama models can follow
it, then add only mechanisms that were earned by failure triage and measured
improvement.

The key bet is:

- The model should generate code and decide local implementation details.
- Rust should handle deterministic facts: path confinement, tool execution,
  parent directory creation, parser mode transitions, bounded verification, and
  clear stop reasons.
- When local repair is exhausted, Anvil should surface evidence and let the user
  explicitly replan, not silently start a larger controller.

## High-Level Shape

```
CLI / REPL
  -> minimal_repl.rs
      interactive commands:
        plain prompt
        /plan-steps
        /plan-run
        /ultra-plan
        /ultra-plan-run
        /run-plan
        /run-ultra-plan

  -> minimal_loop/
      prompt.rs     fixed prompt and tool catalog
      loop_run.rs   one session loop and runtime guards
      compact.rs    token-oriented compaction
      feedback.rs   ephemeral feedback text

  -> minimal_step_runner.rs
      plan generation and orchestration
      minimal_step_runner/
        verify.rs      deterministic step checks
        repair.rs      bounded local repair and exhausted-report generation
        profile.rs     ultra profile contracts and workspace snapshots
        plan_lint.rs   plan sanity checks

  -> planner_llm.rs
      planner-only LLM adapter boundary
      current implementation: Ollama
      planned extension point: non-tool planner providers such as Gemini

  -> shared infrastructure
      src/ollama/      Ollama client and parser/xml fallback
      src/tools/       Bash/Read/Write/Edit/Glob/Grep
      src/session/     persisted session snapshots
```

The production minimal loop is intentionally smaller than the step runner. The
step runner is an outer workflow tool for larger tasks; it must not become a
second legacy controller hidden inside the core loop.

## Core Loop

`src/agent/minimal_loop/loop_run.rs` owns one conversational session turn loop.
Its responsibilities are deliberately narrow:

- build one request with one system prompt;
- choose native tool calls or XML fallback for the whole session;
- execute tool calls through the shared tool registry;
- keep feedback ephemeral instead of storing it as permanent system messages;
- stop on a no-tool final answer, `max_iterations`, cancellation, or a bounded
  runtime guard failure.

The system prompt in `prompt.rs` is fixed and short. It states the final-answer
contract directly: if the assistant says it will create, edit, read, or verify
something, it must call the tool in the same response. A final answer describes
completed work, not planned next steps.

## Runtime Guards

Runtime guards are allowed only when they are based on deterministic facts. The
current important guards are:

- **Completion without write**: if a task reaches a final answer without any
  Write/Edit, send one neutral feedback asking the model to act if the task
  requires file changes.
- **Requested artifact feedback**: if the prompt explicitly names safe
  repository-relative artifacts and they are missing at completion, send one
  feedback listing the missing paths.
- **Planned action without tool**: if Act mode receives a no-tool answer such as
  "Now I will create..." or "Let me verify...", reject it up to a small bound.
- **Missing relative imports**: after source changes, unresolved relative
  imports are reported before accepting final completion.
- **Early success paths**: step runner turns can stop as soon as deterministic
  expected paths exist.

These are not broad intent classifiers. They check observable state in the
workspace or observable assistant wording at the completion boundary.

## Tool Affordances

The tool catalog is part of the architecture. It should remove avoidable traps
instead of teaching the model to work around them.

- Write creates parent directories automatically.
- Bash is for read-only inspection, build/test, and local script validation. It
  is not presented as a directory creation or file creation tool.
- Offline Bash blocks networked, mutating, or general shell commands, but local
  validation such as `python3 check.py` and safe `cd <cwd-child> && <script>` is
  handled as an affordance, not a benchmark trap.
- Tool errors should name the local alternative when there is a deterministic
  one, for example using Write for file creation.

## Step Runner

`/plan-run` and `/ultra-plan-run` exist for tasks that are too large for a
single local-model turn.

Plan generation and plan execution may use different models. `--model` remains
the execution model for the minimal loop and tool use. `--planner-model` selects
the model used only to generate `/plan-steps`, `/plan-run`, `/ultra-plan`, and
`/ultra-plan-run` plans; when omitted, it defaults to `--model`.

The provider boundary is intentionally narrow. `minimal_llm.rs` abstracts only
the minimal chat call, and `planner_llm.rs` abstracts only planner chat. Tool
execution, XML fallback parsing, runtime guards, sessions, and verifier/repair
logic remain shared Anvil code. Ollama may use native tool calls for allowlisted
models; Gemini and OpenAI use the XML fallback contract only. This keeps
provider support small without rebuilding the legacy multi-provider control
layer.

Current CLI shape:

```sh
anvil --engine minimal \
  --provider ollama \
  --model qwen3.6:27b-coding-nvfp4 \
  --planner-model qwen3.5:122b \
  --ultra-plan-run "Build the app"
```

For Gemini:

```sh
anvil --engine minimal \
  --provider gemini \
  --model gemini-3.1-flash-lite \
  --planner-provider gemini \
  --planner-model gemini-3.5-flash \
  --ultra-plan-run "Build the app"
```

`GEMINI_API_KEY` is read from the environment or a `.env` file near the
workspace/current directory.

For OpenAI/GPT planning with Gemini execution:

```sh
anvil --engine minimal \
  --provider gemini \
  --model gemini-3.1-flash-lite \
  --planner-provider openai \
  --planner-model gpt-5.4-mini \
  --ultra-plan-run "Build the app"
```

`OPENAI_API_KEY` is read from the environment or a `.env` file near the
workspace/current directory. When `--planner-provider` is omitted, it defaults
to `--provider`.

`/plan-run`:

- asks the model to produce a JSON step plan;
- saves it under `.anvil/plans/`;
- runs each step with expected paths and optional verify commands;
- applies bounded local repair when deterministic verification fails.

`/ultra-plan-run`:

- asks for a top-level phase plan;
- runs each phase as a separate `/plan-run`;
- carries a workspace snapshot and a profile contract into each phase;
- is explicit user-facing orchestration, not automatic hidden escalation.

Supported ultra profiles are:

- `generic`
- `nextjs`
- `python`
- `rust`
- `investigation`
- `docs`
- `data-analysis`
- `data-pipeline`

Supported styles are:

- `default`
- `tdd`
- `test-hardening`

Profiles are contracts and snapshots, not hard-coded task templates. They should
preserve existing project structure and protect important inputs, but they must
not turn into large domain-specific controllers.

## Verification And Repair

Step verification is deterministic and local:

- expected path existence;
- safe command allowlist such as `npm run build`, `cargo check`, `cargo test`,
  `python3 -m py_compile`, `python3 script.py`, `pytest`, `cat`, and
  `node --check` for JavaScript files.

Repair is intentionally bounded:

- a step may perform at most two file-changing repair cycles;
- repair receives the verifier failure, expected paths, changed files, repeated
  files, and the original step goal;
- if verification still fails, Anvil emits a structured failure report;
- the report includes a suggested `/ultra-plan-run ...` command.

Anvil does not automatically start `/ultra-plan-run` after repair exhaustion.
That boundary is deliberate. It keeps local repair narrow and makes escalation
observable and user-controlled.

## Documentation Of Failures

When a failure repeats, the next action is not "add another guard" by default.
The expected path is:

1. inspect logs and artifacts;
2. classify the failure with engine-neutral vocabulary;
3. separate check bugs and tool-affordance traps from model capability limits;
4. prefer deterministic fixes over feedback;
5. admit a mechanism only after a focused ablation.

This is the reason the repository keeps:

- `docs/eval/triage/*.md`
- `docs/eval/mechanism-ledger.md`
- `docs/eval/frontier.md`
- benchmark roots and recheck reports

## Current Risks

The current risk is not the minimal loop alone; it is the outer workflow layer.
`minimal_step_runner` is useful, but plan generation, lint, profiles,
verification, repair, and ultra planning are the places where legacy-style
growth can return.

The rule of thumb is:

- add deterministic observation before control;
- split large files by responsibility before adding features;
- keep automatic repair bounded;
- make escalation explicit;
- measure every new mechanism against both target improvement and non-target
  non-regression.
