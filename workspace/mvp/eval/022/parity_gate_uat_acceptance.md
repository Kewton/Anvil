# Parity Gate UAT-equivalent Acceptance

作成日: 2026-06-29

## 1. 目的

manual UAT で見つかる「build は通るが成果物が要求を満たしていない」false positive を gate で防ぐ。特に Next.js interactive game では、`npm run build` 成功だけを acceptance success にしない。

## 2. Acceptance Layers

| Layer | Required evidence |
| --- | --- |
| artifact | expected files exist |
| build | deterministic build/test/compile succeeds |
| route/runtime | app starts and expected route responds |
| capability | task-required behavior exists in code/runtime evidence |
| interaction | user input changes state where relevant |
| recovery | failure produces event and suggested next action |

Full pass は required layers をすべて満たす場合のみ。

## 3. Next.js Interactive Game Contract

Generic interactive game capability evidence:

| Capability | Evidence examples |
| --- | --- |
| `stateful_interaction` | React state/reducer/canvas state/game loop |
| `player_control` | keyboard/pointer event handlers that affect player state |
| `adversary_or_challenge` | enemies/obstacles/timer/challenge state |
| `projectile_or_collision` | projectile, collision detection, hit/miss logic |
| `score_or_progression` | score, lives, level, progression |
| `start_restart_failure_flow` | start, game over, restart, win/loss |

Required files for Next.js App Router:

- `package.json`
- `src/app/page.tsx`
- `src/app/layout.tsx`
- stylesheet or component files when referenced

Runtime checks:

- dependency setup succeeds under allowed authority.
- `npm run build` succeeds.
- dev server can bind configured port, or sandbox limitation is explicitly recorded.
- HTTP 200 for `/`.
- release gate full pass requires browser readiness/interaction evidence.

## 4. Negative Cases

These must be acceptance failures:

- static title-only page
- CSS/theme only with no interactive behavior
- docs-only output for app/game task
- package/build setup only
- scaffolded framework app with no task-specific implementation
- page exists but no capability evidence
- route only renders not-found

## 5. Browser Evidence Policy

| Environment | Max status |
| --- | --- |
| Browser automation available and interaction evidence captured | pass |
| Browser unavailable but source semantic + route/build evidence present | partial |
| Build only | fail for interactive app/game |
| Source semantic evidence absent | fail |

`release` gate で browser evidence がない場合は full pass にしない。

## 6. Non-web Profiles

| Profile | Acceptance examples |
| --- | --- |
| CLI | command runs, expected stdout/stderr, exit code, file outputs |
| docs | required sections, examples, consistency, no placeholder |
| library | unit tests compile/run, public API exists, sample use passes |
| data | input preserved, deterministic transform output, schema validation |
| rust | `cargo check/test`, expected crate/module/bin exists |
| python | `py_compile`/`pytest`, expected module/function exists |

## 7. UAT Trace Requirements

Manual/TUI run must leave:

- `.anvil/runs/<run-id>/events.jsonl` or equivalent event log
- command/prompt/profile/provider metadata with secrets redacted
- plan yaml path when generated
- repair prompt path when handoff occurs
- acceptance report path
- failure kind and lifecycle stage on failure

## 8. Acceptance For Phase 7

- build-only success cannot satisfy interactive app/game.
- static title-only app is explicitly failing fixture.
- browser-unavailable state is partial, not pass.
- output quality is judged by generic capability contract, not Space Invaders-specific text.
