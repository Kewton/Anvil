# Anvil CLI Spec

## Purpose

This document defines the command-line product surface for Anvil.
It focuses on interaction behavior, not implementation details.

## Product Shape

Anvil is a terminal-first coding agent.
Its CLI should feel like an operator console for local coding workflows.

## Core Interaction Model

Message roles must be visually distinct:

- `[U] you >` for user input
- `[A] anvil >` for agent output
- `[T] tool  >` for tool execution logs

Status must always be visible in a dedicated area.

## Startup Screen

The startup screen should show:

- ASCII-art-like logo
- short product line
- active model
- context size
- current mode
- current project path
- ready state

## Input Model

Requirements:

- single-line input by default
- multi-line input mode
- type-ahead support while the agent is working
- slash commands available at any prompt
- clear send behavior

Minimum user-visible cues:

- enter to send
- multi-line mode hint
- interrupt hint

## Thinking View

When the agent is thinking, the UI should show:

- current overall plan
- active step within the plan
- short reasoning log
- elapsed time
- current model
- context usage
- interrupt affordance

The thinking view must not become a raw chain-of-thought dump.
It should be a compact user-facing progress explanation.

## Answer View

When the agent answers, the UI should separate:

- explanation to the user
- tool execution log
- completion state

The user should always be able to tell whether the agent is still working or has returned to input-ready state.

## Approval View

When approval is required, the UI should show:

- exactly one pending tool call
- the tool name
- a compact summary of the requested action
- the risk or permission class
- the available approval actions

Approval is handled per tool call, not per turn.
If multiple tool calls require approval, they should be presented one by one in deterministic order.

## Follow-Up View

Follow-up interactions must preserve:

- session continuity
- current context usage visibility
- ability to continue or interrupt work cleanly

## Slash Commands

Initial command categories:

- help and discovery
- session and status
- model and runtime
- planning and execution
- recovery and rollback
- developer workflow helpers

Initial commands to support:

- `/help`
- `/status`
- `/model`
- `/models`
- `/plan`
- `/act`
- `/compact`
- `/save`
- `/resume`
- `/checkpoint`
- `/rollback`
- `/diff`

Custom slash commands must be supported later through an extension mechanism.

## Status Area

The status area should show a compact subset of:

- current state
- active model
- context usage
- active step
- timing
- interrupt or command hints

Preferred state labels:

- `Ready`
- `Thinking`
- `Working`
- `AwaitingApproval`
- `Interrupted`
- `Done`

## Output Style Rules

- agent output should be concise by default
- tool output should be summarized unless detail is needed
- plan display should be stable during execution
- user-facing reasoning logs should stay short and legible
- completion should be explicit

## First-Version Constraints

The first CLI version does not need:

- full-screen multipane editing
- complex mouse interaction
- graphical widgets

The first CLI version does need:

- visual role separation
- stable thinking view
- stable answer view
- safe interruption behavior

## CLI Success Criteria

- a first-time user understands who is speaking and what is happening without explanation
- a user in a long session can always identify the current state quickly
- tool execution does not visually drown out the main answer
- follow-up work feels continuous rather than reset-prone
