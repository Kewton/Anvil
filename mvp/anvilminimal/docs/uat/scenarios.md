# Multi-Scenario UAT Suite

This suite measures tuning against a small task distribution, not a single
Space Invaders prompt. Run every scenario with the same command shape and
compare the final acceptance state across the distribution.

## Command Shape

Start `anvilminimal` with the normal UAT model/provider options, then enter one
scenario command in the TUI:

```text
anvilminimal --yes --context-budget 65536 \
  --model qwen3.6:27b-coding-nvfp4 \
  --planner-model gemini-3.5-flash \
  --planner-provider gemini \
  --provider ollama

/ultra-plan-run --profile nextjs <SCENARIO_PROMPT>
```

Use a fresh empty workdir per scenario. Do not run the three scenarios in
parallel because all of them require port 3011.

## Preflight Runbook

1. Version check: `anvilminimal --version` must show the intended commit or a
   build identifier known to contain that commit. Record the full output in the
   UAT report.
2. Playwright check: verify that the browser probe can run before starting the
   scenario. Use the same Node/Playwright environment as the release gate; if it
   is unavailable, record the probe-unavailable reason and do not count the run
   as a behavioral pass.
3. Port check: confirm port 3011 has no leftover listener before each run. If a
   previous dev server is still alive, stop it and record the cleanup.
4. Evidence capture: attach `.anvil/runs/<run-id>/events.jsonl`,
   `.anvil/runs/<run-id>/summary.md`, any referenced
   `browser_readiness_evidence_path`, any referenced `interaction_evidence_path`,
   `.anvil/repairs/*.md`, and `.anvil/plans/recovery-ultra-plan-*.yaml` when
   recovery is offered.

## Scenarios

### GAME

Prompt:

```text
あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとして開発してください。
```

Expected capability profile:

- Next.js route-bound implementation on port 3011.
- Generic interactive state: visible surface, user input, state mutation.
- Game-specific obligations: start/restart flow, player control,
  adversary/challenge, progression/score, failure/collision rule.
- Browser interaction probe should observe a transition into gameplay and a
  post-start input-driven state change.

Final acceptance requires:

- `src/app/page.tsx` is route-bound and not a static title shell.
- `package.json` has `next dev -p 3011` or `next dev --port 3011`.
- Build succeeds, or dependency setup is explicitly the only blocker.
- Runtime evidence shows playable interaction: start/restart, player control,
  adversary/challenge, score/progression, and failure/collision behavior.

Partial means:

- A buildable Next.js shell exists but gameplay is static or missing one or more
  game-specific capabilities.
- Browser readiness passes but interaction evidence is missing, unavailable, or
  fails either transition or post-start input state change.
- Recovery prompt/YAML is produced with concrete missing capability targets.

### TOOL

Prompt:

```text
ローカルストレージに保存されるTodoアプリ(追加・完了・削除・フィルタ)をNext.jsアプリとして3011ポートで起動可能に開発してください。
```

Expected capability profile:

- Next.js route-bound implementation on port 3011.
- Generic interactive state: visible surface, user input, state mutation,
  visible state change.
- Persistence obligation backed by route-bound storage code such as
  `localStorage`.
- No game-specific adversary/challenge or failure/collision capability.

Final acceptance requires:

- Todo add, complete, delete, and filter workflows are implemented in the UI.
- Todo state persists across reload through local storage or an equivalent
  local browser storage API.
- Browser interaction evidence may pass without a start transition when no
  start-like control exists and direct input changes visible state.

Partial means:

- The app builds and renders but lacks one Todo workflow.
- Todo interactions work only in memory and do not persist.
- Static source suggests a Todo UI, but behavioral evidence does not show input
  state change.

### CONTENT

Prompt:

```text
Markdownをリアルタイムプレビューできるノートアプリ(編集・保存・一覧)をNext.jsアプリとして3011ポートで開発してください。
```

Expected capability profile:

- Next.js route-bound implementation on port 3011.
- Generic interactive state: editor input, live preview state, visible state
  change, list navigation/selection.
- Persistence obligation for saved notes.
- No game-specific adversary/challenge or failure/collision capability.

Final acceptance requires:

- Editing Markdown updates the preview without a manual rebuild or page reload.
- Notes can be saved and listed, and a saved note can be reopened for editing.
- Persistence survives reload through local browser storage or an equivalent
  local-first mechanism.
- Browser interaction evidence may pass without a start transition when direct
  editor input changes visible state.

Partial means:

- The app builds and renders but preview is static or not tied to editor input.
- Save/list exists without persistence across reload.
- Static source contains Markdown or notes labels but lacks route-bound
  stateful input handling.

