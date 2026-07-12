# Integration Notes

## Repair Pressure Integration (2026-07-11)

The following pre-existing gaps were observed while consolidating repair
pressure. They are recorded here because this integration preserves behavior
and does not change thresholds, events, payloads, or terminal reasons.

### No-Progress Does Not Raise Write Pressure

The no-progress streak drives `no_progress_feedback` and the
`model_stagnation:no_progress_recorded` terminal path, but it does not activate
`write_required`. Repeated no-progress turns can therefore terminate without a
write-required intervention. The state-machine transition table and unit test
preserve this gap explicitly. Closing it requires a separate measured task with
a non-regression gate.

### Dependency Setup Can Stall Network-Restricted Tests

`plan_run_nextjs_game_setup_only_fails_inferred_obligation` can enter the real
Node dependency-setup path. Under a network-restricted test environment,
`npm install` can wait for the existing 600-second setup timeout. The test
passes when dependency setup can complete. This integration does not change
the setup decision or timeout.

## Frozen Exception: Template-Owned Implement Steps (2026-07-12)

UAT `test0712_bs_001` run 3 produced an `ensure-port-scripts` step classified
as `implement`. Its package-script objective was already satisfied, but the
Task 24 preset conversion and short-circuit gates both required `setup`, so the
run exhausted through `model_stagnation:no_progress_recorded` four times.

This bounded exception replaces the kind/name gate only for steps that
explicitly reference template-owned artifacts through `expected_paths`, the
planner-authored instruction, the step id, or deterministic verify commands.
Package scripts/ports and known scaffold configuration files qualify;
ambiguous text such as "configure the project" does not. A route-bound game
implementation and `npm run build` alone remain outside the predicate. The
generic profile contract appended by the runner is also excluded so it cannot
reclassify ordinary implementation steps.

The exception permits kind-independent preset verification conversion,
profile-check prechecks, and the existing verification-bearing no-progress
feedback for this artifact class. It does not change the repair-pressure
transition table, escalation thresholds, event names or payloads, or terminal
reason strings. Converted verification paths are evidence dependencies rather
than mutating ownership claims, so they do not conflict with an implementation
step that owns the same path; duplicate ownership between mutating steps still
fails lint. A failing port check still enters the executor repair path.

## Declarative Knowledge Extraction (2026-07-12)

The following values moved without byte changes to embedded TOML files:

- `src/planner/profiles/nextjs/knowledge.toml`: preset UltraPlan phase ids and
  prompts; deterministic scaffold, port, build, and implementation matcher
  tokens; setup and template-owned artifact tokens; state-binding and contract
  attribute guidance; canonical package scripts, required hooks, scaffold
  paths, and scaffold file bodies.
- `src/minimal_loop/evidence_knowledge.toml`: visible-surface, input-handler,
  state-update, adversary, motion, score, failure, restart, and persistence
  vocabulary; goal stopwords and Japanese-to-English translations; behavioral
  evidence keys and ordered Next.js repair target candidates.

Both files are embedded with `include_str!`, parsed once through `OnceLock`,
and have no runtime fallback. Migration golden tests fixed the old Rust values
before the final reference switch; retained tests cover the embedded body,
ordering, translation lookup, contract wording, and canonical JSON structure.
No functional issue was discovered during this extraction. The initial
sandboxed full test run could not bind local browser-probe ports; the same test
command passed when run with local-port permission, without a code change.
