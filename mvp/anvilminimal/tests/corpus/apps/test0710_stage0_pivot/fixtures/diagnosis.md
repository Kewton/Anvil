# Stage-0 Diagnosis

- `test0710_camp_001` is not present in the local corpus checked under `mvp/anvilminimal`.
- The same stale-path injection shape is represented by the prior stale-path fixture: the first foreign workspace literal appears in a model-authored Bash command argument, not in earlier tool output or feedback.
- Near-name reconstruction hypothesis for the available fixture: supported. The stale workspace string appears in model-authored args; no tool output/feedback introduced it before the model used it.
- Runtime classification added: writes whose absolute path matches the current root with digit variance are rejected as `tool_args_path_near_root_corruption`, with feedback quoting the exact current root and no cross-workspace salvage.
