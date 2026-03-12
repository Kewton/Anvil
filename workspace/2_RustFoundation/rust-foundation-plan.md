# Rust Foundation Plan

## Purpose

This document translates the Product Definition outputs into the first Rust implementation layer.

## Objectives

- avoid recreating a `vibe-local` style monolith
- create a minimal but durable Rust structure
- make the first CLI prototype reachable quickly
- preserve room for later provider, tool, and TUI expansion

## Foundation Deliverables

### 1. Repository Layout

Define a repo structure that separates:

- application bootstrap
- domain logic
- provider integration
- tool system
- session/state
- TUI

### 2. Initial Cargo Structure

Start with one binary crate and disciplined modules.
Do not prematurely split into many crates unless a boundary proves unstable.

### 3. Bootstrap Path

Define a clean startup path:

1. load config
2. initialize logging
3. initialize app context
4. initialize session environment
5. initialize provider
6. initialize runtime and TUI
7. enter interactive loop

### 4. Error and Logging Model

Define a single application error model and structured logging conventions before broad implementation starts.

### 5. First Implementation Slice

The first useful Rust slice should prove:

- startup works
- config loads
- a state machine exists
- a simple TUI shell renders
- a mock or placeholder runtime loop can move through visible states

## Constraints

- local-first assumptions must remain the default
- internal interfaces should be typed
- state transitions should be explicit
- approval and interruption must remain first-class concerns

## Exit Criteria

Rust Foundation is complete when:

- implementation can begin from stable module boundaries
- bootstrap behavior is defined
- logging and error handling conventions are fixed
- the first useful prototype slice is sequenced

## Recommended Implementation Order

Use this order for the first implementation pass:

1. create Cargo project and module skeleton
2. implement shared contracts and state primitives
3. implement config loading and `EffectiveConfig`
4. implement TUI shell with static `Ready` rendering
5. implement explicit state transitions
6. implement mock runtime flow for `Thinking -> AwaitingApproval -> Working -> Done`
7. implement interruption flow ending in `Interrupted -> Ready`
8. add integration tests for the visible shell and state behavior

This order is intended to prove the product shape before provider and real tool execution are introduced.
