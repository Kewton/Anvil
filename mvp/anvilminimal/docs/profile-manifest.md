# Profile manifest v0

`ManifestV0` is the draft profile format used for the rule-of-two work. Its
scope is deliberately narrow: preserve the declarative values in the current
Next.js knowledge, make every verification binding catalog-addressable, and
let the B-2 data profile be written far enough to expose real format gaps.
Schema v1 is not implied by this draft.

The embedded Next.js manifest is loaded independently of the existing runtime
knowledge loader. Its status is `draft`, and no planner or minimal-loop path
consults it yet. During this parallel period, Next.js behavior continues to
come from `mvp/anvilminimal/src/planner/profiles/nextjs/knowledge.rs` and
`mvp/anvilminimal/src/minimal_loop/evidence_knowledge.rs`.

## Ownership model

Layer 1 is shared Rust mechanism and shared knowledge. It owns interpretation,
execution, safety, and cross-profile contracts. Layer 2 is the profile
manifest. It owns declarative profile choices and may reference Layer-1 shared
knowledge, but it cannot replace Layer-1 behavior with executable text.

| Section | Layer-2 declaration | Layer-1 owner or reference boundary |
| --- | --- | --- |
| `metadata` | Profile `id`, display name, schema version, and admission status. | The loader validates `v0` and `draft`/`admitted`. B-3 may later use `status`; v0 does not gate execution. |
| `plan` | Preset style, intent, ordered UltraPlan phase ids/prompts, the required literal `{goal}` placeholder, and the optional literal `{port}` placeholder. | UltraPlan construction, placeholder expansion, scheduling, and phase execution remain Rust mechanisms. |
| `step_templates` | Scaffold/build-verification match words, implementation-kill words, ownership markers, and inert template artifact bytes moved from `knowledge.toml`. | Matching precedence, kill decisions, template selection, placeholder expansion, and file writes remain Rust mechanisms. |
| `vocabulary` | A typed reference to `evidence_knowledge` sections `vocabulary` and `goal_hints.translations`. | The values remain single-sourced in `mvp/anvilminimal/src/minimal_loop/evidence_knowledge.toml`; evidence scanning and translation behavior remain Layer 1. |
| `guidance` | The `generic`, `canvas_game`, and `persistence` repair variants plus contract wording. | Failure classification, variant selection, ordering, and deduplication remain Rust mechanisms. |
| `checks` | Named arrays of catalog bindings: an `id` and typed `params`. | `mvp/anvilminimal/src/planner/capability_catalog.rs` owns parameter schemas, validation, adapters, and execution. |
| `evidence_targets` | Either a typed reference to shared `repair_targets` or a profile-local evidence-kind-to-path mapping. | Shared mappings remain single-sourced; local paths are validated as workspace-relative. Route closure and repair-target selection remain Rust mechanisms. |

The vocabulary section uses a reference instead of copied arrays. Next.js also
references its shared evidence-target mapping. The data profile has different
repair targets, so it uses the mutually exclusive local mapping form rather
than copying or modifying the Next.js shared mapping. This keeps each mapping
single-sourced while preserving a typed manifest boundary.

## Schema and load contract

The root section list is fixed by
`mvp/anvilminimal/tests/golden/profile_manifest_v0_sections.txt`:

1. `metadata`
2. `plan`
3. `step_templates`
4. `vocabulary`
5. `guidance`
6. `checks`
7. `evidence_targets`

Unknown root or nested fields are rejected. Metadata enums are closed, phase
ids must be unique, required strings must be non-empty, and `plan.profile`
must equal `metadata.id`. `{goal}` is required; `{port}` may be omitted by a
profile that has no port semantics, but when present it must be the exact
literal token. Neither field permits arbitrary interpolation syntax.

Check bindings use TOML arrays under a logical binding name:

```toml
[[checks.project-setup]]
id = "package_json_port_script"
params = { port = 3011 }
```

