# Initial Implementation Slice

## Goal

Define the first Rust implementation slice that proves the foundation is correct.

## Slice Name

`Bootstrap + State-Driven TUI Shell`

## What This Slice Should Include

- Rust project boots successfully
- config loads from CLI/env/file
- app initializes core paths
- explicit state machine exists
- TUI renders startup screen
- TUI renders `Ready`, `Thinking`, `Working`, `Done`, and `AwaitingApproval`
- TUI renders `Interrupted`
- a mock runtime loop can simulate one user turn
- approval can be represented one tool call at a time
- interruption can be represented and recovered from cleanly

## What This Slice Should Not Include Yet

- full provider integration
- real MCP support
- full tool inventory
- advanced retrieval
- complex compaction logic

## Demonstration Scenario

The first slice should be able to simulate:

1. app start
2. user enters a prompt
3. state changes to `Thinking`
4. a mock plan and reasoning summary appear
5. a mock tool approval is requested for one tool call
6. approval is resolved
7. state changes to `Working`
8. a mock result is shown
9. state changes to `Done`
10. state returns to `Ready`

Additional interruption scenario:

1. app start
2. user enters a prompt
3. state changes to `Thinking`
4. user interrupts
5. state changes to `Interrupted`
6. UI shows preserved status and next actions
7. state returns to `Ready`

## Why This Slice Matters

This slice proves:

- state model viability
- TUI clarity model viability
- approval granularity
- interruption semantics
- bootstrap correctness
- non-monolithic initial structure

## Next Slice After This

After this slice, the next logical step is:

- real Ollama-backed provider integration
- real typed tool execution path
- session persistence integration
