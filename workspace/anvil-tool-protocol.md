# Anvil Tool Protocol

## Purpose

This document defines the core tool protocol for Anvil.
The goal is to avoid string-heavy ad hoc tool handling.

## Design Goals

- typed tool inputs and outputs
- explicit execution policy
- predictable interruption behavior
- support for local-model malformed output recovery
- support for both sequential and parallel-safe execution

## Tool Model

Each tool should define:

- stable tool name
- version
- typed input schema
- typed result schema
- execution class
- permission class
- plan-mode policy

## Execution Classes

Minimum execution classes:

- `ReadOnly`
- `Mutating`
- `Network`
- `Interactive`

These are not mutually exclusive in implementation, but the runtime should derive a canonical execution policy from them.

## Permission Classes

Minimum permission classes:

- `Safe`
- `Confirm`
- `Restricted`

Meaning:

- `Safe`: can run without approval
- `Confirm`: requires user approval unless session mode overrides it
- `Restricted`: blocked or heavily constrained unless explicitly allowed by policy

Approval granularity is one tool call at a time.
The runtime must not collapse multiple confirmation-required tool calls into a single approval decision.

## Parallel-Safe Classification

Each tool must declare whether it is:

- `ParallelSafe`
- `SequentialOnly`

This must be explicit, not inferred from name patterns.
Parallel-safe execution only applies after each involved tool call has individually passed approval requirements.

## Plan-Mode Policy

Each tool must declare plan-mode behavior:

- `Allowed`
- `AllowedWithScope`
- `Blocked`

Example:

- `Read` -> `Allowed`
- `Write` -> `AllowedWithScope`
- destructive shell actions -> `Blocked`

## Input and Result Shape

The runtime should not treat all tool payloads as raw JSON strings after parsing.
After provider normalization, tool calls should become typed internal values.

Representative internal structures:

- `ToolCallRequest`
- `ToolValidationResult`
- `ToolExecutionRequest`
- `ToolExecutionResult`
- `ToolExecutionError`

## Malformed Output Recovery

Because local models may emit broken tool calls, the runtime should support:

- argument repair
- missing field detection
- schema validation errors
- explicit rejection with user-visible summary

Recovered tool calls must still pass typed validation before execution.

## Result Requirements

Tool results should support:

- human-readable summary
- structured machine-readable payload
- error classification
- optional artifact references
- timing metadata

This allows the UI to show concise logs while the runtime preserves structured detail.

## Interruption Rules

If a tool is interrupted:

- the result must still normalize into a valid terminal tool result state
- the session history must remain structurally consistent
- the UI must know whether the tool completed, failed, or was interrupted

## Rollback Hooks

Mutating tools may register rollback hooks or checkpoint triggers.
Rollback policy should be declared at the execution-policy level, not hardcoded into individual UI paths.

## Minimum First-Version Tool Set

First-version tools should cover:

- shell execution
- file read
- file write
- file edit
- file search
- session/status support hooks as needed

This is enough to prove the typed tool protocol without overextending scope.

## Success Criteria

- tools are validated before execution through typed schemas
- permission and execution policy are explicit per tool
- malformed tool calls do not bypass validation
- parallel-safe execution is explicit and testable
- the tool system can grow without turning into a stringly typed monolith
