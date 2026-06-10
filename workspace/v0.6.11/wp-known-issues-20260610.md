# WP4-WP11 Known Issues

Date: 2026-06-10

This file tracks known issues observed while executing WP4-WP11. It is separate from per-WP validation notes so that unresolved issues are easy to carry forward.

## WP4: SemanticCandidate Shadow

- Shadow candidates are still deterministic projections of the existing path, not a fully independent LLM semantic candidate.
- Shadow logs help compare contract quality, but they do not by themselves improve completion or repair behavior.

## WP5: SemanticCandidate Limited Adoption

- Limited adoption is intentionally low risk: docs/data/research/ops and styled coding candidates only.
- Coding success-rate improvement cannot be claimed from WP5 alone because most hard failures are still downstream in evidence binding and repair convergence.

## WP6: Contract-Bound Generation

- Hard Python repair still reached `repair_exhausted` after verifier failures. The taxonomy made the failure visible but did not make repair converge.
- Rust integration tests can be generated and pass manually, but internal generated-test preflight may reject them as `unsupported_contract_assertion`, producing `safe_stop_verifier_missing`.
- Node runnable tests can pass manually, but artifact completion may still exhaust because the test shape is not recognized as bindable evidence.
- Existing-project feature improvement exposed a false-done path. This was fixed by requiring fresh repo edit evidence before accepting verifier success for coding change contracts.
- After the false-done guard, the same scenario now fails closed with `missing_repo_edits`; it still needs WP7/WP9 work to recover into the correct implementation edit.
- TDD order is not strictly enforced by the controller; a successful TDD smoke still wrote implementation before tests on one attempt path.

## WP7: RepairTargetDecision Typed Delta

- Typed delta and ledger facts are now emitted on the live repair path, but this is primarily observability/prompt-boundary improvement. It does not by itself make hard repairs converge.
- A verification-only coding request with existing source/test artifacts still stops at `missing_repo_edits` because the WP6 fresh-edit guard cannot distinguish "verify existing artifacts" from "build/modify/fix source". This should be addressed by a typed verification-only objective or a more precise edit-obligation predicate.
- A Python repair smoke successfully fixed the implementation, but then stopped at `repair_safe_stop: verifier_unavailable`; manual `PYTHONPATH=. pytest -q tests/test_calculator.py` passed. The remaining issue is verifier rerun binding/import environment, not target selection.
- `tool_failure` is part of the delta vocabulary, but live tool-failure recovery paths are not fully migrated to emit it through `RepairTargetDecision` yet.

## WP8: Actor Loop Phase Refactor

- No new behavior regression was observed in docs, Python, or hard Node smoke.
- `run_actor_loop` remains large. WP8 only moved phase-event construction and prepared-tool counter mutation; future slices should keep extracting local transition/data builders instead of adding semantic task branches.

## WP9: TurnState Ownership

- The CLI rejects `--resume` with `--oneshot` / `-p`; two-turn smoke used the same state directory and workdir without `--fresh-session` to continue the latest session.
- Docs -> coding continued session passed, but coding -> data continued session failed: the data-only prompt was admitted into a coding/test-evidence path, wrote `tests/test_main.py`, and stopped at `safe_stop_verifier_missing` without creating `output.csv`.
- This is not caused by verifier event dedup storage. It indicates remaining semantic/session contamination where current-turn ObjectiveContract construction can still inherit stale coding/evidence pressure from prior turns or existing project artifacts.
- Fixing the coding -> data failure should happen at objective contract admission / deliverable obligation construction, not by adding another path-specific reset field or CSV string rule.

## WP10: 20-Run Regression Guard

- WP10 reached 14/20 pass and 14/20 high-quality, but the strict regression guard is not fully passed because false terminal alignment remains.
- Python sales recovered in this sample at 3/3, but this is a 20-run guard, not yet a 50-run improvement claim.
- Data CSV produced an artifact and exited `done`, but the file included an extra `same` column. This is a false-done style content acceptance issue.
- TOML passed the external deterministic grader 2/2, but both runs exited `safe_stop_verifier_missing`; internal evidence binding still disagrees with real artifact/evidence completion.
- FastAPI failed 2/2 with `repair_exhausted` around 422 response mismatches, indicating API request-schema repair is still weak.
- Research and ops tasks stopped at `missing_repo_edits` without creating artifacts, showing non-coding admission can still inherit edit-obligation pressure incorrectly.
- Rust NDJSON failed because generated tests imported `merge_ndjson_lines` from the wrong root path; Rust source/test binding drift remains.

