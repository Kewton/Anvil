# Anvil State Model

## Purpose

This document defines the user-visible and runtime-relevant states of Anvil.

## State Principles

- state must be explicit, not inferred from scattered flags
- UI should render from state snapshots
- interruption and recovery must be modeled directly
- tool execution and provider execution should not be conflated

## Primary States

### `Ready`

Meaning:

- system is initialized
- user input is accepted
- no provider turn or tool execution is active

### `Thinking`

Meaning:

- provider turn is active
- the agent is analyzing or generating
- no tool is currently executing

Visible fields:

- current plan
- active step
- reasoning summary
- elapsed time
- model
- context usage

### `Working`

Meaning:

- one or more tools are currently executing

Visible fields:

- active tool or tools
- active plan step
- elapsed time
- policy state if relevant

### `AwaitingApproval`

Meaning:

- agent proposed an action that requires user approval
- exactly one tool call is pending approval in this state

Visible fields:

- tool name
- summarized action
- risk classification
- pending tool call identifier

### `Interrupted`

Meaning:

- user interrupted the active provider or tool execution path
- session state remains valid

Visible fields:

- what was interrupted
- what remains saved
- next available actions

### `Done`

Meaning:

- the current turn completed successfully
- user input can continue

Visible fields:

- completion summary
- timing
- saved status

### `Error`

Meaning:

- unrecovered error occurred in provider, tool execution, or session layer

Visible fields:

- compact error summary
- whether state was preserved
- recommended next actions

## Transition Rules

Typical transitions:

- `Ready -> Thinking`
- `Thinking -> Working`
- `Working -> Thinking`
- `Thinking -> AwaitingApproval`
- `AwaitingApproval -> Working`
- `AwaitingApproval -> Ready`
- `Thinking -> Done`
- `Working -> Done`
- `Thinking -> Interrupted`
- `Working -> Interrupted`
- `Interrupted -> Ready`
- `Thinking -> Error`
- `Working -> Error`
- `Error -> Ready`

## Turn Model

A user turn may include multiple internal transitions.
Example:

1. user submits input
2. `Ready -> Thinking`
3. provider proposes tool calls
4. `Thinking -> Working`
5. tools finish
6. `Working -> Thinking`
7. provider produces final answer
8. `Thinking -> Done`
9. UI returns to `Ready`

## Plan Visibility Model

State snapshots should include plan visibility fields:

- `plan_items`
- `active_plan_index`
- `active_plan_label`
- `reasoning_summary`

This is necessary for the thinking UI defined in the usage-image document.

## Interrupt Semantics

On interruption:

- current state changes immediately to `Interrupted`
- partial work remains recorded in session if structurally valid
- missing tool results are normalized if needed to preserve history integrity
- the user can continue from a safe next state

## Approval Semantics

`AwaitingApproval` is distinct from `Thinking` and `Working`.
The UI should not hide approval waits inside generic busy states.

Approval is resolved per tool call.
If multiple tool calls in a turn require approval, the runtime should queue them and enter `AwaitingApproval` once for each tool call in order.

## Persistence Semantics

The session layer should persist enough information to restore:

- current message history
- state summary
- last active plan snapshot if relevant

It does not need to restore live tool execution.

## First-Version Success Criteria

- every visible busy period maps to an explicit state
- approval waits are visible as their own state
- interruptions do not leave the UI in an ambiguous state
- state can be rendered without inspecting provider or tool internals directly
