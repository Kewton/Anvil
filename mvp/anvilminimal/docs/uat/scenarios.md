# Multi-Scenario UAT Suite

This suite measures tuning against five small task distributions, not a single
Space Invaders prompt. Run every scenario with the documented command shape and
compare the final acceptance state across the distribution. `AMBIGUOUS` is the
no-profile scenario; because successful promotion leads to web release gates on
this host, it requires the interaction probe just like the Next.js scenarios.

This suite is part of the M6 generality declaration in
[../generality.md](../generality.md). It is a mandatory regression suite for
any probe, evidence, or profile change.

## Command Shape

Start `anvilminimal` with the normal UAT model/provider options, then enter one
scenario command in the TUI. Use the exact scenario command; do not add
`--profile nextjs` to `AMBIGUOUS`.

```text
anvilminimal --yes --context-budget 65536 \
  --model qwen3.6:27b-coding-nvfp4 \
  --planner-model gemini-3.5-flash \
  --planner-provider gemini \
  --provider ollama

/ultra-plan-run --profile nextjs <SCENARIO_PROMPT>
```

Use a fresh empty workdir per scenario. Do not run the three explicit Next.js
scenarios or the `AMBIGUOUS` scenario in parallel because web-gated runs use
port 3011. The CLI scenario uses `--profile python-cli`, does not bind a port,
and must not be treated as a web or browser-interaction run.

Local provider timeout calibration: `ollama`/local provider runs default each
provider turn to 600 seconds; remote-only provider runs default to 180 seconds.
Use `--chat-timeout-secs <seconds>` only when intentionally overriding that
calibrated default. When planner and executor are co-resident on the same local
Ollama host, run scenarios serially and avoid concurrent planner/executor UAT
loads; timeout telemetry records whether the local default or an override was
used.

Local-tier expectation setting: the final local compatibility measurement
`test0704-999-Q1-62646566676869707172_001` used a co-resident
`qwen3.6:27b-coding-nvfp4` planner and `ornith:35b` executor on the local
Ollama host. It reached 8/8 honest terminal states and 2/8 full passes: CLI
1/2, web 1/6, with CONTENT b as the complete web full pass including browser
readiness, interaction evidence, persistence, release-gate pass, and full
earned assurance. Treat this pair as moderately viable for CLI and below the
recommended tier for reliable web delivery. For release-quality web UAT, keep
using the recommended `gemini-3.5-flash`-class tier unless the goal is a local
compatibility measurement.

Single-model local GAME expectation: the instruction 81-87 GAME quality track
used `qwen3.6:27b-coding-nvfp4` as a single local model configuration across a
six-run series. The frontier moved from phase 0, to phase 1, to final-gate
failure, then to `test0707_009` full success with browser readiness,
interaction evidence, score and enemy-state mutation, and an in-play
recovery/restart transition. Treat this as a golden local single-model GAME
reference, distinct from the broader 27b/35b mixed-pair distribution above. It
does not make long local turns suspicious by itself: multi-minute provider
turns and long wall-clock GAME runs are normal on local models when they stay
inside the configured 600-second provider-turn bound and produce progress
telemetry. Budget or timeout failures must still terminate with concrete
bounded handoff reasons.

New-model acceptance order: before any scenario UAT for a new model family or
version, run the model behavior probe documented in
[../model-probe.md](../model-probe.md), review its card, then run two smoke
checks (one CLI and one TOOL), then the full scenario round with landing
criteria committed before measurement. The tier-table entry cites the probe
profile. Re-run the probe when the model version or digest changes; for
cloud-hosted models, re-run it before every measurement campaign because model
identity is not pinned.

## Preflight Runbook

1. Version check: `anvilminimal --version` must exactly match the intended
   commit or build identifier. Record the full output in the UAT report. A
   version mismatch fails preflight.
2. Playwright check for web-gated scenarios only: verify that the browser probe
   can run before starting each Next.js or `AMBIGUOUS` run. Use the same
   Node/Playwright environment as the release gate; if it is unavailable,
   record the probe-unavailable reason and do not count the run as a behavioral
   pass. This check is not required for `CLI (python-cli profile)`.
