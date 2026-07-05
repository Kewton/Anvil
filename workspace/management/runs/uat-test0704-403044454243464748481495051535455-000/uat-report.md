# UAT Report: G3 Revalidation With Non-3000 Port

## Summary

- Result: FAIL
- Important finding: generic -> nextjs live promotion was observed, but the command aborted due to a Rust panic before final acceptance.
- Workspace: `/Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-403044454243464748481495051535455_000`
- Binary: `anvilminimal 0.1.0 1468c3f0 2026-07-04T23:41:35Z`
- Model: `gemini-3.1-flash-lite`
- Planner: `gemini-3.5-flash`
- Provider: `gemini`
- Port: `3011` was used instead of `3000` per user instruction.

Prompt:

```text
/ultra-plan-run ちょっとしたメモアプリを作って。ブラウザで使えるようにしてください。フレームワークはNode.jsエコシステムの標準的なフルスタック構成を選んでください。3011ポートで起動可能にしてください。
```

## Preflight Evidence

- Git HEAD:

```text
1468c3f0 D: harvest G3 corpus and close assurance track
9e45c614 B+C: document ambiguous generic assurance path
545c328d A: default Next.js no-port runs to 3011
746a1945 C: cover generic profile promotion in ultra runs
```

- Installed binary:

```text
/Users/maenokota/.local/bin/anvilminimal
anvilminimal 0.1.0 1468c3f0 2026-07-04T23:41:35Z
```

- Interaction probe existed:

```text
/Users/maenokota/.anvil/tools/interaction-probe/node_modules/playwright/package.json
```

- `/setup-interaction-probe` completed with `probe ready`.
- Port `3011` was free before execution.
- Port `3000` had existing processes and was intentionally not touched after the user instructed to use a non-3000 port.

## Expected G3 Revalidation Criteria

1. Start as `generic`.
2. Bind the generic app contract.
3. Re-infer a concrete profile from workspace evidence after scaffold.
4. Merge promoted profile requirements into subsequent phase prompts.
5. Reach final acceptance with the effective promoted profile and full assurance, or fail for a real capability/runtime reason without false success.

## Actual Result By Stage

| Stage | Result | Evidence |
|---|---:|---|
| Generic start | PASS | `run_start` and `/ultra-plan-run` `tui_command_start` show `profile:"generic"` and empty `profile_inferred`. |
| G1 contract bind | PASS | `generic_contract_bound` emitted with matched token `アプリ` and generic interactive evidence keys. |
| Profile re-inference | PASS | `profile_reinferred` emitted: `generic -> nextjs`, `from:"workspace"`, `at_phase:1`, `phase_id:"setup-framework"`. TUI printed `Profile promoted: generic -> nextjs (workspace evidence, phase 1)`. |
| Promoted profile delta merge | PASS | After promotion, the next phase contract required `nextjs_route_evidence`, `build_command_or_dependency_missing_boundary`, and Next.js paths including `src/app/layout.tsx`, `src/app/page.tsx`, `src/app/globals.css`, and `src/app/global.d.ts`. |
| Full final acceptance | FAIL | Process aborted before final acceptance due to a Rust panic. Browser readiness and interaction evidence remained `not_applicable` / `not_checked`. |

## Promotion Evidence

The original UltraPlan still had `profile: "generic"`:

```yaml
goal: "ちょっとしたメモアプリを作って。ブラウザで使えるようにしてください。フレームワークはNode.jsエコシステムの標準的なフルスタック構成を選んでください。3011ポートで起動可能にしてください。"
profile: "generic"
phases:
  - id: "setup-framework"
  - id: "implement-memo-logic"
  - id: "implement-memo-ui"
  - id: "verify-application"
```

The scaffold produced a real Next.js shape:

```json
{
  "scripts": {
    "build": "next build",
    "dev": "next dev -p 3011",
    "start": "next start -p 3011"
  },
  "dependencies": {
    "next": "14.2.5",
    "react": "^18",
    "react-dom": "^18"
  }
}
```

The promotion event was present:

```json
{
  "event": "profile_reinferred",
  "from_profile": "generic",
  "to_profile": "nextjs",
  "from": "workspace",
  "at_phase": 1,
  "phase_id": "setup-framework",
  "delta_requirements": [
    "nextjs_route_evidence",
    "build_command_or_dependency_missing_boundary"
  ]
}
```

This is the core evidence missing from the previous G3 run.

## Generated Artifacts

Created project files included:

```text
package.json
postcss.config.js
tailwind.config.js
tsconfig.json
src/app/layout.tsx
src/app/page.tsx
src/app/globals.css
src/app/global.d.ts
```

