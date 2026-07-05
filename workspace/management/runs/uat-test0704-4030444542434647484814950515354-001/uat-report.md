# UAT Report: G3 Generic -> Profile Promotion Micro UAT

## Summary

- Result: FAIL
- Workspace: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-4030444542434647484814950515354_001`
- Binary: `anvilminimal 0.1.0 746a1945 2026-07-04T15:40:53Z`
- Model: `gemini-3.1-flash-lite`
- Planner: `gemini-3.5-flash`
- Provider: `gemini`
- Prompt:

```text
/ultra-plan-run ちょっとしたメモアプリを作って。ブラウザで使えるようにしてください。
```

The run correctly started as `generic` and bound the generic app contract, but it did not promote to a concrete web profile. It stayed `generic/reduced`, generated a Vite/React app, and failed during phase 2 verification before browser readiness or interaction acceptance could run.

## Preflight Evidence

- Git HEAD before UAT: `746a1945 C: cover generic profile promotion in ultra runs`
- Installed binary:

```text
/Users/maenokota/.local/bin/anvilminimal
anvilminimal 0.1.0 746a1945 2026-07-04T15:40:53Z
```

- Interaction probe dependency existed:

```text
/Users/maenokota/.anvil/tools/interaction-probe/node_modules/playwright/package.json
```

- Ports `3000` and `3011` were cleared before execution.
- `/setup-interaction-probe` completed with `probe ready`.

## Expected G3 Path

1. Start as `generic` because the goal intentionally avoids known profile tokens.
2. Bind the G1 generic interactive app contract.
3. Re-infer a concrete profile after scaffold/workspace evidence appears.
4. Merge promoted profile requirements into the next phase prompt.
5. End with promoted profile final acceptance, browser/interaction evidence, and `assurance_level=full`.

## Actual Result By Stage

| Stage | Result | Evidence |
|---|---:|---|
| Generic start | PASS | `run_start` and `/ultra-plan-run` `tui_command_start` show `profile:"generic"` and empty `profile_inferred`. |
| G1 contract bind | PASS | `completion_contract_bound` and `generic_contract_bound` were emitted. Required evidence: `user_input_handler_evidence`, `stateful_update_evidence`, `visible_interactive_surface_evidence`. |
| Profile re-inference | FAIL | No `profile_reinferred` event was emitted. Summary remained `Profile: generic`. |
| Promoted profile delta merge | FAIL | No promoted profile was available. No Next.js/browser profile requirements were merged. |
| Full final acceptance | FAIL | Run stopped in phase 2. `Final acceptance: not_checked`, `Browser readiness: not_applicable`, `Interaction evidence: not_applicable`, `Assurance: reduced`. |

## Terminal Failure

The task stopped in phase `implement-memo-logic`.

```text
Status: failed
Completion status: incomplete
Command status: failed
Task status: failed
Runtime acceptance: not_checked
Final acceptance: not_checked
Profile: generic
Assurance: reduced (generic profile - no capability contract, no behavioral verification)
Completed phases:
- setup-project (completed)
Failed phases:
- implement-memo-logic (failed)
Pending phases:
- implement-ui-components (pending)
- final-verification (pending)
```

Primary stop reason:

```text
phase implement-memo-logic failed: step verify-memo-tests failed verification after bounded repair:
command failed: npx vitest run src/utils/memoService.test.ts
ReferenceError: window is not defined
```

## Generated Artifacts

An UltraPlan was generated:

```text
.anvil/plans/ultra-plan-019f2dd7-20d5-7830-8f41-3500da293a5d.yaml
```

It planned a Vite/React/Tailwind app while keeping `profile: "generic"`:

```yaml
goal: "ちょっとしたメモアプリを作って。ブラウザで使えるようにしてください。"
profile: "generic"
phases:
  - id: "setup-project"
  - id: "implement-memo-logic"
  - id: "implement-ui-components"
  - id: "final-verification"
