# CLAUDE.md

## Repo Intent

このリポジトリは `workspace/v0.1.0` に基づく Rust 版 local-first coding agent の実装。
旧来の Anvil 拡張ではなく、vibe-local 的な小さい Ollama 専用実装へ振り切っている。

## Current Architecture

- `src/config.rs`
  - CLI / env / `.anvil/config` のマージ
- `src/model_registry.rs`
  - 利用可能モデルとメモリ量から main / sidecar を選択
- `src/ollama/client.rs`
  - `/api/tags` `/api/chat` 呼び出し
- `src/ollama/xml_fallback.rs`
  - `<think>` 除去と XML tool call 回収
- `src/tools/*`
  - built-in tools
- `src/agent/loop_run/spinner.rs`
  - 推論 / ツール実行中のスピナー（stderr, 80ms, TTY / `NO_COLOR` / `ANVIL_NO_SPINNER` / UTF-8 分岐）
- `src/agent/loop_run/interrupt.rs`
  - ESC 割り込み monitor（stdin raw mode + daemon thread、`ANVIL_NO_INTERRUPT` / 非TTY / approve prompt で自動無効化）
- `src/agent/loop_run/footer.rs`
  - 固定フッター daemon (stdout, 200ms, DECSTBM 再適用、token / mode / log / yes 表示、current_cols broadcast for turn.rs progress line)
- `src/tui/markdown.rs`
  - assistant 応答の SGR-only markdown renderer（行バッファ式、`<think>` strip、`ANVIL_NO_MARKDOWN` / `NO_COLOR` / `is_terminal` で多段無効化、session storage は raw LLM text のまま保持）
- `src/modes/plan_act.rs`
  - Plan / Act の単純な状態