The partial `src/app/page.tsx` contained:

```tsx
"use client";

import { useState } from "react";

export default function Home() {
  const [memos, setMemos] = useState<Memo[]>([]);
  const [text, setText] = useState("");

  const addMemo = () => {
    if (!text.trim()) return;
    const newMemo: Memo = {
      id: Date.now().toString(),
      text,
      createdAt: Date.now(),
    };
    setMemos([...memos, newMemo]);
    setText("");
  };

  const deleteMemo = (id: string) => {
    setMemos(memos.filter((m) => m.id !== id));
  };
}
```

This is an incomplete app: it has add/delete state in React, but no `localStorage` persistence yet. The run aborted before UI/final verification could complete.

## Abort / Panic Evidence

The process exited with code `101` and printed:

```text
thread 'main' (111645636) panicked at src/minimal_loop/evidence.rs:2988:33:
byte index 1222 is not a char boundary; it is inside '除' (bytes 1220..1223)
```

The corresponding code is:

```rust
for (index, _) in lower.match_indices(needle) {
    let end = lower.len().min(index + 500);
    segments.push(&lower[index..end]);
}
```

`index + 500` can land in the middle of a multibyte UTF-8 character. In this run it landed inside the Japanese character `除`, causing a panic while extracting evidence segments from generated source text.

## Summary Evidence

The run summary recorded:

```text
Status: aborted
Completion status: incomplete
Command status: aborted
Task status: aborted
Runtime acceptance: not_checked
Final acceptance: not_checked
Profile: generic
Assurance: reduced (generic profile — no capability contract, no behavioral verification)
Stop reason: command aborted before completion
Completed phases:
- setup-framework (completed)
Failed phases:
- implement-memo-logic (aborted)
Pending phases:
- phase 3 (pending)
- phase 4 (pending)
```

The summary still reports `Profile: generic` and `Assurance: reduced` even though events show `profile_reinferred` to `nextjs`. That is a reporting/state propagation gap after promotion and before panic.

## Issues Found

1. UTF-8 unsafe slicing in `src/minimal_loop/evidence.rs`.

   `lower[index..end]` assumes `end` is a char boundary. Generated Japanese UI text made this false. This is a general Unicode bug, not a scenario-specific issue.

2. Panic prevents honest terminal diagnostics.

   The summary says `tui_command_aborted`, but there is no structured `panic` failure kind in events. The real failure was a Rust panic in evidence extraction, not user abort or model/runtime task failure.

3. Promoted profile is not reflected in final aborted summary.

   Events show promotion to `nextjs`, but summary/TUI stop projection still says `Profile: generic` and reduced assurance. For G3/debugging, the effective promoted profile should be visible even on abort.

4. Final acceptance was not reached.

   This run proves live promotion, but not the full G3 end-to-end path. Browser readiness, interaction evidence, and release gate were not exercised.

5. Generated app was still incomplete at abort.

   `src/app/page.tsx` had React state and add/delete UI, but no localStorage persistence and no final browser evidence.

## Verdict

G3 revalidation partially succeeded:

- The live generic -> nextjs promotion path is now proven with event evidence.
- The prior ambiguity around Vite/static-tier fallback was addressed for this run.

G3 is not fully accepted:

- The command did not complete.
- Final acceptance did not run.
- The process panicked before browser/interaction evidence could be collected.

## Follow-Up Fix Prompt

```text
G3 revalidation in /Users/maenokota/share/work/localwork/commandagent_mvp/01/test0704-403044454243464748481495051535455_000 proved generic -> nextjs promotion but failed due to a Rust panic.

Please fix:
1. In mvp/anvilminimal/src/minimal_loop/evidence.rs, make evidence segment extraction UTF-8 safe. Do not slice strings with byte offsets that may not be char boundaries. Use char_indices, floor to a valid char boundary, or collect a bounded char sequence.
2. Add a regression test with Japanese text around an event/input handler token so segment extraction cannot panic on multibyte characters.
3. Ensure promoted effective profile is reflected in aborted/failed summary and tui_command_stop projection. Events currently show `profile_reinferred` to nextjs, but summary still says Profile: generic / Assurance: reduced.
4. Add structured panic/abort diagnostics if a command aborts unexpectedly, so UAT can distinguish a real Rust panic from user abort.
5. Re-run the same G3 prompt on a non-3000 port and require: generic start, generic_contract_bound, profile_reinferred to nextjs, promoted nextjs requirements in phase 2, no panic, and final acceptance/browser/interaction evidence or a real capability failure.
```
