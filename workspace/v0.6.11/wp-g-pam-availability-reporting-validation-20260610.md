# WP-G: PAM Availability Reporting Validation

Date: 2026-06-10

## Objective

Separate PAM availability from task outcome and terminal authority. The goal is to make evaluation logs answer whether PAM was disabled, unavailable, injected, or blocked without treating PAM failure as task failure.

## Implementation Summary

- Added `PamEvalSummary.availability` with the values `disabled`, `failed`, `injected`, `blocked_warning`, `not_injected`, and `unknown`.
- Recorded `disabled` when Photon/PAM is explicitly off, instead of reporting it as `photon_unavailable`.
- Projected `availability=failed` into `evaluation_taxonomy.pam_variant=pam_unavailable`, keeping no-PAM runs as `pam_off`.
- Added PAM availability columns to the WP evaluation matrix:
  - `pam_availability`
  - `pam_injected_count`
  - `pam_unused_reason`
- Added a `By PAM Availability` summary section so no-PAM and unavailable-PAM rows are visible separately.

## Deterministic Verification

- `cargo test --lib pam_eval_summary -- --nocapture`: passed
- `cargo test --lib pam_eval -- --nocapture`: passed
- `cargo test --lib evaluation_taxonomy_separates_pam_unavailable -- --nocapture`: passed
- `python3 -m py_compile workspace/v0.6.11/wp_eval_matrix.py`: passed
- `cargo build`: passed
- `git diff --check`: passed

## Real LLM Validation

All real LLM runs used local Ollama through the WP11 smoke matrix.

| Run | Variant | Result | PAM availability observation |
| --- | --- | --- | --- |
| `wp-g-pam-availability-no-pam-20260610` | `no_pam` | 5/5 pass, 5/5 high_quality | `no_pam:disabled` 5/5 |
| `wp-g-pam-availability-pam-20260610` | `pam` | 5/5 pass, 5/5 high_quality | `pam:failed` 5/5, `unused_reason=context_pack_failed` |
| `wp-g-pam-taxonomy-pam-unavailable-20260610` | `pam` | 1/1 pass, 1/1 high_quality | eval log projected `pam_variant=pam_unavailable` |

The final taxonomy smoke recorded:

- `pam_eval.availability=failed`
- `pam_eval.unused_reason=context_pack_failed`
- `evaluation_taxonomy.pam_variant=pam_unavailable`
- task terminal outcome remained `done`

## Interpretation

WP-G validates that PAM availability is now reported as a separate evaluation dimension. A task can pass while PAM is unavailable, and no-PAM runs are no longer conflated with failed PAM context generation.

This does not prove PAM improves task quality. In this environment, PAM context generation failed before any memory was injected, so WP-G only validates observability and taxonomy separation.

## Known Issues

- PAM effectiveness is still unmeasured because local Photon context generation returned `context_pack_failed` in the PAM smoke.
- No live sample covered `injected` or `blocked_warning`; deterministic tests cover those projections.
- PAM remains advisory and must not override completion judgement. This is intentional, but future improvement claims need a run where injected PAM is actually observed.