- `src/session/*`
  - セッション保存と compaction
  - `discovery.rs`: `iter_session_dirs` 共有イテレータ（UUID/symlink/size/parse 防御）
  - `sessions_cli.rs`: `anvil sessions list|show|clean` と `--resume <ID>` の path confinement / plan / I/O
  - `feedback.rs`: `FeedbackFrame` / `FeedbackKind`（Bash / auto_test / tool parser / unsafe block / no-progress / edit failure を `SessionSnapshot.last_feedback` として正規化、excerpt 8 KiB cap + secret mask + workspace 相対 PathBuf、sealed `from_draft` ＋ `build_feedback_frame`）
  - `precaution.rs`: `WorkingMemory.active_precautions` 用の `Precaution` 型群（serde snake_case、deterministic id、status/severity/source enum）。Issue #453 で `severity_order(Severity) -> u8` helper を追加（High=0, Medium=Unknown=1, Low=2、turn.rs の prompt selector 用 stable sort key）。
  - `store.rs`: `WorkingMemory` 本体と `format_for_prompt()` wrapper / `format_for_prompt_with_precautions(&[Precaution])` 新メソッド (Issue #453)。後者は呼び出し側 (`turn.rs::select_precautions_for_prompt`) が severity sort + 関連度フィルタ + token budget cap (`MAX_ACTIVE_PRECAUTIONS_PROMPT=8`, `MAX_ACTIVE_PRECAUTIONS_CHARS=1024`) を適用済みの slice を渡す前提で render する。renderer は `status == Active` を防御的に再 filter (S5-001)。前者は full active-only list を渡す薄い wrapper で、Reminder Sidecar 経路 / 既存 4 regression test の互換維持に使う。Issue #456 で `SessionSnapshot` に AnvilScore 関連 5 field 追加 (`last_anvil_score` lossy persist + `unsafe_blocks_this_turn` / `repo_edit_succeeded_this_turn` / `touched_files_at_turn_start` runtime + `consecutive_no_progress_turns` session-cumulative)、`load_or_new` で `MAX_SESSION_JSON_BYTES` size cap を read 前検査。
  - `anvil_score.rs`: Issue #456 で導入。`AnvilScore` 値オブジェクト (12 field / 5 lifecycle group) + `compute_anvil_score` 純関数 + `AnvilScoreInputs` / `AnvilTestSummary` view + `AnvilScoreSnapshot<'_> { PreviousTurn / CurrentTurn }` enum (serde 非対応、Reminder 内部流通) + `format_for_prompt(MAX_RENDERED_CHARS=512)` 単一情報源 renderer + `sanitize` / `deserialize_lossy_anvil_score` (RawValue 経由、`MAX_ANVIL_SCORE_RAW_BYTES=64KiB` cap)。turn.rs から呼び出され、`agent.anvil_score.computed` jsonl event は `session_id` / `score` / `render_chars` / `compute_ms` のみを payload に含める。
  - `tmp_tests.rs`: tmp-test の lifecycle (#458)。`state_root/sessions/<id>/tmp-tests/{files,metadata}/` 配下で管理。`TmpTest`(serde snake_case) / `TmpTestStatus` (Draft/Promoted/Discarded) / `TmpTestRunResult` (将来用 reserved)。`create_generated_test` / `discard_tmp_test` / `promote_tmp_test` / `list_tmp_tests`。エラー型は既存 `Result<_, String>` を踏襲 (`thiserror` 不可、設計判断 #8)。`derive_test_id` (sha256 + created_at) / `validate_test_id` (`tmp_[a-z0-9_-]{16,64}` allowlist) / `validate_tmp_test_relative_path` (NUL/`..`/abs/1024 byte 拒否)。`MAX_TMP_TEST_METADATA_BYTES=64KiB`, `MAX_TMP_TESTS_PER_SESSION=4096`。promote は `auto_approve=false && interactive_approval=false` で拒否、衝突時は `--force` 必須、symlink dst は force 時も拒否、`OpenOptions::create_new(true)` で TOCTOU 回避。auto_test 経路には載せない。session 単位 cleanup は既存 `run_clean` が吸収。流儀は `precaution.rs` 寄り (型定義 + 自由な construction): promote/discard で status を上書きする lifecycle のため、`feedback.rs` の sealed `build_feedback_frame` 流儀は採用しない。`ToolContext.tmp_tests_root: Option<PathBuf>` (registry.rs の 9 番目フィールド) と shared helper `resolve_tmp_tests_path` で Read/Write/Edit の `tmp-tests/<rel>` prefix を session-scoped root に routing し、Plan mode 判定より前で評価することで Plan 中の LLM が tmp-tests へ書き戻すユースケースを塞がない。CLI は `anvil sessions tmp-tests {promote,discard,list} --session <id>` で sessions 体系に集約 (新規 top-level なし)。Issue #459 で 10 番目フィールド `ToolContext.tester_active: bool` を追加し、`enforce_tmp_tests_only_when_active` で Tester 起動中の Edit/Write を `tmp-tests/` prefix 強制 (registry 層の path confinement、in-memory のみで session JSON には書き出さない)。
- `src/util/file_classify.rs`
  - Issue #456 で導入。`is_test_file` / `is_setup_file` / `is_implementation_file` の単一情報源。`agent::orchestration::verify_repo_progress` と `session::anvil_score` の双方から参照され、layer 逆転 (session → agent) を起こさず DRY を維持する (DR1-007)。
- `src/agent/loop_run/reminder.rs`
  - Reminder Sidecar (#452): FeedbackFrame の失敗系 kind が記録された後、sidecar Ollama を 1 回だけ同期呼び出し（`tools=None`、JSON-only、`<think>` strip + first JSON object 抽出、tool_calls 非空は no-op）して `WorkingMemory::add_precaution` に書く。closure-based pure 関数 `run_reminder_with_strategy` + `Agent::maybe_invoke_reminder` wrapper（iteration-internal hook + post-loop hook）。`reminder_called_this_turn` で per-turn cap = 1。`ANVIL_NO_REMINDER` で disable。Logging: `agent.reminder.{completed,failed,skipped}` を `llm-io.jsonl` に。sidecar の `active_precautions_summary` 入力は `format_for_prompt()` wrapper 経由で full active-only list（cap 適用前）を受け取り、重複 push 抑止精度を保つ (Issue #453 設計判断 #5)。Issue #456 で `ReminderInputs.anvil_score: Option<AnvilScoreSnapshot<'_>>` 追加。`AnvilScore::MAX_RENDERED_CHARS = 512` で renderer 側 truncate、prompt boundary は `# Active precautions` の後 / `# Latest feedback` の前で、最終 `# Output\nJSON only.` guard が残ることを test で固定。iteration-internal hook では `PreviousTurn(&last_anvil_score)`、post-loop hook では `CurrentTurn(&computed_score)` を `anvil_score_computed_this_turn` フラグで切り替える (DR1-006)。
- `src/agent/loop_run/tester.rs`
  - Tester Skill v1 (#459): `AutoTestRunner::detect == None` かつ `TesterCandidate::detect == Some` (Rust / Node / Python のいずれか) のとき、Act-mode の post-loop hook で main model を 1 回だけ同期呼び出し（`tools=None`、JSON-only、`<think>` strip + first JSON object 抽出、tool_calls 非空は abort）して smoke test を生成し、`tmp-tests/files/` に保存したうえで固定テンプレートの Bash（Rust: `cargo test --manifest-path`、Node: `node --check`、Python: `python3 -m py_compile`）を `TESTER_SMOKE_TIMEOUT_SECS=30` の明示 timeout 付きで実行する。Rust 経路は `state_root/sessions/<id>/tester-runs/<run_id>/` に transient harness（path dependency = workspace package、`tmp-tests/` とは独立 dir、DR1-009）を生成し、終了後に best-effort cleanup する。closure-based pure 関数 `run_tester_with_strategy<F, B, A>` + `Agent::try_invoke_tester` wrapper。`tester_called_this_turn` で per-turn cap = 1。`ANVIL_NO_TESTER` / Plan mode で disable。disable 順序は per-turn cap → Plan mode → `ANVIL_NO_TESTER` → no candidate（`check_invocation_gate` で production / E2E 共有 single source of truth）。承認モードは runtime 派生: `--yes` → `Auto`、TTY → `Interactive`（y/yes 以外 abort）、それ以外 → `Forbidden`。LLM reply は `validate_llm_reply` で空 / `<think>`-only / `>64 KiB` (`MAX_TESTER_LLM_REPLY_BYTES`) / `MAX_GENERATED_TESTS_PER_TURN=1` 超 / `tmp-tests/` 外 path / ASCII control char in path を防御 (`AbortReason::LlmMalformed` / `PathConfinementViolation`)。`build_command` は固定テンプレートで absolute `shell_quote`-escape、`classify_tester_outcome` は薄い分類器で `classify_auto_test` は流用しない (DR1-010)。`run_id` は `derive_run_id`(sha256 + timestamp) + `validate_run_id` で `run_[a-z0-9_-]{16,64}` allowlist 強制 (DR4-003)。Logging: `agent.tester.{llm_call_started,llm_call_completed,llm_call_failed,completed,failed,skipped}` を `llm-io.jsonl` に。`sanitize_tester_log` (`mask_secrets` + `truncate_for_log` cap=`TESTER_LOG_CAP=20_000`) で payload を防御 (DR4-006)。E2E は `tests/tester_skill_smoke.rs` に closure DI で 12 ケース (Ollama 不要、0.09s)。
- `src/agent/loop_run/auto_test.rs`
  - Watcher と connect する auto-test runner。Issue #459 で `pub(super)` 化された helper を Tester から流用する: `has_cargo_manifest(work_root)` / `package_json_has_test_script(work_root)` / `has_python_surface(work_root, &[String])` / `first_python_script(work_root) -> Option<PathBuf>` / `shell_quote(&Path) -> String` / `MAX_OUTPUT_BYTES = 12_000`。`classify_auto_test` は Tester では流用せず、Tester 側で独自の `classify_tester_outcome` を持つ (DR1-010)。
- `src/git/checkpoint.rs`
  - checkpoint / rollback

## Development Expectations

- 旧アーキテクチャへ戻す方向の継ぎ足しはしない
- provider abstraction を増やさない
- local LLM 向けの prompt / loop の単純さを優先する
- 新機能は E2E 寄りの検証を伴わせる

## Release Process

変更しない。

- package / binary は `anvil`
- `.github/workflows/ci.yml` は `fmt` `clippy` `test` `build`
- `.github/workflows/release.yml` は tag push 時に gzip artifact を作って GitHub Release を切る
