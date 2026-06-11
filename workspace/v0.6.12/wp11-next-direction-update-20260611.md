# WP11 Next Direction Update

Date: 2026-06-11

## Inputs

This update uses:

- `workspace/v0.6.12/future-architecture-direction-20260611.md`
- `workspace/v0.6.12/wp0-failure-composition-20260611.md`
- `workspace/v0.6.12/wp9-20run-regression-guard-20260611.md`
- `workspace/v0.6.12/wp10-50run-improvement-candidate-20260611.md`

## Executive Read

The v0.6.12 direction was valid, and WP1-WP10 produced a strong recovery candidate.

Observed recovery:

- v0.6.12 baseline: 14/50 pass = 28.0%
- WP9 guard: 20/20 pass, 20/20 HQ
- WP10 candidate: 47/50 pass, 46/50 HQ
- false-done: stayed 0
- false-missing: stayed 0 in WP9/WP10

The recovery is not a final success claim yet:

- one 50-run can be high variance
- FastAPI remains unstable
- Python markdown can still externally pass while Anvil exits `repair_exhausted`
- ops command-observation can externally pass while Anvil exits `max_iterations`
- PAM failed in all PAM rows, so PAM did not contribute evidence of improvement

## Hypotheses That Worked

### Weak verifier as repair target

Evidence:

- Rust word was 0/6 with `safe_stop_verifier_weak` in the v0.6.12 baseline.
- WP10: Rust word 4/4 HQ.
- WP10: Rust NDJSON 4/4 HQ.
- No `safe_stop_verifier_weak` recurrence in the 50-run summary.

Read:

- The move from terminal-only weak verifier handling toward typed repair target was directionally correct.
- The key is still to preserve false-done safety while not stopping repairable cases.

### Behavior delta for feature improvement

Evidence:

- WP5 targeted smoke: feature_discount 4/4 HQ.
- WP9: feature_discount 2/2 HQ.
- Early `missing_repo_edits` did not recur in the feature sample.

Read:

- Current-turn behavior delta should remain first-class.
- This was achieved without a `discounts.py`-specific branch, so it supports the general architecture.

### SemanticCandidate limited primary adoption

Evidence:

- WP6 docs/data/ops smoke: 11/11 pass/HQ.
- WP9 and WP10 non-coding cases remained stable.

Read:

- Limited adoption for stable non-coding task kinds is safe enough to keep.
- Coding/research should remain more conservative until ambiguity handling is stronger.

### Prompt authority boundary

Evidence:

- WP7: Python sales 6/6 HQ, Python markdown 3/3 pass, docs/data stable.
- WP10: Python sales 8/8 HQ.
- Explicit unittest direct smoke generated valid unittest tests, though terminal convergence failed.

Read:

- Separating EvidenceRunner, AuthoringStyle, and RuntimeCapability in the prompt is correct.
- The boundary should remain typed and authority-scoped, not keyword-based.

### Actor loop no-op refactor

Evidence:

- WP8 deterministic checks passed.
- WP8 smoke: 3/3 pass, 2/3 HQ.
- WP9/10 did not show terminal projection regression.

Read:

- Small pure-decision extraction is safe.
- Large actor-loop phase-machine rewrites should still be avoided until protected by guards.

## Hypotheses Still Unproven Or Failed

### PAM benefit

Observation:

- WP10 PAM rows were `pam:failed`, with `context_pack_failed:sidecar_call`.
- PAM variant scored 24/25 HQ, but PAM did not inject successfully.

Read:

- PAM cannot be credited.
- PAM must remain advisory-only.
- Next PAM work should be availability and sidecar-call reporting, not semantic authority.

### API contract/state stability

Observation:

- FastAPI notes was 3/6 HQ in WP10.
- Failures involved state/list expectation drift and generated tests referencing unsupported `app.notes`.

Read:

- API contract expectation binding is now the highest coding-specific bottleneck.
- The problem is not simply "model weak"; the controller does not yet give a sufficiently stable typed state contract and test expectation boundary for API tasks.

### Terminal convergence for externally passing work

Observation:

- Python markdown externally passed but Anvil exited `repair_exhausted` in WP7/WP10.
- ops_health externally passed and was high-quality but exited `max_iterations` in WP9/WP10.

Read:

- Evidence observation and terminal projection still diverge for some task kinds.
- This should be fixed generically through evidence binding and terminal projection, not per-case final-answer rules.

## Updated Architecture Direction

The target architecture remains:

