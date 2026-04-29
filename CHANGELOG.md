# Changelog

## [Unreleased]

## [0.3.0] - 2026-04-29

Epic B: Verification & Temporary Testing — turn 単位で検証可能な進捗信号を凝集する `AnvilScore`、test verifier が無い repo 向けの Tester Skill、生成テストを repo に汚染させない Temporary Test Workspace、`auto_test` ↔ AnvilScore 接続、`/tests` REPL コマンド、ランタイム sandbox の強化を一括投入した。

### Added

- `AnvilScore` 値オブジェクトと `compute_anvil_score` 純関数 (#456)。turn 単位の build / test / repo-diff / unsafe-block / no-progress 信号を 12 フィールド（5 lifecycle group）に集約する deterministic snapshot。`SessionSnapshot.last_anvil_score` に lossy persist し、`ReminderInputs.anvil_score` 経由で Reminder Sidecar に `PreviousTurn` / `CurrentTurn` enum で渡す。`format_for_prompt(MAX_RENDERED_CHARS=512)` 単一情報源 renderer、`MAX_ANVIL_SCORE_RAW_BYTES=64KiB` cap、`agent.anvil_score.computed` jsonl event。
- `src/util/file_classify.rs` 新規 (#456)。`is_implementation_file` / `is_test_file` / `is_setup_file` の単一情報源を `agent::orchestration` と `session::anvil_score` の双方が参照し、layer 逆転を起こさず DRY を維持する。
- `auto_test` を `AnvilScore` / `FeedbackFrame` に接続 (#457)。`build_anvil_test_summary(plan, &result) -> AnvilTestSummary` アダプタを `agent/loop_run/turn.rs` に追加し、`AutoTestKind × passed` 4 アーム写像で `build_passed` / `tests_passed` / `compile_error_count` / `test_failure_count` を populate する。`count_compile_errors` / `count_test_failures` を `auto_test.rs` に plan 非依存の純関数として追加し、マーカー pattern を module-private const に SSOT 化。Tester / NoVerifier / Skip / TransportError は従来どおり `None` を渡す。
- Temporary Test Workspace (#458)。`state_root/sessions/<id>/tmp-tests/{files,metadata}/` 配下で `TmpTest` / `TmpTestStatus` / `TmpTestRunResult` の lifecycle (`create_generated_test` / `discard_tmp_test` / `promote_tmp_test` / `list_tmp_tests`) を管理。`derive_test_id` (sha256 + created_at) + `validate_test_id` allowlist、`MAX_TMP_TEST_METADATA_BYTES=64KiB` / `MAX_TMP_TESTS_PER_SESSION=4096` cap、`OpenOptions::create_new(true)` で TOCTOU 回避、symlink dst は `--force` でも拒否。`ToolContext.tmp_tests_root` と `resolve_tmp_tests_path` で Read/Write/Edit の `tmp-tests/<rel>` prefix を session-scoped root に routing し、Plan mode 判定より前で評価する。`anvil sessions tmp-tests {promote,discard,list}` CLI を追加。
- Tester Skill v1 (#459)。`AutoTestRunner::detect == None` かつ Rust / Node / Python のいずれかが検出された Act-mode ターンで、main model を 1 回だけ同期呼び出して smoke test を生成し、`state_root/sessions/<id>/tmp-tests/files/` に保存したうえで固定テンプレートの Bash（Rust: `cargo test --manifest-path`、Node: `node --check`、Python: `python3 -m py_compile`）を 30 秒の明示 timeout 付きで実行（per-turn cap = 1、`tools=None`、JSON-only、`<think>` strip + first JSON object 抽出、`tool_calls` 非空は abort、`ANVIL_NO_TESTER` / Plan mode で disable 可）。Rust 経路は `state_root/sessions/<id>/tester-runs/<run_id>/` に transient harness（path dependency = workspace package）を書き出し、`TESTER_RUNS_KEEP_LATEST=8` で best-effort cleanup する。承認モードは runtime 派生（`--yes`→Auto / TTY→Interactive / それ以外→Forbidden）。`derive_run_id` + `validate_run_id` (`run_[a-z0-9_-]{16,64}`) と `BashEnvPolicy::TesterSanitized` で shell injection / secret leakage を防御。結果は `FeedbackFrame` として記録され、Reminder Sidecar 経路で `Precaution` 生成に繋がる。`agent.tester.{llm_call_started,llm_call_completed,llm_call_failed,completed,failed,skipped}` ログ出力。`ToolContext.tester_active` で Tester 起動中の Edit/Write を `tmp-tests/` 配下に強制。
- `/tests promote|discard|list` REPL slash コマンド (#460)。既存の `tmp_tests::{promote_tmp_test,discard_tmp_test,list_tmp_tests}` lifecycle (#458) を REPL から直接呼び出す（`ToolRegistry::Write` 経由ではないため `tester_active=true` の path confinement は迂回しない）。Plan モードでは `/tests promote` を拒否し、`list` / `discard` のみ許可。`auto_approve` は `config.yes_mode` に追従。
- Epic A / Epic B の orchestration plans / summaries を `workspace/orchestration/runs/2026-04-27/` および `workspace/orchestration/runs/2026-04-28/` 配下に追加。

### Changed

- ランタイム sandbox policy の集約 (#461 Phase A)。`bash::check_blocked_command` を destructive 判定の **Single Source of Truth** に格上げし、新規 `BlockCategory::{DangerousVerb, KillSignalOne, DeviceRedirect, ForkBomb}` を per-category predicate で実装。新規パターン: shutdown / reboot / halt / iptables / ufw / route / `kill -1` / `>` `>>` `>|` `1>` `2>` `1>>` `2>>` to `/dev/sd[a-z]` / fork bomb (whitespace-stripped)。`render_block_error` で Err 文字列を一元生成。`registry.rs` に `preflight_bash_command` を新設し、`execute` / `execute_bash_with_outcome` の双方で `maybe_confirm` の **前** に preflight する。`apply_unix_pgroup` / `terminate_child` / `enforce_offline_policy` を `pub(crate)` 昇格。`logging::mask_payload_inplace` で `log_llm_event` の payload tree を recursive mask（secret-like key 配下は object/array/number/bool/null も `Value::String("***")` に置換）。`build_feedback_for_unsafe_block_reason` で primary_error には rendered block reason のみを格納し、Reminder Sidecar prompt に raw blocked-command が流れない（DR4-004）。

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
