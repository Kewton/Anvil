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

## Design Principles

Minimal is not trying to build a smarter hidden controller around the model. It
is trying to make small local-model turns succeed by giving them clear contracts,
deterministic checks, and explicit escalation points.

Planning, execution, and orchestration therefore have different jobs:

- **Planning** splits a request into work that can be executed in small local
  turns. It should preserve artifact contracts, verification expectations,
  profile constraints, and work intent. It should not become a detailed
  executor.
- **Execution** performs one scoped task with tools. It should keep the loop
  small, rely on observable workspace facts, and accept completion only when
  the deterministic contract for the step is satisfied.
- **Orchestration** connects phases and reports failure evidence. It may run
  bounded repair, but it must not silently escalate into an unbounded
  "keep trying until success" controller.

The practical ordering is:

1. remove deterministic traps before adding feedback;
2. verify artifacts and local commands before trusting natural-language status;
3. keep repair bounded and evidence-rich;
4. add a new mechanism only after failure triage and focused ablation;
5. prefer deleting ambiguity over adding prompt examples.

This keeps the system closer to "small agent in a well-shaped environment" than
to "large legacy controller with many hidden recovery paths".

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
        profiles/      profile-specific contracts, snapshots, and verification
          nextjs.rs
          data.rs
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

Step plans use a small common DSL. Each step has a `kind`:

- `inspect`: read or examine existing state;
- `create`: create new artifacts;
- `edit`: modify existing artifacts;
- `setup`: prepare dependencies or local environment;
- `verify`: run deterministic checks only;
- `repair`: fix a known failed check;
- `report`: stop with an explicit blocker such as `dependency_missing`;
- `work`: backward-compatible default for older plans.

The important boundary is profile-neutral: setup and verify must not be mixed.
A setup step may install or prepare dependencies, but it does not claim build or
test success. A verify step may run `npm run build`, `cargo test`, `pytest`, or
similar deterministic checks, but it must not install dependencies, scaffold
files, edit source, or repair failures. If a verifier requires dependencies, the
plan must contain an earlier setup step or report `dependency_missing` instead
of pretending the check passed.

`/ultra-plan-run`:

- asks for a top-level phase plan;
- runs each phase as a separate `/plan-run`;
- carries a workspace snapshot and a profile contract into each phase;
- carries a detected work intent such as `create`, `fix`, `investigate`,
  `enhance`, `document`, or `refactor` into phase prompts;
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

Profile checks must stay small and explainable. A profile may check
deterministic consistency inside its own technology contract, such as "Next.js
build scripts still call `next build`" or "Tailwind directives have the Tailwind
toolchain installed". A profile should not judge subjective output quality or
encode one benchmark's preferred file layout unless the user supplied that
layout as an explicit artifact contract.

When a profile starts accumulating many checks, split the implementation before
adding behavior. The preferred shape is small contract helpers under
`minimal_step_runner/profiles/`, with tests that show both the rejected
misconfiguration and the accepted valid configuration. Cross-profile concepts
belong in `verify.rs` or `plan_lint.rs` only when the same deterministic rule
applies to multiple domains.

Work intent is a second axis separate from profile:

- `profile` describes the technical domain, such as Next.js, Rust, Python, or
  data analysis.
- `intent` describes why the user is working, such as new creation,
  investigation, bug fixing, enhancement, documentation, or refactoring.

This separation prevents a contract for `nextjs + create` from accidentally
over-constraining `nextjs + investigate` or `nextjs + fix`. Intent is currently
lightweight: it is detected from the user goal, stored in generated ultra plans,
and included in phase prompts. Concrete intent-specific verification is added
only when failure triage shows a real need.

## Plan Lint Boundary

Plan lint is a safety boundary, not a quality oracle. It should reject clear
contradictions before execution, but it should not enforce stylistic preferences
or make the planner satisfy unnecessary wording constraints.

Plan lint should reject:

