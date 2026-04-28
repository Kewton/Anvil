# Changelog

## [Unreleased]

### Added

- Tester Skill v1 (#459)。`AutoTestRunner::detect == None` かつ Rust / Node / Python のいずれかが検出された Act-mode ターンで、main model を 1 回だけ同期呼び出して smoke test を生成し、`state_root/sessions/<id>/tmp-tests/files/` に保存したうえで固定テンプレートの Bash（Rust: `cargo test --manifest-path`、Node: `node --check`、Python: `python3 -m py_compile`）を 30 秒の明示 timeout 付きで実行（per-turn cap = 1、`tools=None`、JSON-only、`<think>` strip + first JSON object 抽出、`tool_calls` 非空は abort、`ANVIL_NO_TESTER` / Plan mode で disable 可）。Rust 経路は `state_root/sessions/<id>/tester-runs/<run_id>/` に transient harness を書き出し、終了後に best-effort cleanup する。結果は `FeedbackFrame` として `WorkingMemory.last_feedback` に記録され、Reminder Sidecar 経路で `Precaution` 生成に繋がる。`agent.tester.{llm_call_started,llm_call_completed,llm_call_failed,completed,failed,skipped}` ログ出力。影響モジュール: `src/agent/loop_run/tester.rs` (新規)、`src/tools/registry.rs` (`ToolContext.tester_active` フィールド追加 + Edit/Write の `tmp-tests/` 強制)、`src/agent/loop_run/auto_test.rs` (`has_cargo_manifest` / `package_json_has_test_script` / `has_python_surface` / `first_python_script` / `shell_quote` / `MAX_OUTPUT_BYTES` の `pub(super)` 化)、`src/tools/bash.rs` (`run_with_outcome` に `explicit_timeout: Option<Duration>` 追加)、`src/agent/loop_run/turn.rs` (`Agent.tester_called_this_turn` + `try_invoke_tester` 分岐挿入)。

## [0.2.0] - 2026-04-28

Epic A: Dynamic Precaution Runtime — runtime feedback の正規化、Active Precaution の永続化、Reminder Sidecar による自動生成、Act-mode prompt への注入、`/precautions` REPL コマンド、ランタイム回復経路との接続を一通り入れた。

### Added

- `FeedbackFrame` / `FeedbackKind` runtime feedback 値型 (#450)。Bash / auto_test / tool parser failure / unsafe command block / no-progress / edit failure を共通 shape に正規化し、`SessionSnapshot.last_feedback` に保存。secret mask（token prefix / kv secret / URL credential）と head+tail 8 KiB excerpt cap、PathBuf workspace 相対化を含む。
- `WorkingMemory.active_precautions` と `Precaution` 型 (#451)。SHA-256 deterministic id、FIFO eviction、bounded caps（active 16 / total 64 / text 240）、untrusted-load sanitizer、`format_for_prompt` での Active Precautions セクション出力。
- Reminder Sidecar (#452)。`FeedbackFrame` の失敗系 kind を sidecar Ollama に渡して `Precaution` を自動生成（per-turn cap = 1、`tools=None`、JSON-only、`<think>` strip + first JSON object 抽出、`ANVIL_NO_REMINDER` で disable 可）。`agent.reminder.{completed,failed,skipped}` ログ出力。
- Active Precautions の Act-mode prompt 注入 (#453)。`select_precautions_for_prompt` で severity sort、touched/suspected ファイル相対の relevance scoring、token budget cap（`MAX_ACTIVE_PRECAUTIONS_PROMPT=8` / `MAX_ACTIVE_PRECAUTIONS_CHARS=1024`）、Plan-mode 抑止を適用。
- `/precautions [add <text>|retire <id>|clear]` REPL コマンド (#454)。Active precautions を一覧 / 追加 / retire / clear。`PrecautionSource::as_label` で表示と `compute_precaution_id` の source label を共有。
- Runtime recovery と precaution の接続 (#455)。`FeedbackKind::NoToolCall` (D1) と deterministic content fallback (D2) を追加し、no-tool-call exhaustion / 5 つの in-loop fallback success / 2 つの timeout wrapper / 3 つの post-loop site で feedback を記録。`eligible_feedback_recorded_this_turn` flag で first-eligible-failure-wins を保証。

### Fixed

- `history_roundtrip_via_append_and_load` ほか rustyline history テストの flaky を解消 (#443)。rustyline 14 の `History::save()` が umask を非アトミックに反転する race を、test ファイル全体を process-wide Mutex で直列化することで回避。
- `non_utf8_stdout_does_not_panic_or_error` テストを CI の dash で安定化。`printf '\xff\xfe\xfd'` を POSIX 互換の `printf '\377\376\375'` に置換し、escape が展開されなかった場合の guard assertion を追加。

## [0.1.0] - 2026-04-16

- Rebuilt Anvil from scratch as an Ollama-first local coding agent.
- Removed the old generic agent architecture and its large state-machine surface.
- Added a minimal tool-first loop with `Bash`, `Read`, `Write`, `Edit`, `Glob`, and `Grep`.
- Added session persistence, deterministic compaction, Plan / Act mode, and git checkpoint / rollback.
- Added `<think>` stripping and XML tool-call fallback for local models.
- Preserved the existing CI and GitHub Releases process with the `anvil` binary name unchanged.