`ManifestV0::from_toml()` calls `resolve()` before returning. Every entry is
resolved through the capability catalog at load time. An unknown id, missing
or extra parameter, wrong type, unsafe path, unsupported value, or registered
but unimplemented adapter makes the manifest load fail. There is no permissive
fallback and no deferred runtime validation.

`nextjs_manifest()` follows the existing knowledge-loader pattern: the TOML is
embedded with `include_str!`, parsed once through `OnceLock`, and an invalid
embedded manifest panics immediately. Calling the new loader is not wired into
the Next.js execution path in v0.

## Intentionally unsupported

Manifest v0 has no representation for:

- free-form shell commands or command fragments;
- conditionals, predicates, loops, or profile-authored branching;
- executable template selection, expansion, write, or repair logic;
- capability implementations, probe dispatch, or validation weakening; or
- runtime admission and fallback policy.

The artifact strings under `step_templates` are inert data retained from the
existing Next.js knowledge. They do not make template logic declarative. A new
verification need must be added to the capability catalog with its schema,
adapter, golden update, and tests; it must not be smuggled into a manifest as a
shell string.

## Format-gap ledger

This section is the canonical format-gap list for B-2 and B-4. B-2 must append
a row whenever a data-profile requirement cannot be represented faithfully by
v0. B-2 must not loosen validation or add one-off executable fields to avoid
recording a gap.

Each row uses a stable `FG-B2-NNN` id and records the source requirement, the
attempted v0 mapping, the missing semantic, and a concrete fixture or test.
During B-4 every open row must receive one of two dispositions: `resolved` by a
schema-v1 change justified by both profiles, or `accepted-layer1` because the
semantic intentionally remains shared mechanism. The decision and validating
test stay in the table; rows are not silently deleted. Schema v1 cannot be
declared complete while a row remains `open`.

| Gap id | B-2 requirement and source | Attempted v0 mapping | Missing semantic | Fixture/test | B-4 disposition |
| --- | --- | --- | --- | --- | --- |
| `FG-B2-001` | Data plan/guidance from contract §8 | Populate the existing `step_templates` and T28-shaped `guidance` slots with data keywords and deterministic pipeline guidance. | v0 has Next.js-shaped artifact and guidance field names rather than profile-neutral typed variants; unrelated inert fields must remain empty. | `planner::profiles::data::manifest::tests::manifest_drives_plan_guidance_requirements_and_repair_targets` | `open` |
| `FG-B2-002` | Contract §1 permits `output/report.{html,md}` | B-2b fixes `output/report.md` as the required v0 artifact and permits `output/report.html` only as an additional artifact; E2 continues to inspect either present report. | v0 artifact lists cannot express an exactly-one-of path requirement without conditional manifest logic, so the mandatory Markdown path is an intentional Layer-1 policy choice. | `data_manifest_knowledge_matches_b2b_golden`; `planner::profiles::data::runtime::tests::manifest_dispatch_produces_full_only_after_all_checks_pass` | `accepted-layer1` |
| `FG-B2-003` | Contract §4 earned assurance hierarchy | Bind all checks declaratively, then classify observed check evidence in the typed data runtime adapter. | `full`/`partial`/`static`/`failed` depends on execution history and cannot be safely expressed as manifest branching. | `planner::runner::tests::assurance_tests::moved::data_assurance_is_earned_from_the_observed_profile_probe_level` | `open` |

B-2 の data プロファイル実装は docs/data-profile-contract.md（fixed）に適合しなければならない。契約との不整合はスキーマ側でなく実装側の問題として扱う。

## Promotion boundary

`draft` and `admitted` are the only v0 status values. This task only reserves
the field. B-3 owns any admission gate, and B-4 owns format-gap settlement and
schema-v1 finalization. Until a later change explicitly wires an admitted
manifest, the legacy Next.js loaders remain the sole runtime source of truth.
