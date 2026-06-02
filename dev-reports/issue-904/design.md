# Design Note

Issue #904 focuses on evaluation taxonomy.

Plan:

- add a bounded `evaluation_taxonomy` object to `EvalRecord`,
- record PAM variant separately from terminal outcome,
- infer task kind from the task text for machine-readable aggregation,
- expose outcome agreement as `external_postcheck_unavailable` unless an external harness supplies that signal later,
- preserve existing schema as an additive field.