3. Port check for web-gated scenarios only: never use port 3000; it is
   permanently occupied on the UAT host. Confirm port 3011 has no leftover
   listener before each Next.js or `AMBIGUOUS` run. With the default-port
   policy, no cleanup check for 3000 is needed. If a previous 3011 dev server
   is still alive, stop it and record the cleanup. The CLI scenario has no
   port. When manually launching a generated Next.js dev server for inspection,
   use `env -u NODE_ENV npm run dev` so manual checks match the normalized
   runtime launcher environment.
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

## Presentation Acceptance

Every presentation-layer change must be checked by a human with:

```sh
anvilminimal --ux-demo
```

The reviewer requires explicit confirmation that the demo was run and that the
banner, plan card, phase header, activity narration, live footer interruption
hint, and terminal summary card were visible and ordered correctly. This demo is
offline and does not replace the scenario suite; it is the permanent acceptance
vehicle for TUI/scrollback/footer presentation changes.

For footer-specific changes, the reviewer also checks `anvilminimal --ux-demo`
and one real local run for zero blinking, readable long-turn breadcrumbs, and a
stable footer. If a terminal shows cursor-region artifacts, rerun with
`--footer off`; the fixed footer is disabled and scrollback breadcrumbs remain.

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

### AMBIGUOUS (no profile, no stack tokens)

Prompt:

```text
/ultra-plan-run ちょっとしたメモアプリを作って。ブラウザで使える
ようにしてください。
```

Notes:

- No `--profile` flag.
- The prompt intentionally contains app-intent tokens but no profile or stack
  tokens. Do not "fix" the scenario by adding Next.js, React, Vite, web app, or
  another stack token.
- Preflight must check port 3011 only. Port 3000 is host-occupied and must not
  be used.

Expected trajectory:

1. Generic start: `tui_command_start` records profile `generic` with no
   profile inference.
2. Generic contract bound: `completion_contract_bound` and
   `generic_contract_bound` record `generic_interactive_contract`,
   `user_input_handler_evidence`, `stateful_update_evidence`, and
   `visible_interactive_surface_evidence`.
3. Scaffold manifest: the planner creates a project manifest in the workspace.
4. Profile reinferred promotion: if the manifest is known, `profile_reinferred`
   promotes `generic` to the known profile.
5. Full-assurance terminal: the promoted profile reaches
   `ultra_final_acceptance` / `ultra_plan_complete` with full assurance and
   behavioral verification, including browser readiness and interaction
   evidence for web profiles.

Events runbook:

```sh
events=.anvil/runs/<run-id>/events.jsonl
rg '"event":"tui_command_start"|"event":"generic_contract_bound"|"event":"profile_reinferred"' "$events"
rg '"event":"dev_server_lifecycle"|"event":"browser_probe"|"event":"ultra_final_acceptance"|"event":"ultra_plan_complete"' "$events"
rg '"assurance_level":"full"|"final_acceptance_status":"full_success"|"release_gate_status":"pass"|"interaction_evidence_status":"passed"' "$events"
```

Closure checklist:

- Quote the actual `profile_reinferred` event line for any promoted run. If no
  promotion happens, quote the absence check and the static-tier terminal line.
- Quote the actual line carrying `contract_origin`. Promoted interactive runs
  must show `contract_origin=promoted_union`.
- Quote dependency reconciliation lines when dependency setup is triggered,
  including the event that shows the dependency need and the event that shows
  setup authority / install status.
- Quote the actual browser readiness and interaction execution values:
  `browser_readiness_status` or `browser_readiness_execution_status`, and
  `interaction_evidence_status` or `interaction_evidence_execution_status`.
  `not_applicable` is disqualifying for promoted interactive web runs.
- Quote the earned-assurance line from `ultra_final_acceptance` or
  `ultra_plan_complete`, including `assurance_level`,
  `final_acceptance_status`, `release_gate_status`, and the gate-status fields
  used to earn it.

Reviewer rule: assurance labels are never accepted without their gate-status
fields.

Honest fallback:

- If the planner scaffolds an unknown stack, static-tier termination is correct
  behavior, not a UAT failure.
- Record the absence of `profile_reinferred`, the static assurance level, and
  the scaffolded manifest/stack in the UAT report.
