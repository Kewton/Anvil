# Issue 864 Design

`ArtifactObligation` remains the task-contract obligation record, but it is generalized from "role + file path" to a deliverable record with a deliverable kind and optional validation detail. Existing role-based completion stays intact: file obligations still map to `ArtifactRole`, evidence still arrives through repo-edit categories, and path-specific identities remain the conservative gate for required files.

The small change is to extend the obligation model in place:

- Add deliverable kinds for `file`, `directory`, `command_output`, `structured_record`, and `external_reference`.
- Add optional required README sections and structured-record columns to obligations.
- Add a `DataOutput` role and `Data` repo-edit category so CSV-like output files can be tracked as first-class deliverables.
- Infer conventional CLI obligations for Node/Python CLI tasks where the request asks for package/setup, implementation, tests, and README.
- Validate docs-only README section coverage and structured-record column coverage through the existing post-edit excerpt sidecar.

This avoids a new provider or planner abstraction. The existing `CompletionPolicy`, artifact-state projection, and recovery flow continue to use roles as the coarse scheduler while `ArtifactObligation` carries the more precise deliverable identity and schema details.
