# Plan Architecture, Incidents, and Issues

## Scope

This document focuses on the current `Plan` flow inside Anvil's auto mode, the observable incidents seen in local-LLM runs, and the problems that remain unsolved as of `v0.1.0`.

## Current Architecture

### 1. Auto entry

- User input is received in `Act` mode.
- The main model is asked once to classify:
  - whether the task is a `large_task`
  - which `task_profile` applies
- If `large_task=true`, Anvil automatically transitions into `Plan` mode.

Relevant code:

- `src/agent/loop_run/commands.rs`
  - `classify_large_task_with_main_model()`
  - `maybe_auto_plan_prompt()`

### 2. Plan file lifecycle

- Entering `Plan` mode creates or reuses an active plan file under the session directory.
- The plan file acts as the single writable artifact during planning.
- Until approval, code changes are not supposed to happen.

Relevant code:

- `src/agent/loop_run/commands.rs`
  - `enter_plan_mode()`
- `src/agent/loop_run/lifecycle.rs`
  - `ensure_plan_file()`

### 3. Plan structure

The current plan template is section-based:

- `Goal`
- `Constraints`
- `Deliverables`
- `Acceptance Criteria`
- `Quality Bar`
- `Execution Plan`
- `Verification Plan`
- `Risks / Fallbacks`

The implementation treats this as a staged fill process:

- Stage 1: `Goal / Constraints / Deliverables`
- Stage 2: `Acceptance Criteria / Quality Bar`
- Stage 3: `Execution Plan / Verification Plan / Risks / Fallbacks`

Relevant code:

- `src/agent/loop_run/lifecycle.rs`
  - `plan_missing_sections()`
  - `plan_next_stage_sections()`
  - `plan_is_substantive()`
  - `plan_act_summary()`

### 4. Plan-mode tool policy

In `Plan` mode:

- exploration is expected via `Read / Glob / Grep`
- `Bash` is disabled
- `Write / Edit` are intended only for the active plan file

This policy is defined mostly by prompt and turn-time checks, not by a strict runtime state machine.

Relevant code:

- `src/system_prompt.rs`
- `src/agent/loop_run/turn.rs`

### 5. Plan progress handling

- `PlanProgress` is treated as an action expectation.
- If the model keeps exploring without advancing the plan file, a recovery note is appended.
- Recovery today is primarily prompt-level, not hard runtime enforcement.

Relevant code:

- `src/agent/recovery.rs`
  - `ActionExpectation::PlanProgress`
  - `plan_progress_recovery_note()`
- `src/agent/loop_run/turn.rs`

### 6. Approval and handoff to Act

- When the plan is considered complete enough, approval UI is shown.
- If approved, Anvil transitions back to `Act`.
- A compact summary of the accepted plan is re-injected as guidance for execution.

Relevant code:

- `src/agent/loop_run/commands.rs`
  - `approve_plan_mode()`
  - approval prompt path in `process_line()`

## Observed Incidents

### 1. Repeated exploration in Plan mode

Most common failure pattern:

- model enters `Plan`
- reads `README.md`
- reads `README.md` again or repeats `Glob`
- delays or fails to switch to `Write / Edit(plan)`

This has been repeatedly observed with:

- `qwen3.5:27b`
- `qwen3.6:35b-a3b`
- `qwen3.6:27b-coding-nvfp4`

### 2. Plan write instability

Observed failure forms:

- malformed tool call markup
- nested/incorrect tool-call JSON wrappers
- wrong argument names such as `file` instead of `path`

One concrete improvement was already accepted:

- argument normalization for `Write/Edit`
- commit: `e8958cc`

This fixed a known `qwen3.5:122b` case where `Write` failed with `missing string field: path`.

### 3. Classifier instability

The auto-plan classifier is not fully stable across models.

Observed issues:

- transport failures when calling classifier
- parse failures for nonstandard classifier output
- fallback needed to keep `content` routing working

Accepted improvements already exist, but classifier reliability is still not fully stable across all test models.

### 4. Approval rarely reached

The approval UI itself works, but many runs do not reach it.

The problem is not mainly the approval UI. The real problem is that the planning loop often stalls before the plan becomes acceptable for approval.

### 5. Medium local models are especially sensitive

The heavier and more structured the plan requirements become, the harder it is for medium local models to complete the full planning flow reliably.

This was especially visible after adding:

- `Quality Bar`
- staged section filling
- stronger completeness expectations

## Main Problems

### Problem 1: Plan is still prompt-driven more than runtime-driven

Current behavior depends too much on:

- "do not keep exploring"
- "write the plan next"

These are recovery notes, not strong runtime constraints.

Effect:

- models can still continue exploration loops
- planning progress is suggested, not enforced

### Problem 2: There is no robust runtime notion of "enough exploration"

The system can count exploration calls per turn, but it still does not reliably answer:

- has this exact inspection already been done?
- is this exploration redundant for the current plan stage?
- should the next allowed action be only `Write/Edit(plan)`?

This is why repeated `Read README.md` remains difficult to stop cleanly.

### Problem 3: Plan completeness is structurally good but operationally heavy

The section model is reasonable, but the effective threshold for getting from:

- exploration
to
- first meaningful plan write
to
- approval

is still too heavy for several local models.

This creates a mismatch:

- architecture wants a high-quality plan
- runtime behavior of local models tends to stall before reaching that bar

### Problem 4: Tool-call robustness is still part of the planning problem

`Plan` quality is not just about reasoning quality. It is also constrained by whether the model can emit a valid tool call for:

- `Write(plan)`
- `Edit(plan)`

Even when the model "knows what to write", the plan can still fail due to malformed or aliased tool arguments.

### Problem 5: Observability improved, but control is still weak

Accepted milestone logging now helps identify:

- classifier result
- first plan write
- plan approval
- first repo edit
- turn completion

This improved diagnosis, but did not by itself improve runtime control.

## What Has Been Tried

### Accepted

- classifier fallback improvements
  - commit `4f8ccda`
- `PlanProgress` expectation
  - commit `4c5402f`
- milestone logging for E2E evaluation
  - commit `cfee420`
- aliased tool-argument normalization
  - commit `e8958cc`

### Rejected after testing

- normalized repeated-exploration detection
- tool-level block for repeated exploration
- classifier retry
- lighter `PlanStage` exploration budgets
- relaxed `PlanReadyForApproval`

These were rejected because they did not show stable improvement across the model set, or they worsened the best-case path.

## Current Conclusion

The current `Plan` architecture is conceptually organized, but execution control is still weak.

In short:

- entry into `Plan` mostly works
- the section template is clear
- progress recovery exists
- approval exists
- logging exists

But:

- exploration loops are not reliably broken
- medium local models still stall before approval
- runtime enforcement remains weaker than the prompt structure

## Practical Summary

If `Plan` is the focus, the core unresolved issue is:

**Anvil can describe how planning should proceed, but it still cannot reliably force local models to advance from repeated inspection into stable plan-file updates and approval.**
