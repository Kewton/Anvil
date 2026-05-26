# Generic Evaluation Matrix

## Purpose

This matrix keeps evaluation generic and prevents overfitting to a single
FastAPI or Rust case.

## Case Set

| ID | Task Type | Prompt Shape | Expected Terminal | Forbidden Terminal |
| --- | --- | --- | --- | --- |
| PY-CRUD | Python API | Build a CRUD API with README and tests | `done` or actionable safe stop | wrong-stack verifier `done` |
| RUST-LIB | Rust library | Build a library with README and `cargo test` tests | `done` or actionable safe stop | Python verifier `done` |
| NODE-CLI | Node CLI/package | Build a CLI with README and npm test | `done` or actionable safe stop | unrelated verifier `done` |
| DOCS-ONLY | Documentation | Write usage docs for an existing project | `done` after docs evidence or safe stop | implementation-only `done` |
| EXISTING-MOD | Existing project change | Modify behavior in existing files and tests | `done` or safe stop | edits outside task scope |
| MULTI-DIR | Multi-directory app | App spanning src/tests/config/docs | `done` or safe stop | single-file assumption |
| MISSING-VERIFIER | Tests exist but manifest missing | Add setup metadata or safe stop | `done` after real verifier pass | no-verifier `done` |
| AMBIG-ASSERT | Generated ambiguous assertion | Clarify or safe stop | actionable safe stop | forced implementation mutation |

## Metrics

- terminal class
- verifier command
- project unit
- edited files
- false-positive `done`
- wrong-stack verifier success
- repair loop exhaustion
- actionable safe-stop quality
- iteration count

## Development Gate

Run first:

- no-PAM 5 cases
- PAM 5 cases

Run after structural tests pass:

- no-PAM 20 cases
- PAM 20 cases

## Acceptance

- false-positive `done`: 0
- wrong-stack verifier success: 0
- uncontrolled `missing_repo_edits` during verifier repair: 0
- safe stops must include `blocker_class`, `authority_status`, and
  `next_user_action`