- shell commands in natural-language step instructions;
- unsafe or setup-oriented `verify` commands;
- absolute paths or paths that escape the workspace;
- `verify` steps that install or prepare dependencies;
- `setup` steps that try to claim build/test verification;
- npm verification before dependency setup or an existing `node_modules`;
- `npm run build` before a Next.js package and entry path are present;
- TDD ultra plans without an explicit red/failing test phase;
- creation/edit steps with multiple expected artifacts but no concrete artifact
  name in the instruction.

Plan lint should not reject a verification-only step just because it has
multiple `expected_paths`. A final build/check step often validates previously
created artifacts as a set. Requiring that step to repeat every file name is a
formatting constraint, not a deterministic safety check.

The boundary should be:

- creation/edit steps still need concrete artifact names;
- verification-only steps may reference multiple expected paths without naming
  each one;
- a step is verification-only only when it has `verify` commands, has
  verification/check/build/test language in the instruction or step id, and does
  not ask to create, add, edit, update, write, or implement artifacts;
- when workspace context is available, a verification-only step should be even
  safer if its expected paths were already introduced by earlier steps or
  already exist in the workspace.

When plan lint rejects a generated plan, the planner currently gets a short
"previous plan was invalid" retry prompt and can regenerate up to three times.
That retry loop is useful for malformed plans, but it is not the right fix for a
false positive lint rule. If lint rejects a reasonable `final-build-check` style
step, the lint boundary should be narrowed rather than making the planner fight
the rule with more prompt text.

The `final-build-check` failure exposed a general root cause, not a Next.js-only
bug. The previous lint rule treated "a step has multiple expected paths but the
instruction does not name one" as suspicious for every step. That was valid for
creation and edit steps, but not for final verification steps that intentionally
validate a previously introduced set of artifacts. The model had already
separated the work correctly; lint conflated artifact production with artifact
verification.

The horizontal rule is therefore profile-neutral:

- classify the step shape first, not the framework;
- apply concrete-file-name strictness to creation/edit steps;
- allow verification-only steps to validate multiple known artifacts;
- keep unknown new artifacts strict so missing creation work is still caught;
- do not add per-model or per-profile exceptions for wording like
  `final-build-check`.

This applies equally to Next.js, Python, Rust, data, and documentation profiles.
For example, `cargo test` over `Cargo.toml` and `src/lib.rs`, `pytest` over a
package and tests, or a data-report validation over previously created
`report.md` and `summary.csv` are verification steps. They should not have to
repeat every file name in natural language. A step that creates those files
still must name the concrete artifact it is responsible for.

## Verification And Repair

Step verification is deterministic and local:

- expected path existence;
- safe command allowlist such as `npm run build`, `cargo check`, `cargo test`,
  `python3 -m py_compile`, `python3 script.py`, `pytest`, `cat`, and
  `node --check` for JavaScript files.

Verifier failures should move toward stable, engine-neutral language when a
deterministic classification is available. The vocabulary being introduced is:

- `missing_artifact`
- `dependency_missing`
- `verifier_unavailable`
- `verifier_failed`
- `contract_violation`
- `repair_exhausted`

These categories are not repair recipes. They are evidence labels that keep
reports and repair prompts clear without adding a large hidden controller. They
are added incrementally when triage identifies a deterministic distinction worth
preserving.

For example, in `nextjs + create`, `npm run build` requires a real Next.js
binary. If `package.json` says `scripts.build = "next build"` but
`node_modules/.bin/next` is missing, Anvil reports `dependency_missing` /
`verifier_unavailable` instead of accepting a fake build script such as
`echo skipped`. It does not automatically run `npm install`; the model must use
the explicit step context or stop with the dependency failure.

Build success is necessary but not always sufficient. A generated Next.js app
can compile while still violating a toolchain contract, for example by writing
`@tailwind` directives and Tailwind utility classes without a Tailwind
dependency or config. That is a profile-level contract violation, not a reason
to add a larger executor. The profile verifier should catch deterministic
toolchain mismatches such as:

- `@tailwind` directives without `tailwindcss`;
- Tailwind directives without `tailwind.config.*`;
- Tailwind directives without `postcss.config.*`;
- fake build scripts that replace `next build`.

This keeps the guard generic: Anvil checks that the declared technology stack is
internally consistent. It does not try to judge whether the game is fun or
whether a specific visual design is good.

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
