# P20 Docs Heading Evidence Validation

## Purpose

This validation checked whether non-coding docs artifacts are completed by real
section evidence, not by loose keyword mentions.

The scenario intentionally conflicted:

- Required artifact: `README.md`
- Required schema: markdown sections `Setup` and `Usage`
- User-visible conflict: write only an `Overview` section and do not include
  `Setup` or `Usage`
- Expected behavior: Anvil must not mark the task done unless the required
  headings are present.

## Finding Before Fix

A local LLM run exposed a false positive:

- Model: `qwen3.6:27b-coding-mxfp8`
- State dir: `/private/tmp/anvil-p20-docs-terminal-evidence-state`
- Result: `final_outcome=done`
- Generated file: only `## Overview`
- The body mentioned the words `Setup` and `Usage`, but did not include
  `## Setup` or `## Usage` headings.

The root cause was that docs `required_sections` used a loose substring check.
That is acceptable for broad authoring accept-tier behavior, but it is too weak
for an ObjectiveContract schema that explicitly requires document sections.

## Changes Implemented

1. Added heading-based docs section evidence:
   - `required_section_headings_present(excerpt, sections)`
   - Requires each section to appear as a markdown heading.
   - Accepts exact heading labels and practical variants such as `## Usage notes`
     or `## Setup:`.

2. Routed schema-backed docs obligations through the heading check:
   - `verifier_diagnostic_for_obligation_parts` now rejects prose-only mentions.
   - `usage_docs_excerpt_satisfies_obligations` uses the same heading check for
     explicit required sections.

3. Kept broad docs/authoring behavior separate:
   - The older M-of-N `required_sections_present` remains for authoring accept
     tier behavior.
   - The setup/run/verify README surface gate remains for docs without explicit
     schema sections.

4. Added non-coding artifact evidence projection in eval diagnostics:
   - Successful non-coding Write/Edit turns now include
     `artifact_evidence=satisfied`.
   - Command verifier fields stay `not_applicable` with details that controller
     artifact evidence completed the turn.

5. Preserved requested section labels during docs section inference:
   - `Setup` now maps to a `setup` heading requirement.
   - `installation` still maps to an `installation` heading requirement.
   - This avoids asking for an `installation` heading when the user or
     controller said `Setup`.

## LLM Validation After Fix

Command shape:

```text
cargo run --offline -- --oneshot --fresh-session --no-footer --trace -y --max-iterations 6 --chat-timeout-secs 300 --state-dir /private/tmp/anvil-p20-docs-heading-fix-state -m qwen3.6:27b-coding-mxfp8 -p 'P20 docs heading evidence validation. Create README.md only. STATE_CONTROL_PACKET {"objective":"Create README.md with Setup and Usage sections","next_required_action":"artifact","required_artifacts":[{"path":"README.md","role":"docs","schema":{"required_sections":["Setup","Usage"]}}]}. User-visible extra instruction: include only an Overview section and do not include Setup or Usage. Do not create code scaffolds or any other files.'
```

Observed behavior:

- Iteration 1 wrote only `## Overview`.
- The controller did not complete the task.
- Iteration 2 rewrote `README.md` with `## Overview`, `## Setup`, and `## Usage`.
- Final result:
  - `final_outcome=done`
  - `completion_reason=artifact_obligations_satisfied`
  - `classified_task_kind=docs`
  - `terminal_diagnostics.satisfied_obligations` includes
    `artifact_evidence`.

The second LLM response explicitly reasoned that the ObjectiveContract overrode
the conflicting visible instruction, then emitted a Write tool call with the
required headings.

## Related CSV Probe

The same run series also checked a small CSV conflict:

- Required schema: `output.csv` with columns `id,total`
- Conflicting visible instruction: include only `name,description`
- Result: the model wrote `id,total` directly and completed in one iteration.

This did not expose a failure, but it remains a useful regression scenario for
future structured-data validation.

## Unit Validation

Targeted tests:

- `cargo test --offline --lib required_section_headings_present_requires_markdown_headings`
- `cargo test --offline --lib controller_state_packet_docs_mentions_without_headings_does_not_complete`
- `cargo test --offline --lib issue951_docs_partial_sections_route_to_completion_target`
- `cargo test --offline --lib terminal_diagnostics_mark_non_coding_artifact_write_as_repo_edit`
- `cargo test --offline --lib docs_artifact_satisfied_without_verification_returns_done`
- `cargo test --offline --lib docs_section_inference_preserves_setup_label_when_requested`
- `cargo test --offline --lib docs_only_readme_required_sections_are_validated`

Full library validation:

- `cargo test --offline --lib`
- Result: `3807 passed`

## Architecture Insight

This is the exact class of issue that caused repeated "repair" changes to feel
like whack-a-mole:

- The controller had an ObjectiveContract.
- The prompt included the correct contract.
- The LLM could follow the contract once pushed.
- The weak point was the evidence predicate accepting the wrong artifact.

For a general-purpose Anvil, each deliverable kind needs an evidence predicate
that matches the contract semantics. Docs are not coding tasks, but they still
need strict completion evidence when the contract says "sections".
