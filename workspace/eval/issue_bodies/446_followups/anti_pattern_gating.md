## Context

Follow-up of #446.

Parent follow-up:
- #498

Issue 446 evaluation confirmed AntiPattern creation, repeat increment, and prompt injection:

- First repeated unsafe Bash failure created an AntiPattern.
- Second occurrence incremented repeat_count to 2.
- Third run injected `Avoid Patterns`.

However, the model still attempted the same blocked Bash command again.

## Problem

AntiPattern memory currently influences the prompt, but does not reliably prevent the repeated action. For local LLMs, prompt-only avoidance is not enough for repeated tool misuse.

## Desired Direction

- Convert high-confidence AntiPatterns into runtime/tool gating hints.
- Block or rewrite repeated unsafe command shapes before tool execution.
- Keep the mechanism scoped and explainable to avoid false positives.
- Preserve user intent when the user explicitly asks for a diagnostic run, but avoid loops.

## Acceptance Criteria

- A repeated AntiPattern can prevent the same unsafe Bash command from being attempted again.
- The tool result explains that the action was blocked due to a remembered avoid pattern.
- False positives are bounded by task similarity, feedback kind, and repeat count.
- Tests cover repeated unsafe Bash, unrelated Bash, and explicit user override/diagnostic cases.
