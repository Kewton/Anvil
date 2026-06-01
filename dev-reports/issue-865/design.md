# Issue 865 Design

## Goal

Introduce a small verifier abstraction that separates task-kind policy from verifier execution details, then adapt the existing coding and docs paths without replacing the current local-first auto-test flow.

## Approach

- Add a `Verifier` trait plus `CodingVerifier`, `DocsVerifier`, and `DataVerifier` adapters under `src/agent/loop_run`.
- Keep coding verifier execution on the existing structured `AutoTestRunner` path, but update its task-kind adapters:
  - Rust final success runs full `cargo test` while still validating that owned test artifacts exist and are in scope before execution.
  - Python uses a canonical `python3 -m pytest -q` structured command for pytest evidence.
  - Node package-script verification runs `npm test` rather than partial `node --test <file>` for package.json test scripts.
- Route docs required-section checks through `DocsVerifier`; a pass can be represented as completion evidence and a failure can be converted into the same task-kind-independent `FailurePacket` shape used by repair.
- Keep repair packet payloads generic: command, failure kind, bounded output, candidate artifacts, and prior attempts. Do not add task-kind-specific fields to repair input.

## Non-goals

- Do not add provider abstraction or remote verifier support.
- Do not redesign the verifier repair controller.
- Do not change release workflow.
