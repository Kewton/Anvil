# Anvil Concept

## One-Line Concept

Anvil is a Rust-based coding agent for local LLMs that aims to deliver flagship-class coding workflow quality on a local machine, with higher speed, stability, and operator clarity than today's local-first agents.

## Problem

Most coding agents are still optimized around hosted frontier models.
Even when they support local backends, the product assumptions often remain cloud-first:

- tool-calling is assumed to be highly reliable
- latency is treated as acceptable if the model is strong
- session state is not designed around long local work
- UX quality is tuned for cloud ecosystems rather than local constraints
- extensibility is often coupled to a provider or a specific frontend pattern

For users who want to code primarily with local LLMs, this creates a gap:

- existing local-first tools are often simpler, slower, or less polished
- stronger UX often comes with cloud assumptions
- stronger local support often comes with architectural limits

## Opportunity

By March 2026, a practical local model baseline can be assumed:

- around 20 GB model size
- slightly better than GPT-5 mini in quality
- about half the speed of GPT-5 mini
- 200k+ context window

That changes the design space.
The local model is no longer too weak to drive serious coding workflows, but it is still imperfect enough that the runtime must compensate for:

- weaker tool-call reliability
- higher latency sensitivity
- longer uninterrupted sessions
- constrained local execution safety

This creates an opening for a product built local-first from the ground up rather than adapted from a cloud-first agent.

## Core Thesis

The quality of a local coding agent is no longer determined only by model quality.
It is determined by the combined quality of:

- model routing
- tool-call recovery
- execution policy
- context management
- terminal UX
- interruption and recovery behavior

Anvil wins if it makes a very good local model feel like a great coding agent.

## Target User

Primary user:

- a developer who wants to do serious coding work with a local LLM in the terminal

Typical traits:

- values privacy, local control, or offline capability
- wants lower operational dependence on hosted APIs
- is willing to run models locally, but not willing to accept poor UX
- expects agent behavior to be fast, legible, and robust during real coding sessions

Secondary user:

- a power user or researcher who wants a local-first agent with a modern architecture that can evolve quickly

## Product Promise

Anvil should feel like:

- a serious coding tool, not a demo wrapper around Ollama
- a local-first agent, not a cloud agent with a local compatibility mode
- a terminal product with strong visual clarity, not a raw transcript viewer
- a system that fails safely and predictably, not a fragile chain of prompts and shell calls

## Competitive Position

Relative to `vibe-local`:

- preserve the strengths that matter for local use
- exceed it on runtime efficiency
- exceed it on architectural extensibility
- exceed it on terminal clarity and interaction polish
- exceed it on large-context and large-repo handling

Relative to Claude Code:

- aim for comparable terminal UX quality
- accept that model quality may differ, but close the gap through runtime quality
- provide a local-first experience where the surrounding system is tuned for local constraints rather than hosted assumptions

## Differentiation

### 1. Local-First By Design

Anvil should assume:

- local inference is the main path
- long context is available
- tool calls can be malformed
- speed and iteration latency matter constantly
- safety must work inside a local shell-first environment

### 2. Operator Clarity

The user should be able to tell at a glance:

- what they said
- what the agent is doing
- what tool is running
- whether the system is waiting, thinking, blocked, or done

This is not a cosmetic preference.
It is part of stability and trust.

### 3. Runtime Quality As Product Value

Anvil should treat these as core product features rather than implementation details:

- tool-call repair and recovery
- interruption handling
- rollback and execution policy
- session continuity
- context shaping
- sidecar model usage

### 4. Extensible Modern Core

Anvil should be built so it can absorb evolving coding-agent UX patterns without major rewrites.
That means the architecture should be prepared for:

- custom slash commands
- richer tool ecosystems
- new local model backends
- upgraded terminal interaction patterns
- more structured planning and execution workflows

## Experience Principles

### Fast

- low startup overhead
- low interaction overhead
- minimal waiting between action and feedback

### Legible

- clear visual distinction between user input and agent output
- explicit status visibility
- concise but informative tool execution display

### Stable

- interruptions should be safe
- tool failures should degrade predictably
- session state should survive normal failure cases

### Local

- local inference should be the default assumption
- local constraints should shape the design
- local execution safety should not be an afterthought

### Evolvable

- architecture should permit feature growth without structural collapse

## UX Identity

The product should have a terminal identity distinct from generic coding CLIs:

- an ASCII-art-like logo
- clear separation of user and agent messages
- custom slash commands as a first-class concept
- a layout that feels deliberate and fast rather than decorative

The ideal is to feel closer to a professional operator console than to a chat transcript.

## Success Definition

Anvil succeeds if a user can say:

- this feels built for local models, not merely compatible with them
- this is faster and more stable than `vibe-local`
- this is clearer and more modern to operate than most local coding agents
- this is the first local agent I can use for long real coding sessions without friction building up

## Early Scope Direction

Before detailed architecture, the product should orient around a first version that proves:

- strong local inference control
- resilient tool-driven coding loops
- high terminal clarity
- safe execution behavior
- persistent long-session usability

The first version does not need to prove every extension path.
It does need to prove the core concept:

local LLM coding can feel first-class if the agent runtime is designed correctly.