```

Important generated files:

```text
package.json
index.html
vite.config.ts
tailwind.config.js
postcss.config.js
tsconfig.json
src/App.tsx
src/main.tsx
src/index.css
src/utils/memoService.ts
src/utils/memoService.test.ts
```

The partial UI in `src/App.tsx` is only a textarea-backed memo shell. It has visible input and state update locally, but CRUD/persistence integration was not completed because the run stopped before UI integration and final verification.

## Failure Details

`src/utils/memoService.ts` uses browser `localStorage` directly:

```ts
const data = localStorage.getItem(STORAGE_KEY);
localStorage.setItem(STORAGE_KEY, JSON.stringify(memos));
```

`src/utils/memoService.test.ts` installs a mock with:

```ts
Object.defineProperty(window, 'localStorage', {
  value: localStorageMock,
});
```

Vitest was invoked as:

```text
npx vitest run src/utils/memoService.test.ts
```

The test ran in the default Node environment, so `window` was undefined. Although `jsdom` was present in `package.json`, no Vitest `jsdom` environment was configured before verification. Bounded repair did not resolve that mismatch.

## Recovery Evidence

Recovery handoff worked and did not mark the task as successful.

Saved files:

```text
.anvil/repairs/repair-phase-implement-memo-logic-019f2dd9-101c-7a03-81a5-c8170197a6eb.md
.anvil/plans/recovery-ultra-plan-phase-implement-memo-logic-019f2dd9-101c-7a03-81a5-c82922ae4b8a.yaml
```

Suggested commands:

```text
/ultra-plan-run --profile generic "$(cat .anvil/repairs/repair-phase-implement-memo-logic-019f2dd9-101c-7a03-81a5-c8170197a6eb.md)"
/run-ultra-plan .anvil/plans/recovery-ultra-plan-phase-implement-memo-logic-019f2dd9-101c-7a03-81a5-c82922ae4b8a.yaml
```

The recovery YAML parses and contains focused `inspect-current-state`, `repair-implement-memo-logic`, and `verify-recovery` phases.

## Issues Found

1. Profile promotion did not happen.

   The expected G3 path requires a concrete web profile after scaffold evidence appears. The workspace contained a Vite/React manifest and browser app files, but no `profile_reinferred` event appeared. The run remained `generic`, and the summary stayed `Assurance: reduced`.

2. Browser app test verification is not environment-aware enough.

   The generated memo service and test are browser-oriented, but the verify command used Vitest without `jsdom` configuration. This produced `ReferenceError: window is not defined`. Either the verifier/repair path should steer the implementation toward environment-independent storage injection, or it should repair browser-global tests by configuring the test environment.

3. Final acceptance was never reached.

   Browser readiness, interaction probe, release gate, and final acceptance were all `not_applicable` or `not_checked` because phase 2 failed.

4. Summary has two lifecycle blocks and can be misread.

   The command block correctly says the task failed. The later process block says the REPL exited cleanly and shows `Task status: completed (reduced assurance)`. This is technically process-level status, but for UAT readers it is easy to confuse with task success.

5. Completion contract information was not fully carried into the failed command stop summary.

   Events show `completion_contract_bound` and `generic_contract_bound`, but the failed `tui_command_stop` and summary report `completion_contract_generated=false` / `completion_contract_path=missing`. This weakens post-run diagnosis.

## Interpretation

This run is useful because it proves the G1 generic contract path is alive and recovery handoff is working. It does not prove G3. The core G3 requirement, live promotion from generic to a concrete profile with full assurance, was not observed.

The immediate blocker is not merely model output quality. The system generated enough workspace evidence to identify a browser app shape, but the runtime stayed generic and then verified browser-specific code with a Node-only test environment.

## Follow-Up Prompt

```text
G3 failed in /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-4030444542434647484814950515354_001.

Please fix the G3 blockers:
1. Ensure a generic ultra run can promote from workspace evidence when the scaffold creates a browser app manifest and entrypoints. If Vite/React is intentionally not a known profile, either add a web-app promotion target or record an explicit non-promotable reason in events/summary.
2. Preserve the generic contract bind evidence in failed tui_command_stop and summary output.
3. For browser-oriented unit tests, make bounded repair environment-aware: if verification fails with `window is not defined`, repair either by configuring Vitest `environment: jsdom` or by refactoring storage access behind an injectable/mocked boundary.
4. Keep recovery handoff behavior unchanged: failed task must remain failed, and recovery `.md` plus recovery UltraPlan YAML must be saved.
5. Re-run the same G3 micro UAT and require: `generic_contract_bound`, `profile_reinferred` or explicit non-promotable reason, final command status that cannot be confused with REPL process status, and no static/reduced assurance success once a concrete web app shape exists.
```
