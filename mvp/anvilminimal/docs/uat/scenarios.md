# Multi-Scenario UAT Suite

This suite measures tuning against a small task distribution, not a single
Space Invaders prompt. Run every scenario with the same command shape and
compare the final acceptance state across the distribution.

This suite is part of the M6 generality declaration in
[../generality.md](../generality.md). It is a mandatory regression suite for
any probe, evidence, or profile change.

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

Use a fresh empty workdir per scenario. Do not run the three Next.js scenarios
in parallel because all of them require port 3011. The CLI scenario uses
`--profile python-cli`, does not bind a port, and must not be treated as a web
or browser-interaction run.

## Preflight Runbook

1. Version check: `anvilminimal --version` must exactly match the intended
   commit or build identifier. Record the full output in the UAT report. A
   version mismatch fails preflight.
2. Playwright check for Next.js scenarios only: verify that the browser probe
   can run before starting the scenario. Use the same Node/Playwright
   environment as the release gate; if it is unavailable, record the
   probe-unavailable reason and do not count the run as a behavioral pass. This
   check is not required for `CLI (python-cli profile)`.
3. Port check for Next.js scenarios only: confirm port 3011 has no leftover
   listener before each run. If a previous dev server is still alive, stop it
   and record the cleanup. The CLI scenario has no port.
4. Evidence capture: attach `.anvil/runs/<run-id>/events.jsonl`,
   `.anvil/runs/<run-id>/summary.md`, any referenced
   `browser_readiness_evidence_path`, any referenced `interaction_evidence_path`,
   any referenced `profile_behavior_probe_evidence_path`,
   `.anvil/evidence/python-cli-fixtures/*.csv` for the CLI scenario,
   `.anvil/repairs/*.md`, and `.anvil/plans/recovery-ultra-plan-*.yaml` when
   recovery is offered.
5. Corpus duty: every UAT anomaly, including false positives, false negatives,
   probe drift, profile drift, and terminal-state ambiguity, must become a
   corpus case or be explicitly recorded as out of scope before changing probe,
   evidence, or profile logic.

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

### CLI (python-cli profile)

Prompt:

```text
/ultra-plan-run --profile python-cli CSVファイルを読み込み、数値列の
合計・平均・最大・最小を集計して表形式で標準出力するCLIツールを
Pythonで開発してください。
```

Expected capability profile:

- CLI-shaped implementation only: argument/input handling, deterministic
  stdout output, and missing/invalid file error handling.
- Python package scaffold with `pyproject.toml` and `src/<package>/main.py`.
- No web, browser, or interaction-probe evidence. No port. No Playwright
  prerequisite.
- `/setup-interaction-probe` is not required for this scenario.

Final acceptance requires:

- The dependency lifecycle completes, including venv creation and pip
  installation.
- The Python compile oracle passes with `python -m compileall -q src`.
- The profile behavior probe runs the CLI against generated fixture CSV input,
  passes the CSV path as an argument, observes exit 0, observes non-empty stdout,
  finds computed aggregate values in stdout, and confirms stdout changes when
  the input CSV changes.
- Honest terminal state as usual: a full success must have matching
  `profile_behavior_probe_status: pass`, `final_acceptance_status:
  full_success`, and `ultra_plan_complete`; partial or incomplete runs must
  record the missing layer and recovery handoff.

Partial means:

- Dependency setup is blocked or unavailable, so venv/pip lifecycle completion
  is unverified.
- Compile passes but the behavior probe is unavailable or fails, so CSV argv
  handling, aggregate stdout, or input variation remains unverified.
- The valid CSV behavior passes but missing/invalid file handling is only
  supported by static source evidence and has not been separately exercised.
- The run reports partial/incomplete terminal state with concrete recovery
  targets instead of claiming full success.

Evidence to attach:

- The head of `.anvil/runs/<run-id>/summary.md`.
- `.anvil/runs/<run-id>/events.jsonl`.
- `.anvil/evidence/python-cli-behavior.json` and any behavior-probe evidence
  file(s) it references.
- Generated sources, especially `pyproject.toml` and `src/<package>/main.py`.
- The generated fixture CSV file(s) used by the behavior probe.

M4 exit criteria:

The run must traverse the shared runner lifecycle without touching
nextjs-specific code paths; classification/terminal semantics identical in
kind.