1. LLM or deterministic pre-pass proposes semantic intent.
2. Controller admits only typed, non-contradictory facts into `ObjectiveContract`.
3. The actor loop operates from sealed contract, not raw prompt reinterpretation.
4. Tools produce local facts.
5. Facts enter artifact ledger and evidence observations.
6. Completion is decided from contract + ledger + evidence + terminal projection.
7. Memory/PAM are advisory and never terminal authority.

WP10 updates the priority order:

1. Preserve the now-recovered controller path before doing broad refactors.
2. Fix API contract/state expectation drift.
3. Fix terminal/evidence convergence for externally passing work.
4. Continue small pure-decision extraction from actor loop only when guarded.
5. Keep expanding non-coding support through typed evidence, not coding-specific verifier assumptions.

## Next P0 Work

### P0-A: API Contract State Obligation

Goal:

- Make API state behavior explicit enough that tests do not invent unsupported storage shape or wrong list ordering/state persistence.

Approach:

- Add a typed API state expectation summary from admitted API contracts.
- Bind tests to public request/response behavior, not internal `app.notes` or incidental storage.
- Keep it framework-generic: route, method, request body, response body, state persistence, ordering policy.

Avoid:

- FastAPI-specific hard-coding.
- exact status-code invention when status is unspecified.
- test assertions against private implementation attributes.

Minimum validation:

- FastAPI notes 6-run.
- Node notes API 3-run.
- Python sales 2-run as non-API regression.
- docs/data 1-run each.

### P0-B: EvidenceObservation Terminal Alignment

Goal:

- If required deliverable and evidence observations are satisfied, terminal projection should not drift to `repair_exhausted` or `max_iterations`.

Approach:

- Compare active terminal decision with shadow terminal evidence for externally passing cases.
- Add a typed reconciliation decision for "evidence observed but terminal not converged".
- Keep false-done guard: do not turn failed evidence into success.

Minimum validation:

- Python markdown 6-run.
- ops_health 4-run.
- docs/data/research 1-run each.

### P0-C: PAM Availability Before PAM Effectiveness

Goal:

- Make PAM success/failure explicit enough that future evaluations cannot accidentally credit failed PAM.

Approach:

- Separate `pam_requested`, `pam_available`, `pam_injected`, `pam_failed`.
- Keep all PAM facts advisory.
- Add run summary counts for injected vs failed.

Minimum validation:

- one no-PAM/PAM paired 10-run.
- assert no contract/terminal difference when PAM unavailable.

## Next P1 Work

### P1-A: Continue ActorLoop Phase Decision Extraction

Only extract pure or near-pure decisions:

- terminal projection
- evidence-observation reconciliation
- repair target selection

Do not rewrite the whole loop.

### P1-B: Expand Non-Coding Evidence

The docs/data/research/ops direction is strong. Next work should add generic typed evidence rather than coding verifier concepts:

- document section acceptance
- data schema/row-count observation
- source fetch/citation observation
- command observation completeness

### P1-C: Repeat 50-Run Confirmation

Before declaring the architecture "fixed":

- repeat 50-run on a fresh round
- compare failure composition, not only pass rate
- require false-done 0
- require PAM rows to be reported separately by availability

## Guardrails

Keep:

- false-done 0 as a hard constraint
- sealed contract as downstream semantic authority
- EvidenceRunner / AuthoringStyle / RuntimeCapability separation
- behavior delta for current-turn feature work
- semantic candidate limited adoption only where stable
- 20-run before 50-run

Avoid:

- FastAPI-specific fix branches
- TOML-specific parser hacks
- WorkMode-driven semantic authority
- PAM as hidden authority
- large actor-loop rewrites without a guard run
- adding more request-substring heuristics to `task_contract.rs`

## Known Issues To Carry Forward

1. FastAPI/API tasks still fail on typed state and test expectation drift.
2. Python markdown can pass externally while active terminal exits `repair_exhausted`.
3. ops command-observation can pass externally while active terminal exits `max_iterations`.
4. PAM sidecar/context-pack calls fail in the current 50-run, so PAM benefit remains unproven.
5. `task_contract.rs`, `actor_loop_flow.rs`, `auto_test.rs`, and `quality.rs` remain complexity hotspots.

## Final Position

The architecture is closer to the target than it was at v0.6.12 baseline. The strongest signal is not just the 47/50 result; it is that false-done stayed 0 while hard coding cases and non-coding cases both improved in the same run.

The next step should not be a broad refactor. It should be a narrow, typed improvement to API contract/state obligations and evidence/terminal alignment, followed by another guarded 20-run and 50-run.
