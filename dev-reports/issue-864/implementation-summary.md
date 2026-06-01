# Issue 864 Implementation Summary

Implemented a generalized deliverable obligation model while preserving the existing role-based task-contract flow.

- Extended `ArtifactObligation` with a `DeliverableKind`, README required sections, and structured-record schema columns.
- Added `ArtifactRole::DataOutput` and `RepoEditCategory::Data` for CSV/TSV/JSONL-style output files.
- Added Node/Python CLI conventional file obligations for setup/package files, implementation, tests, and README.
- Added docs-only README section validation through the existing excerpt-based recovery gate.
- Added data-output column validation through structured-record obligations and excerpt inspection.
- Updated role/path admission, project probing, scaffold snapshot mapping, repair ranking, and repair-brief role parsing for the new data-output role/category.

Focused tests were added for:

- Node CLI separate required obligations.
- Python CLI not completing from `main.py` alone.
- README required section validation.
- Structured output-file column validation.
