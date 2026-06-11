# WP7 Style / Runner / Capability Boundary

Date: 2026-06-11

## Intent

WP7 reinforced the prompt boundary between:

- `EvidenceRunner`: verification means only.
- `AuthoringStyle`: artifact/test shape authority.
- `RuntimeCapability`: observed compatibility context only.

The change avoids adding task-specific matching. It projects existing typed controller facts into the Contract-Bound Generation packet as separate authority fields.

## Implementation

Changed:

- `src/agent/loop_run/contract_bound_generation.rs`

Added prompt packet fields:

- `runtime_capability_authority=context_only_not_completion_or_style_authority`
- `authoring_style_enforcement=...`
- `evidence_runner_policy=runner_kind=...,authority=verification_only_not_authoring_style`

The adoption is intentionally small:

- no new provider abstraction
- no benchmark-specific case branch
- no new execution lifecycle behavior
- no change to terminal projection

## Deterministic Verification

Commands:

```text
cargo fmt --check
cargo test --lib contract_bound_generation -- --nocapture
git diff --check
cargo build
```

Result:

- `contract_bound_generation`: 9/9 passed
- `cargo build`: passed

Pinned behavior:

- explicit unittest requests are authoritative for test artifact shape
- ambiguous Python defaults remain advisory
- evidence runner is rendered as verification-only authority
- runtime capability is rendered as context-only authority

## Real LLM Smoke

Harness command:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence python_sales,python_sales,python_sales,python_sales,python_sales,python_sales,python_markdown,python_markdown,python_markdown,docs_runbook,data_json \
  --variant no_pam \
  --run-id wp7-style-runner-capability-boundary-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

Result:

| Case | Runs | Pass | HQ | Notes |
| --- | ---: | ---: | ---: | --- |
| python_sales | 6 | 6 | 6 | no false-done, no false-missing |
| python_markdown | 3 | 3 | 2 | one `repair_exhausted`; external grader pass true but HQ false |
| docs_runbook | 1 | 1 | 1 | no coding scaffold regression |
| data_json | 1 | 1 | 1 | no coding scaffold regression |

Overall:

- pass: 11/11
- high_quality: 10/11
- verification_pass: 10/11
- false_done: 0
- false_missing: 0
- repair_exhausted: 1
- max_iterations: 0

Direct LLM smoke for prompts not covered by the harness:

| Prompt shape | Result | Notes |
| --- | --- | --- |
| explicit Python unittest | generated valid `unittest.TestCase` tests; external `python3 -m unittest discover -s tests` passed | Anvil terminal was `max_iterations` because verifier evidence was not reached |
| Python TDD slugify | generated implementation and pytest tests; external `pytest` passed; Anvil terminal was `done` | created unexpected `README.md` despite "Do not create README" |

## Log Evidence

The generated LLM prompt included the separated boundary packet:

```text
runtime_capability_authority=context_only_not_completion_or_style_authority
authoring_style_decision=style=unittest_class_style,authority=explicit_user_request
authoring_style_enforcement=authoritative_for_artifact_shape
evidence_runner_policy=runner_kind=coding_build_test,authority=verification_only_not_authoring_style
```

For ambiguous Python coding, the prompt included:

```text
authoring_style_decision=style=pytest_function_style,authority=model_robust_default
authoring_style_enforcement=advisory_model_robust_default_preference_not_runner_authority
```

## Assessment

The architecture direction remains valid. The prompt now exposes the three authorities separately without introducing rule sprawl:

- runner selection no longer needs to imply unittest/pytest authoring style
- runtime capability can guide compatible implementation without becoming completion authority
- weak style defaults are explicitly advisory

This WP does not claim broad success-rate improvement by itself. It is a boundary hardening change that made the controller prompt more legible and easier to debug.

## Carried-Forward Issues

- Explicit unittest generation can produce valid artifacts but still end in `max_iterations` when evidence is not invoked.
- TDD-shaped coding can still create an unexpected documentation artifact.
- Python markdown still has a repair convergence gap: one run passed external grading but ended `repair_exhausted`.

These are not fixed by prompt boundary separation. They point to later WP areas:

- terminal/evidence convergence
- unexpected artifact prevention
- repair lifecycle target selection
