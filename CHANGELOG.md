# Changelog

## [Unreleased]

## [0.6.0] - 2026-04-30

Epic F: Structured Eval & Dataset Export — turn 単位の評価ログを構造化 JSONL で永続化し、ローカルモデルの A/B 評価ハーネスと fine-tuning 用データセットエクスポートを提供。

### Added

- Structured evaluation log per turn (`src/session/eval_log.rs`, #471)。Act mode の post-loop hook で `EvalRecord` (schema_version / session_id / ts_ms / task / model / mode / tool_protocol / tool_calls / feedback_frame / active_precautions / anvil_score / changed_file_classes / verify_commands / case_retrieval_result / final_outcome) を `state_root/sessions/<id>/logs/eval.jsonl` に 1 record/turn で追記。`OnceLock<Mutex<File>>` の append-only logger + permissions 0o600。セキュリティパイプライン: `to_value → mask_payload_inplace → to_string → scrub_absolute_paths (ANVIL_EVAL_SCRUB_PATHS opt-in) → size check (MAX_EVAL_LOG_RECORD_BYTES=64KiB) → append`。E2E smoke tests 12 件 (`tests/eval_log_smoke.rs` R1-R12、Ollama-free)。
- Local model A/B evaluation harness (#472)。セッション間でのモデル比較評価を自動化するハーネス。eval.jsonl を入力に、指定モデル対の結果を集計・比較する。
- Fine-tuning dataset export (`src/session/export.rs`, #473)。`llm-io.jsonl` から `agent.reminder.completed` と `agent.anvil_score.computed` を `(session_id, turn_index)` で join し、fine-tuning 用 training JSONL を出力。`ExportConfig` / `ExportScope` / `ExportFilter` / `ExportOutput` / `ExportResult` 型。`anonymize_paths` (unix only、`OnceLock<Regex>`、work_root prefix→`<workdir>`、home dir→`~/`) → `mask_secrets` 順でスクラブ。`MAX_EXPORT_JSONL_BYTES=64MiB` 超 session は parse 前 skip。`--output FILE` は symlink 拒否 + Unix 0600 新規作成。`run_export_with_io<FR,FW,FS>` closure DI seam で E2E 12 ケースを Ollama 不要で検証 (`tests/dataset_export_smoke.rs`)。

### Changed

- `agent.reminder.completed` payload を 17 key に拡張 (`turn_index` / `task_at_call_time` / `precautions_at_call_time` / `feedback_excerpt` / `added_precautions_text` を追加) — fine-tuning dataset export との join key として `turn_index` を追加 (#473)。
- `agent.anvil_score.computed` event payload に `turn_index` を追加 (dataset export の join key, #473)。

## [0.5.0] - 2026-04-30

Epic E: Repo Graph & Domain Context — ローカルリポジトリの import 依存グラフを静的解析で構築し、graph-aware な repo context 選択でコンテキスト精度を向上。path-aware な ANVIL.md 指示ファイルをワークスペースから読み込み可能にした。

### Added

- `RepoGraph` v1 独立 layer (`src/repo_graph/*`) (#468)。`mod.rs` 公開境界 + builder / `parse.rs` 言語別 regex parser / `pair.rs` `likely_covers` ペアリング / `persist.rs` JSON I/O + LRU / `fingerprint.rs` git rev + path hash の 5 ファイル構成。`agent` / `session` のいずれにも依存せず `util::file_classify` / `util::git_hardened` のみを参照。public 4 型 (`RepoGraph` / `BuildOutcome` / `BuildOptions` / `RepoGraphError`) + ImportRef/ImportKind。const SSOT 5 種 (`MAX_REPO_GRAPH_FILES=50_000` / `_DEPTH=32` / `_FILE_BYTES=1MiB` / `_TOTAL_BYTES=16MiB` / `_FILES_PERSISTED=8`)。永続化先 `state_root/repo_graph/<fingerprint_id>.json`。`Agent::new` 直後に同期 1 回 build、env gate `ANVIL_NO_REPO_GRAPH`。
- Graph-aware repo context ranking (#469)。`src/agent/prompting.rs` に `WEIGHT_*` (file_name=8 / path_token=5 / path_contains=3 / content_token=2 / content_contains=1 / symbol=4 / test_impl_pair=7 / suspected_file=10 / changed_file=4 / graph_neighbor=6) と `BONUS_PATH_CONTENT_COHERENCE=3` の 11 要素を SSOT 集約。`RepoContextInputs<'a>` 入力束ね型で signature を 3 引数化。`rank_repo_candidates` は `score_lexical` / `score_graph` 純関数 + graph 第二パス (`build_neighbor_index` + `resolve_import_target_to_file`) の 3 段で score 計算。`ANVIL_NO_GRAPH_RANKING` env gate で graph 由来 score を 0 に。
- Path-aware ANVIL.md instructions (#470)。ワークスペースルートの `ANVIL.md` をリポジトリ固有の指示ファイルとして読み込み、システムプロンプトに注入する仕組みを追加。
- `util/git_hardened.rs` (#468)。`pub(crate) fn run_git(work_root, args) -> Option<String>` の単一情報源。`case_record` の private `run_git` を物理移設し、`session::case_record` と `repo_graph::fingerprint` の双方が共有する。`-c core.sshCommand= -c credential.helper= -c core.pager=cat -c core.hooksPath=/dev/null -c gpg.program=/dev/null` + `GIT_*` env scrub + 1s timeout の hardening を集約。
- AGENTS.md、workspace 成果物 (UAT records / eval runs / agent skills / v0.1.1 docs) を追加 (#524)。

### Fixed

- readline history テストの rustyline 14 umask race を process-wide Mutex でシリアライズして修正 (#474)。`History::save()` が `umask` を非アトミックに変更する window に他スレッドの `tempdir()` が入ると EACCES が発生する問題を解消。

## [0.4.0] - 2026-04-29

Epic C: Case Memory & CBR + Epic D: Agentic Skills Layer を統合リリース。turn 単位の手続き記憶を CaseRecord として永続化し、6-element Jaccard 類似度で過去ケースを次 turn の prompt に注入。失敗パターンを AntiPattern として upsert し、繰り返し閾値超で "Avoid Patterns:" を注入する。さらに `AgentSkill` trait + `SkillRegistry` 基盤を導入し、Reminder / Verifier を skill ディスパッチに移植、`SkillTrustTier` で read-only skill の Write 違反を runtime block する。

### Added

- `CaseRecord` 抽出 + `state_root/cases/<case_id>.json` 永続化 (#462)。Act mode の post-loop hook で `last_anvil_score` の success 条件を満たす turn から手続き記憶 (case_id / repo_fingerprint / task_signature / language_stack / initial_feedback / successful_precautions / changed_files_summary / verify_commands / outcome_score) を抽出。`MAX_CASE_RECORD_BYTES=16 KiB`, `MAX_CASE_RECORDS=256` (lazy LRU eviction)、`case_[a-z0-9_-]{16,64}` allowlist。git_remote 取得は `Command` に `-c core.sshCommand= -c credential.helper= -c core.pager=cat -c core.hooksPath=/dev/null -c gpg.program=/dev/null` + 全 `GIT_*` env 削除 + 1s timeout で hardening し、`mask_secrets` → `sanitize_git_remote` (rfind('@') で IPv6 / 多重@ / scheme 無し ssh shorthand 対応) を通す。per-turn cap = 1、Plan mode disable、`ANVIL_NO_CASE_RECORD` / `ANVIL_CASE_RECORD_DRY_RUN` 環境変数。
- Case retrieval (#463)。新 turn 開始時に `state_root/cases/` を母集団に 6-element Jaccard 重み付き和（W_TASK 0.30 / W_STACK 0.10 / W_REPO 0.20 / W_FILES 0.20 / W_KIND 0.10 / W_PRECAUTIONS 0.10、合計 1.0）で score し、閾値 `CASE_RETRIEVAL_SCORE_THRESHOLD=0.40` 以上を `MAX_SELECTED_CASES=3` 件まで選定。`format_for_prompt` は per-case `MAX_CASE_RENDERED_CHARS_PER_CASE=240` / total `MAX_CASE_RENDERED_CHARS_TOTAL=1024` cap。`try_inject_case_retrieval_message` adapter で Act mode prompt に "Relevant Local Cases:" セクションを注入。Plan mode / per-turn cap / `ANVIL_NO_CASE_RETRIEVAL` を adapter 内で完結判定 (S3-008 layer 分離)。
- AntiPattern (#464)。`(workspace_key, task_signature, feedback_kind)` をキーに失敗パターンを upsert し、`repeat_count >= 2` で "Avoid Patterns:" system message 注入。env gates / per-turn cap で過剰注入を防御。
- `AgentSkill` trait + `SkillRegistry` 基盤 (#465)。`SkillTrigger::{IterationInternal, MessageBuild, PostLoop}` で 3 か所の dispatch site を統一。Reminder Sidecar を thin trait adapter に移行し、既存 4 reminder regression + 7 ReminderGate test を温存。
- VerifierSkill (#466)。post-loop verifier dispatch を `SkillRegistry::invoke(VerifierSkill)` 経由に切替。AnvilScore 計算 / AutoTest runner / verify-command 収集を skill ディスパッチに統合。
- `SkillTrustTier` enum 4 variant + `AgentSkill::tier()` + `SkillOutput::PermissionDenied` + `FeedbackKind::SkillPermissionDenied` + `agent.skill.permission_denied` jsonl event (#467)。read-only skill による誤 Write を runtime block し、permission 違反を FeedbackFrame 経由で記録。
- Epic A/B/C/D の orchestration plans / summaries を `workspace/orchestration/runs/{2026-04-27,2026-04-28,2026-04-29,2026-04-29-epic-d}/` 配下に追加。

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
