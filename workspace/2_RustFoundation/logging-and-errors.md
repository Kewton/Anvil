# Logging and Error Model

## Goal

Define a logging and error strategy before broader implementation begins.

## Logging Principles

- logs are for diagnosis, not user-facing narration
- user-facing status belongs in the TUI, not logs
- errors should carry enough context to debug boundaries
- logs should support comparison-axis measurement later

## Logging Categories

Recommended categories:

- app startup
- config resolution
- provider requests and responses
- state transitions
- tool execution lifecycle
- session persistence
- interruption and recovery

## Event Logging

Important events to log structurally:

- startup completed
- config resolved
- provider initialized
- turn started
- tool call proposed
- approval requested
- approval resolved
- tool started
- tool finished
- interruption
- session saved
- unrecovered error

## Error Model

Use a central application error enum with subsystem-specific variants.

Representative categories:

- config error
- filesystem error
- provider error
- protocol error
- tool validation error
- tool execution error
- state transition error
- session persistence error
- tui error

## Error Rules

- errors crossing module boundaries should use typed variants
- errors should include source context where useful
- user-facing error messages should be compact and actionable
- internal error detail can remain richer than UI output

## User-Facing Error Taxonomy

The TUI should classify surfaced errors into compact user-facing groups:

- `Recoverable`
- `ApprovalBlocked`
- `Interrupted`
- `Fatal`

Meaning:

- `Recoverable`: the session can continue normally after acknowledgement
- `ApprovalBlocked`: execution did not proceed because approval was denied or restricted
- `Interrupted`: execution stopped due to explicit user interruption
- `Fatal`: the current app flow cannot continue without restart or major recovery

This taxonomy is for presentation and operator clarity.
It can be derived from richer internal error variants.

## Recommended Practice

- use one top-level app error type
- avoid passing raw strings as primary error values
- attach timing and subsystem context where possible

## First-Version Success

- every major subsystem returns typed errors
- startup failures are readable
- runtime failures do not force ad hoc string parsing
- user-facing error rendering is consistent with the TUI state model
