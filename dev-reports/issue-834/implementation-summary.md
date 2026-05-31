# Issue 834 Implementation Summary

## Investigation Result

The failing Node CLI path is reproducible without Ollama by following the pure gates for this prompt:

```text
Node.jsでToDo管理CLIを開発してください。README.mdとテストコードも作成してください。
```

Observed gate chain:

1. `src/modes/plan_act.rs:741` classifies this exact Node CLI request as `WorkMode::GenericCode`, not `TypeScriptUi`.
2. `src/modes/plan_act.rs:99` maps `GenericCode` to a policy with `allow_ui_deterministic_fallback: false` and no Python/docs deterministic fallback.
3. `src/agent/loop_run/scaffold_pipeline.rs:751` only returns deterministic file scaffold specs for Python or docs policies. There is no Node CLI deterministic scaffold spec, so no deterministic writer can create `package.json` for this request.
4. `src/agent/loop_run/scaffold_pipeline.rs:148` recognizes only `next.js` / `nextjs`, `nuxt`, and `react` as scaffold frameworks. The Node CLI prompt returns `None`.
5. Because the framework detector returns `None`, `src/agent/loop_run/scaffold_pipeline.rs:667` does not activate the empty-workspace scaffold policy error, and `src/agent/loop_run/build_request_messages.rs:122` does not add the framework-specific scaffold-now note. The model receives only the generic empty-workspace note from `src/agent/recovery.rs:174`.
6. If the model replies without a repo edit, missing-repo recovery reaches `src/agent/loop_run/actor_loop_flow.rs:713`, but `maybe_apply_deterministic_nextjs_scaffold` exits at `src/agent/loop_run/scaffold_pipeline.rs:897` because `active_task_requires_nextjs_scaffold` is false for Node CLI. It therefore returns `NotApplicable`, and the generic no-edit retry continues without generating `package.json`.

Adjacent non-Node failure gate:

- Even for a real Next.js request, the deterministic CLI fallback is skipped in offline mode or in non-interactive mode without `--yes` at `src/agent/loop_run/scaffold_pipeline.rs:657`. That is a separate `Skipped` path, not the Node CLI `NotApplicable` path.

## Skeleton Separation Finding

Skeleton separation alone does not fix the Node CLI case. The observed Node CLI request never reaches a Node skeleton/scaffold writer: it is classified as `GenericCode`, has no deterministic Node scaffold spec, is not recognized as a framework scaffold request, and fails the Next.js-only fallback predicate. Separating scaffold skeleton from post-scaffold implementation would help only after a Node CLI scaffold admission path exists.

For the adjacent Next.js non-interactive skip, skeleton separation also does not address the immediate gate because the fallback is blocked before launch by the offline / `--yes` / TTY check.

## Code Change

Added focused assertions in `src/agent/loop_run/truncate_tests.rs` that pin the finding:

- `requested_scaffold_framework("Node.js...CLI...") == None`
- `task_or_plan_requires_nextjs_scaffold("Node.js...CLI...") == false`

No production behavior was changed in this investigation task.