## WP11: 50-Run Improvement Claim

- WP11 reached 38/50 pass and 35/50 high-quality in the focused matrix. This is a strong success-rate signal, but not a clean unconditional improvement claim because false-done and false-missing remain.
- PAM was attempted for 25 rows, but Photon context/evaluate failed and no memory was injected. PAM effectiveness was not actually measured.
- Python sales recovered strongly at 8/8, and Node JSON/CSV remained stable at 10/10 combined.
- TOML was externally valid 6/6, but only 4/6 high-quality because internal terminal states still reported verifier-missing or repair-exhausted. Evidence binding remains misaligned with real artifact success.
- Data CSV failed 0/2 with `done` despite schema/content mismatch. Typed schema evidence is needed for data tasks.
- Research and ops failed 0/2 each with `missing_repo_edits`; non-coding objective admission still carries coding/edit-obligation leakage.
- FastAPI failed 5/6 with `repair_exhausted` around 422 request/response mismatches. Generic API schema reconciliation remains a high-priority repair gap.
- Python markdown had one externally passing row with `repair_exhausted`; terminal/evidence alignment remains incomplete even when artifacts pass local checks.

## WP-C: Non-Coding DeliverableObligation

- Initial mixed smoke recovered Research but Ops remained 0/6 with `missing_repo_edits`. Log inspection showed the model attempted the correct `Write(reports/health-check.md)`, but controller policy had selected a local small-edit target on `scripts/health.sh` and exposed only `Edit`.
- Root cause was contract admission, not model unwillingness: command-observation report requests could still be classified as Coding or Research before reaching Ops.
- After separating command-observation artifacts from implementation inference, Ops recovered to 6/6 pass and `done`.
- Ops high_quality remains 3/6 because some runs create temporary stdout/stderr/exit-code files outside the requested report artifact. This should be addressed as artifact-scope hygiene, not by weakening terminal projection.

## WP-D: EvidenceBinding / Terminal Projection

- TOML false-missing recovered in the focused smoke: 4/4 TOML and 7/7 mixed coding rows exited `done` with `false_missing=0`.
- The immediate cause was over-broad generated-test non-ASCII rejection: prose/docstring literals were treated as unsupported assertions and removed the explicit test artifact from verifier binding.
- The preflight predicate is now assertion-context scoped, but it remains a deterministic safety boundary. Future expansion should keep it tied to typed assertion/evidence intent, not benchmark text.
- All-candidate preflight rejection is now observable as weak binding metadata, but there is still no dedicated repair operator for rejected generated-test content.
- Real LLM validation requires localhost Ollama access; sandboxed evaluation fails before model invocation if `127.0.0.1:11434` is blocked.

## WP-F: Current-Turn ObjectiveContract Authority

- Continued-session real LLM smoke recovered the WP9-style contamination scenario in the focused sample: coding -> data, coding -> docs, docs -> coding, and data -> TDD all passed 4/4 high-quality.
- This supports the narrow hypothesis that current user prompt authority must dominate stale `working_memory.active_task`; it is not yet a broad success-rate claim.
- `docs_to_coding` externally passed but reported `repair_safe_stop`, so terminal projection can still disagree with externally valid artifacts/evidence.
- The first setup turn in `coding_to_data` asked for `package.json` plus `src/add.js`, but the contract only required `src/add.js`; this is acceptable for stale-context setup but should not be used as coding completeness evidence.
- Real LLM validation still requires unsandboxed localhost Ollama access.

## WP-E: API Contract Expectation

- Typed API expectations are now extracted and projected into generation, artifact-directed recovery, semantic shadow logs, and repair deltas without FastAPI-specific matching.
- Real LLM validation improved the focused mixed smoke from 3/6 to 4/6 high-quality. FastAPI moved from 0/3 to 1/3, while Node, Python, and docs regressions remained green.
- JSON body binding improved materially: after first-write artifact-directed context was connected, 3/3 FastAPI rows generated body-object style implementations in `wp-e-api-contract-smoke5-20260610`.
- The remaining FastAPI failures are still convergence problems: one row ignored the JSON body binding even with typed context present, and another row invented exact `201` status despite `expected_status=unspecified`.
- Repair diagnosis can identify the right hypothesis, but bounded repair proposals may still be rejected as malformed or ambiguous before convergence.
- Future work should add evidence-side API contract observation and repair binding, not framework-specific string rules.
