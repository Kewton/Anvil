# Residual Known Issues After RWP-0 to RWP-10

Date: 2026-06-10

## Summary

RWP-0 through RWP-10 materially improved the main evaluation suite, but the
architecture is not finished. The remaining issues are concentrated in
feature-improvement admission, terminal cleanup, and evidence-repair
convergence.

## Known Issues

### 1. Feature-improvement tasks can stop before useful editing

Observed in:

- `rwp10-feature-supplement-no-pam-20260610`: 0/2 high_quality
- `rwp10-feature-supplement-pam-20260610`: 0/2 high_quality

Signature:

- terminal: `missing_repo_edits`
- duration: about 1.4 seconds
- changed files: only `tests/__pycache__/...`
- `discounts.py` remained unchanged

Interpretation:

The controller can run verifier/completion logic before turning the current
feature request into a required behavior delta. Existing passing tests are not
enough evidence for a requested behavior change.

Next hypothesis:

- Introduce a typed "behavior delta required" obligation for existing-project
  feature requests.
- The obligation should be derived from ObjectiveContract/current request
  semantics, not from case-specific filename matching.

### 2. Functional success can still end as repair_exhausted

Observed in:

- RWP-10 `python_markdown` no_pam and pam rows.

Signature:

- pass: true
- high_quality: false
- terminal: `repair_exhausted`
- shadow terminal: `evidence_repair_exhausted`

Interpretation:

The implementation can be functionally acceptable while evidence repair fails
to converge. This should remain non-HQ, but the system should produce a more
actionable next repair target than a generic exhausted terminal.

Next hypothesis:

- Project evidence repair state from typed EvidenceObservation into a concise
  repair target.
- Avoid adding benchmark-specific text checks for Markdown.

### 3. Active terminal and shadow terminal can still conflict

Observed in:

- RWP-10 `toml_merge` no_pam row.
- RWP-10 `node_csv` no_pam row.

Signature:

- active terminal: `max_iterations`
- shadow terminal: `success`
- `shadow_conflict=1`

Interpretation:

The read-only shadow projection can see success, but the active lifecycle can
continue until max iterations. This is a remaining convergence-control issue.

Next hypothesis:

- Move active terminal projection closer to the typed success observation
  already used by shadow terminal.
- Keep the adoption narrow and guarded by focused tests plus LLM smoke; do not
  add task-specific terminal shortcuts.

### 4. PAM is observable but not injected

Observed in:

- RWP-7 PAM smoke.
- RWP-10 all `pam` rows.

Signature:

- `pam_availability=failed`
- `pam_unused_reason=context_pack_failed:sidecar_call`
- `pam_failure_phase=sidecar_call`
- `pam_injected_count=0`

Interpretation:

PAM reporting is now clearer, but PAM is not contributing context. Current
evaluation cannot claim PAM benefit.

Next hypothesis:

- Treat PAM availability as a first-class execution diagnostic.
- Fix sidecar context-pack failure separately before using PAM in improvement
  claims.

### 5. Main suite coverage still underrepresents feature tasks

Observed in:

- Built-in `wp11` does not include `feature_discount`.

Interpretation:

The headline 50-run can look strong while missing an important class of
existing-project feature-improvement tasks.

Next hypothesis:

- Update future benchmark suites to include feature improvement and TDD
  behavior-delta tasks as first-class categories.
- Report task-kind high_quality separately; do not rely only on aggregate
  success rate.

## What Not To Do

- Do not add filename-specific or benchmark-specific rules for
  `discounts.py`, `markdown_lint.py`, or CSV whitespace.
- Do not treat PAM-failed rows as PAM-injected improvement.
- Do not collapse feature-improvement into generic coding without a typed
  behavior-delta obligation.
- Do not make `task_contract.rs` the dumping ground for the next fix; continue
  extracting projection/admission/evidence boundaries.
