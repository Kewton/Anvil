# Anvil Comparison Axes

## Purpose

This document defines measurable comparison axes for evaluating Anvil against `vibe-local`.

## Rules

- axes should be measurable or at least operationally scorable
- axes should reflect local-first coding workflows
- axes should inform product and engineering prioritization

## Core Axes

### 1. First-Use Experience

Measure:

- install/setup steps
- time-to-first-usable-session
- model/runtime readiness clarity

### 2. Iteration Speed

Measure:

- startup latency
- first prompt latency
- median follow-up turn latency
- tool execution overhead outside model inference

### 3. Tool-Call Robustness

Measure:

- malformed tool-call recovery success rate
- rate of unrecoverable tool-call failures
- loop-detection effectiveness

### 4. Stability and Recovery

Measure:

- session survival after interruption
- behavior after provider disconnect
- consistency of history after cancelled tool runs
- rollback and checkpoint reliability

### 5. Long-Session Usability

Measure:

- context usage visibility
- compaction quality
- session resume quality
- user ability to continue work after many turns

### 6. UX Clarity

Measure:

- distinguishability of user / agent / tool output
- visibility of current state
- visibility of plan and active step during thinking
- readability of answers during tool-heavy workflows

### 7. Large-Repo Handling

Measure:

- retrieval latency on larger repositories
- relevance quality of retrieved context
- responsiveness under larger context windows

## Use

These axes should be referenced in:

- architecture decisions
- milestone reviews
- prototype evaluations
- regression checks against `vibe-local`
