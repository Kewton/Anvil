# 残SG根本対策 実装作業計画

作成日: 2026-06-25

対象SG:

- `SG-07`, `SG-08`, `SG-11`
- `SG-14`, `SG-15`, `SG-16`, `SG-18`, `SG-19`, `SG-20`, `SG-21`
- `SG-25`, `SG-31`, `SG-32`, `SG-34`, `SG-35`, `SG-36`, `SG-38`

この計画は、前回 `defer:別計画` としたSGを全て実装完了まで持っていくための追加計画である。今回は `defer` を残さない。最終完了条件は `mvp/anvilminimal/tests/safety_parity_traceability.rs` から `defer:` が消え、対象SGがすべて実テスト名へ対応すること。

## 実施結果

2026-06-25 時点で Phase R0〜R8 を実施し、対象SGの `defer:` は `mvp/anvilminimal/tests/safety_parity_traceability.rs` から削除した。

- R1: `PlanStep.expected_result`、typed `StepKind` 正規化、step kind contract、semantic lint、aggregated `VerificationReport` を実装。
- R2: `WorkspacePolicy` を tool context へ接続し、Read/Glob/Grep の metadata/generated directory 抑制と large output shaping を実装。
- R3: Edit の already-applied/no-op/normalized-line/token-anchor fallback と recoverable feedback 分類を実装。
- R4: Bash structured outcome、dangerous classifier、timeout/cancel、large output truncation、failure summary を実装。
- R5: final 前 relative import scanner と compaction evidence protection を実装。
- R6: `RunSessionOutcome`、step/repair iteration cap、bounded repair report を実装。
- R7: Next.js Tailwind/rootDir/alias/script weakening contract と data profile raw/input snapshot verification を実装。
- R8: traceability を実テスト名へ置換し、`safety_traceability_has_no_defer_after_remaining_sg_completion` を追加。

実施時の verification:

- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`

追加の最終 verification として、`cargo clippy --manifest-path mvp/anvilminimal/Cargo.toml --all-targets -- -D warnings` を実行する。

## 全体方針

- 既存の `anvilminimal` 操作性を維持する。
  - `--prompt`
  - `--plan-steps`
  - `--plan-run`
  - `--ultra-plan-run`
  - TUI slash command
- 先に regression test を入れ、fake client / temp workspace / local process で deterministic に red/green を確認する。
- provider live test は最後に分離する。通常の acceptance は network/API key なしで判定可能にする。
- 旧 heavy loop の汎用 mechanism ledger は入れない。ただし今回対象SGに含まれる direct minimal safety は MVP 側に実装する。
- 各Phaseの完了時点で `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` と `cargo clippy --manifest-path mvp/anvilminimal/Cargo.toml --all-targets -- -D warnings` を通す。

## レビュー結果と反映

| 観点 | 指摘 | 反映 |
|---|---|---|
| SG網羅 | 対象SGは列挙されているが、最終的に `defer:` が消えたことを機械的に検証する条件が R8 まで遅い | R0 で最終テスト名を先に固定し、R8 で `defer:` ゼロを必須にする方針を維持 |
| 型移行リスク | `PlanStep.kind` と `VerifyReport` の型変更は runner/profile/repair/eval へ波及する。互換 shim がないと既存テストを大きく壊す | R1 に wire format 互換、`StepKind::Unknown`, `VerificationReport` 互換 accessor を追記 |
| workspace policy | `.anvil` を全面ブロックすると `run-plan` や内部 plan/report 読み取りと衝突し得る | R2 に「tool経由の通常taskだけ制限し、内部filesystem readは専用許可」と明記 |
| Bash OS差分 | process group kill は Unix と非Unixで実装差が出る。テスト条件を分けないと CI が不安定になる | R4 に Unix-only child cleanup test と non-Unix best-effort test を分離 |
| import scanner | static import/require/export と dynamic import、CSS/JSON import の扱いが曖昧 | R5 に対応構文と除外条件を明記 |
| repair API | `run_session` の戻り値を変えると既存操作性を壊す | R6 に `RunSessionOutcome` は内部API、既存 string API は wrapper 維持と追記 |
| profile snapshot | data profile の phase前後 snapshot をどこで保持するか不明 | R7 に `ProfileSnapshot` 相当の lifecycle hook と保存場所を追記 |
| eval判定力 | 新 failure kind の分類だけでは、各SGの deterministic fixture が十分か判断しづらい | R8 に SG別 fixture matrix と success gate を追加 |

## 完了条件 / テスト計画レビュー追補

| 観点 | 指摘 | 対応 |
|---|---|---|
| R0/R8 の責務 | R0 の追加テスト名に `no_defer` が含まれており、R0時点で未実装SGが残る方針と矛盾する | R0 は予定テスト名の登録確認に限定し、`defer:` ゼロは R8 の最終ゲートへ移動 |
| SG別判定 | 各Phaseにはテスト名があるが、SG単位で unit / integration / eval のどこで完了判定するかが一覧化されていない | SG別 Test Coverage Matrix を追加 |
| regression強度 | unit test だけでは CLI/TUI 操作性、eval分類、raw event 形状の regressions を取りこぼす | Phaseごとの共通完了ゲートに CLI/TUI/eval smoke を追加 |
| OS依存 | Bash child cleanup は Unix と非Unixで同じ合格条件にできない | SG-32/38 の完了条件を Unix-only cleanup と non-Unix structured timeout に分割 |
| 互換性 | typed plan / structured Bash / outcome API は既存 public wrapper を壊す可能性がある | R1/R4/R6 に backward compatibility tests を必須化 |
| data安全性 | data profile snapshot が raw data 内容を `.anvil` に複製すると安全装置自体が漏洩元になる | SG-34 に raw content non-copy test を追加 |

## SG別 Test Coverage Matrix

| SG | Unit | Integration | Eval / Report | 完了判定 |
|---|---|---|---|---|
| SG-07 | relative import resolver / scanner tests | fake loopで no-tool final 前に missing import feedback | `relative_import_missing` fixture | 未解決 relative import が final success にならない |
| SG-08 | Edit typed error / feedback tests | fake loopで Edit mismatch 後に Read誘導 | `edit_recoverable_error` fixture | recoverable Edit error と path/security error を分類できる |
| SG-11 | workspace policy allow/block tests | normal loop tool context が `.anvil` 等を隠す | `workspace_policy_blocked` fixture | tool経由の通常探索で controller metadata を読まない |
| SG-14 | StepKind / expected_result parse-render tests | legacy plan file run compatibility | plan scoring fixture | typed contract を保持しつつ既存YAMLが読める |
| SG-15 | step kind contract lint tests | generated planが契約違反時に実行されない | plan-quality score fixture | inspect/setup/implement/verify/report の責任境界が壊れない |
| SG-16 | semantic lint tests | Next.js/Rust/Python代表planで順序違反を拒否 | plan-quality score fixture | setup before verify / expected path ownership を検証できる |
| SG-18 | step/repair cap config tests | fake clientで global maxより短く step failure | stop reason event fixture | step単位の失敗が粗い global max_iterations へ落ちない |
| SG-19 | progress detector tests | repair no-progress fake client | `repair_exhausted` fixture | missing path減少なし・同一path反復を warning/report に出す |
| SG-20 | repair report rendering tests | repair exhausted file生成 | report parser fixture | 人間が missing paths / failures / next command を読める |
| SG-21 | VerificationReport aggregation tests | repair promptに複数failureが入る | failure breakdown fixture | first failure だけで診断が欠落しない |
| SG-25 | Next.js contract unit tests | ultra phase後 profile verify | `profile_contract_failed` fixture | script weakening / Tailwind/rootDir/alias drift を止める |
| SG-31 | compaction priority tests | long loopで Read/Edit evidence保持 | context compaction fixture | compaction後も修復に必要な証跡が残る |
| SG-32 | Bash classifier / timeout / cancellation tests | TUI interrupt + Bash fake/short process | `bash_timeout` / `bash_cancelled` fixture | timeout/cancel/block が structured outcome になる |
| SG-34 | data snapshot / mutation tests | ultra data profile phase前後 snapshot | `data_input_modified` fixture | raw/input data の削除・改変が pass しない |
| SG-35 | Read/Glob/Grep bounds tests | workspaceに generated dirs を置く integration | output-size fixture | metadata/generated dirs と巨大出力で context を壊さない |
| SG-36 | Edit fallback / already-applied tests | fake loopで fallback成功 | `edit_recoverable_error` fixture | 空白差・既適用・曖昧anchorを正しく扱う |
| SG-38 | Bash output shaping tests | build/test failure summary integration | large-output fixture | Bash result が診断向きに要約される |

## Phase共通完了ゲート

各Phaseは個別テストに加え、少なくとも次を満たす。

- `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
- `cargo clippy --manifest-path mvp/anvilminimal/Cargo.toml --all-targets -- -D warnings`
- `python3 -m unittest discover -s mvp/anvilminimal/tests/eval`
- 既存 CLI/TUI 操作性に触れたPhaseでは `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test tui_repl --test tui_integration` 相当を追加実行する。
- eval分類や report に触れたPhaseでは、該当 fixture の Python unit と dry-run eval command 生成を確認する。

## SG-to-Phase

| Phase | 対象SG | 主な目的 |
|---|---|---|
| Phase R0 | 全対象SG | red baseline、traceability更新方針、既存互換確認 |
| Phase R1 | SG-14, SG-15, SG-16, SG-21 | PlanStep / VerifyReport の契約を source parity へ近づける |
| Phase R2 | SG-11, SG-35 | workspace policy と Read/Glob/Grep の探索・出力制御 |
| Phase R3 | SG-08, SG-36 | Edit の recoverable feedback と fallback |
| Phase R4 | SG-32, SG-38 | Bash の timeout/cancel/structured outcome/output shaping |
| Phase R5 | SG-07, SG-31 | final前 import scanner と compaction evidence protection |
| Phase R6 | SG-18, SG-19, SG-20 | step/repair cap と progress-aware bounded repair |
| Phase R7 | SG-25, SG-34 | Next.js/data profile contract の補完 |
| Phase R8 | 全対象SG | eval/TUI/traceability の最終収束 |

## Phase R0: Red Baseline と作業境界固定

### 対象ファイル

- `mvp/anvilminimal/tests/safety_parity_traceability.rs`
- `mvp/anvilminimal/tests/*`
- `mvp/anvilminimal/src/**`

### 作業

- `defer:` の各SGに対し、最終的に対応するテスト名を先に決める。
- 各SGについて、実装前に失敗する regression test を追加する。
- red baseline は実装前確認に限定し、default suite に失敗を残さない。
- 作業中は `#[ignore]` を使わず、実装と同一コミット内で green 化する。
- 既存コミット `ef26879` の traceability table を起点にし、対象SGの `defer:` 行を「予定テスト名」と「実装Phase」に分解した一時表へ置換する。
- R0 で追加するテストは「最終DoDの存在確認」までに留め、未実装SGを理由に default suite が失敗する assertion は入れない。

### 追加テスト

- `remaining_sg_traceability_targets_are_defined`
- `remaining_sg_regression_suite_is_registered`

### 受け入れ条件

- 対象SGすべてに最終テスト名が割り当てられている。
- このPhase完了時点では `defer:` が残っていてよいが、R8の完了条件として削除することが明記されている。
- R0 のコミットでは、実装済みと未実装の境界が文書・traceability・テスト名で一致している。

## Phase R1: PlanStep / VerifyReport 契約強化

### 対象SG

- SG-14: typed `StepKind` / `expected_result`
- SG-15: step kind contract
- SG-16: semantic plan lint
- SG-21: verification failure aggregation

### 対象ファイル

- `mvp/anvilminimal/src/planner/step_plan.rs`
- `mvp/anvilminimal/src/planner/lint.rs`
- `mvp/anvilminimal/src/planner/verify.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/repair.rs`
- `mvp/anvilminimal/scripts/eval_lib/plan_scoring.py`

### 実装方針

- `PlanStep.kind` は YAML wire format では従来どおり string を維持し、Rust 側で `StepKind` enum へ正規化する。
  - `inspect`
  - `setup`
  - `implement`
  - `verify`
  - `report`
  - 既存YAML互換のため unknown は parse error ではなく `StepKind::Unknown(String)` として読み、lint error にする。
- `PlanStep.expected_result` を追加する。
  - `pass`
  - `fail`
  - 既存planは default `pass` として読む。
- `VerifyStatus` 単一値から、複数 failure を保持できる `VerificationReport` へ拡張する。ただし既存 caller 互換のため、移行Phase中は `status` 互換 accessor を残す。
  - `missing_paths: Vec<String>`
  - `command_failures: Vec<CommandFailure>`
  - `dependency_missing: Vec<String>`
  - `profile_failures: Vec<String>`
- `is_pass()` は aggregation 後の総合判定として維持する。
- `profile_failure(...)`、`verify_profile(...)`、`repair_prompt(...)`、`run_step(...)` は R1 内で同時に移行し、中間状態を残さない。
- `Debug` 表示だけに依存した repair report は廃止し、structured fields から Markdown を生成する。
- `lint_step_plan` に step kind contract を追加する。
  - `inspect`: write/edit expected_paths 禁止、verify command 禁止
  - `setup`: build verify 禁止、install/setup command は明示許可された setup step のみ
  - `implement`: expected_paths なしの実装stepを warning/error 扱い
  - `verify`: write/edit を促す instruction 禁止、verify command 必須
  - `report`: file変更・verify command 禁止
- semantic lint を追加する。
  - dependency setup before verify
  - Next.js build before package/entry creation を拒否
  - expected_paths が曖昧すぎる step を拒否
  - duplicate path ownership を warning ではなく error にする。

### 追加テスト

- `step_plan_parses_typed_kind_and_expected_result`
- `legacy_step_plan_defaults_kind_and_expected_result`
- `step_kind_contract_rejects_setup_with_build_verify`
- `inspect_step_rejects_expected_paths`
- `verify_step_requires_verify_command`
- `implement_step_requires_concrete_expected_paths`
- `semantic_lint_rejects_next_build_before_entrypoint`
- `semantic_lint_rejects_dependency_verify_without_setup`
- `verify_step_aggregates_missing_paths_and_command_failures`
- `verify_expected_result_fail_accepts_nonzero_command`
- `verify_expected_result_pass_rejects_nonzero_command`
- `plan_scoring_uses_typed_kind_and_expected_result`
- `verification_report_status_compat_accessor_matches_primary_failure`
- `profile_failure_populates_aggregated_report`

### 受け入れ条件

- 既存 plan YAML が backward compatible に parse できる。
- 新 plan YAML は typed kind / expected_result を round-trip できる。
- verify failure が1件目で打ち切られず、repair prompt に複数 failure が入る。
- plan scoring で responsibility boundary と verify contract が加点/減点対象になる。
- profile verification と repair prompt が新旧 `VerificationReport` 形状の差で情報欠落しない。

## Phase R2: Workspace Policy と Read/Glob/Grep 制御

### 対象SG

- SG-11: controller metadata read/discovery 抑制
- SG-35: Read/Glob/Grep の ignore walker、large output 要約、metadata filtering

### 対象ファイル

- `mvp/anvilminimal/src/tools/workspace_policy.rs`
- `mvp/anvilminimal/src/tools/registry.rs`
- `mvp/anvilminimal/src/tools/read.rs`
- `mvp/anvilminimal/src/tools/glob.rs`
- `mvp/anvilminimal/src/tools/grep.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`

### 実装方針

- `WorkspacePolicy` を placeholder から実効ポリシーへ変更する。
  - `NormalTask`
  - `ControllerMetadataAllowed`
  - `GeneratedArtifactsAllowed`
- `ToolContext` に `workspace_policy` を追加する。
- policy は tool 経由の `Read` / `Glob` / `Grep` に適用する。`run-plan` が plan file を読む処理、repair report 保存、eval artifact collection などの内部 filesystem access は専用関数で扱い、通常 task policy と混同しない。
- 通常taskでは次を read/discovery から除外する。
  - `.git`
  - `.anvil`
  - `.next`
  - `target`
  - `node_modules`
  - coverage/cache directories
- `Read` は上限を持つ。
  - max bytes
  - max lines
  - truncation marker
- `Glob` / `Grep` は ignore walker を共有する。
- `Grep` は match件数と各match前後行に上限を持つ。
- `.anvil` plan/report を読む必要がある明示コマンドは別 mode とする。
- path rejection は user-facing tool result では `workspace_policy_blocked` として分類できる文言に統一する。
- Glob/Grep の ignore 判定は path component ベースで行い、`my-node_modules-note.md` のような通常ファイル名を誤除外しない。

### 追加テスト

- `normal_workspace_policy_blocks_anvil_metadata_read`
- `normal_workspace_policy_blocks_node_modules_glob`
- `normal_workspace_policy_blocks_target_grep`
- `controller_metadata_policy_allows_anvil_read`
- `read_large_file_is_truncated_with_marker`
- `grep_large_result_is_summarized`
- `glob_uses_ignore_walker`
- `tool_context_passes_workspace_policy_from_loop`
- `run_plan_internal_read_is_not_blocked_by_normal_workspace_policy`
- `workspace_policy_component_match_does_not_block_similar_file_names`

### 受け入れ条件

- 通常の minimal loop で `.anvil`, `target`, `node_modules` を探索しない。
- 明示許可された内部処理だけが `.anvil` を読める。
- Read/Grep/Glob の出力が context budget を壊さない。
- policy blocked は eval event と failure classification で `workspace_policy_blocked` として観測できる。

## Phase R3: Edit Feedback と Fallback

### 対象SG

- SG-08: Edit anchor mismatch feedback
- SG-36: already-applied/no-op/normalized-line/token-anchor fallback

### 対象ファイル

- `mvp/anvilminimal/src/tools/edit.rs`
- `mvp/anvilminimal/src/tools/registry.rs`
- `mvp/anvilminimal/src/minimal_loop/feedback.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`

### 実装方針

- `Edit` の失敗種別を typed error にする。
  - `AnchorNotFound`
  - `AlreadyApplied`
  - `NoOp`
  - `AmbiguousAnchor`
  - `PathRejected`
- exact anchor が失敗したら normalized-line fallback を試す。
- normalized fallback でも失敗した場合、短い token anchor fallback を試す。
- already-applied は success 扱いにする。
- no-op は recoverable feedback として返す。
- ambiguous は hard error ではなく `Read` し直しを促す feedback にする。
- loop は Edit recoverable error を tool result として残し、次 request の feedback にも反映する。
- `tool_error_kind` に Edit 専用分類を追加する。
  - `edit_anchor_not_found`
  - `edit_noop`
  - `edit_ambiguous_anchor`
  - `edit_already_applied`
- `PathRejected` は security boundary なので recoverable feedback へ落とさない。
- fallback は最大1回ずつに限定し、複数候補がある場合は自動適用せず `Read` し直しへ誘導する。

### 追加テスト

- `edit_anchor_mismatch_returns_recoverable_feedback`
- `edit_already_applied_is_success`
- `edit_noop_returns_recoverable_feedback`
- `edit_normalized_line_fallback_applies_change`
- `edit_token_anchor_fallback_applies_change`
- `edit_ambiguous_anchor_prompts_read_again`
- `edit_path_rejection_remains_hard_error`
- `edit_anchor_feedback_is_ephemeral`
- `edit_recoverable_error_emits_eval_event_kind`
- `edit_fallback_does_not_apply_when_multiple_candidates_exist`

### 受け入れ条件

- 軽微な空白差・改行差で Edit が不要に失敗しない。
- LLM が修復できる Edit 失敗は max_iterations ではなく feedback loop に戻る。
- path escape は引き続き hard error。
- eval 上で Edit recoverable failure と path/security failure を区別できる。

## Phase R4: Bash Safety と Output Shaping

### 対象SG

- SG-32: dangerous command classifier、timeout、process group、cancel flag、structured outcome
- SG-38: build/test summary、large output truncate

### 対象ファイル

- `mvp/anvilminimal/src/tools/bash.rs`
- `mvp/anvilminimal/src/tools/registry.rs`
- `mvp/anvilminimal/src/tui/interrupt.rs`
- `mvp/anvilminimal/src/planner/verify.rs`

### 実装方針

- `BashOutcome` を追加する。
  - `Success`
  - `CommandFailed`
  - `Blocked`
  - `Timeout`
  - `Cancelled`
- dangerous command classifier を substring から typed classifier へ拡張する。
  - destructive filesystem
  - credential exfiltration
  - network pipe-to-shell
  - privilege escalation
  - background/dev-server long-running
- timeout を config / tool default で設定する。
- Unix では process group を分離し、timeout/cancel で子プロセスまで kill する。
- Windows/非Unix は best effort として実装し、child cleanup の強い assertion は Unix-only test に分離する。
- TUI interrupt の cancel flag と Bash runner を直接結合しすぎない。`CancellationToken` 相当の薄い抽象を tool context に渡し、非TUI CLI では常に not-cancelled とする。
- stdout/stderr を shaping する。
  - max bytes
  - build/test failure summary
  - large `cat` style output truncate
  - status/elapsed/outcome を structured text で返す。
- verify では `Timeout` / `Cancelled` / `DependencyMissing` を分類して report に載せる。
- `run_checked` は既存 caller 互換のため `anyhow::Result<String>` wrapper を残し、内部では `run_structured` を呼ぶ。
- dev server 系 command は verify では拒否し、agent action の Bash では timeout前提の通常 command として扱う。

### 追加テスト

- `bash_blocks_destructive_filesystem_command`
- `bash_blocks_credential_exfiltration_command`
- `bash_blocks_network_pipe_to_shell`
- `bash_timeout_returns_structured_outcome`
- `bash_timeout_kills_child_process_group_unix_only`
- `bash_timeout_non_unix_returns_timeout_without_cleanup_assertion`
- `bash_cancel_returns_structured_outcome`
- `bash_large_stdout_is_truncated`
- `bash_test_output_extracts_failure_summary`
- `verify_timeout_is_command_failure_with_timeout_kind`
- `run_checked_wrapper_preserves_existing_error_contract`

### 受け入れ条件

- long-running command が test suite を止めない。
- timeout/cancel 後に子プロセスが残らない。
- Bash tool result が巨大出力で context を破壊しない。
- eval failure classification が timeout/cancel/block を区別できる。
- TUI ESC と non-TUI timeout が同じ structured outcome API で扱える。

## Phase R5: Final前 Import Scanner と Compaction保護

### 対象SG

- SG-07: frontend relative import scanner
- SG-31: compaction evidence protection

### 対象ファイル

- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/minimal_loop/feedback.rs`
- `mvp/anvilminimal/src/minimal_loop/compact.rs`
- `mvp/anvilminimal/src/state.rs`

### 実装方針

- no-tool final の前に source import scan を走らせる。
- scan 対象は今回の turn で作成/編集された source file と required artifact 配下に限定する。workspace全体を毎回scanしない。
- 対象拡張子:
  - `.js`
  - `.jsx`
  - `.ts`
  - `.tsx`
- relative import だけを見る。
  - `./`
  - `../`
- 対応構文:
  - `import ... from "..."`
  - `export ... from "..."`
  - `import("...")`
  - `require("...")`
  - CSS side-effect import
- 解決候補:
  - exact
  - `.ts`, `.tsx`, `.js`, `.jsx`
  - `.json`
  - `.css`
  - `/index.tsx` 等
- package import、`@/` alias、URL import は R5 では対象外とし、profile lint 側で扱う。
- missing import がある場合 final assistant message を破棄し、feedback を次 request に入れる。
- compaction は削除優先度を変える。
  - system は保持
  - latest user は保持
  - pending feedback は保持しない
  - 直近 Read result は保持
  - 直近 Edit error / Bash failure は保持
  - tool-call assistant preamble は引き続き request時に除外
- pending feedback は session history に永続化しない現行方針を維持し、compaction保護対象に含めない。
- compaction は token概算/文字数上限のどちらでも同じ優先度規則を使う。

### 追加テスト

- `missing_relative_import_gets_repair_prompt_before_final`
- `relative_import_scanner_resolves_tsx_extension`
- `relative_import_scanner_resolves_index_file`
- `relative_import_scanner_ignores_package_imports`
- `relative_import_scanner_ignores_type_only_external_imports`
- `relative_import_scanner_checks_relative_type_imports`
- `relative_import_scanner_resolves_css_and_json_imports`
- `relative_import_scanner_scans_changed_files_only`
- `compaction_preserves_latest_user_message`
- `compaction_preserves_recent_read_evidence`
- `compaction_preserves_recent_edit_error`
- `compaction_drops_old_chat_before_evidence`

### 受け入れ条件

- Next.js/TS/JS task で import 先未作成のまま成功しない。
- compaction 後も修復に必要な Read/Edit evidence が残る。
- import scanner の誤検出で package import や alias import を壊さない。

## Phase R6: Step Cap と Progress-aware Repair

### 対象SG

- SG-18: step / repair max iteration cap
- SG-19: progress-aware bounded repair
- SG-20: repair exhausted report

### 対象ファイル

- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/repair.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/config.rs`
- `mvp/anvilminimal/src/cli.rs`

### 実装方針

- step実行とrepair実行で loop config を分ける。
  - default step cap: 8
  - default repair cap: 6
  - CLI/envで override 可能にする場合は互換性を壊さない hidden/advanced option とする。
- `run_session` の結果に stop reason と changed files を返す lightweight result API を追加する。
  - 既存 public API は wrapper で維持する。
- 新規内部API名は `run_session_with_outcome` とし、戻り値は `RunSessionOutcome { final_text, stop_reason, changed_paths, iterations, tool_calls }` とする。
- 既存 `run_session*` 系は `RunSessionOutcome.final_text` を返す wrapper として維持し、CLI/TUI 操作性を変えない。
- initial turn / repair turn の Write/Edit path を収集する。
- changed path 収集は tool call arguments だけでなく、tool実行成功後の正規化済み workspace-relative path を使う。
- repair は最大 turn数と file-changing repair数で bounded にする。
- repair turn が non-file-changing でも verify failure が改善した場合は progress とみなす。
- missing expected paths が減っていない場合、repair prompt に progress warning を入れる。
- repair exhausted report に以下を出す。
  - missing expected paths
  - verification failures
  - changed files
  - repeated changed files
  - initial stop reason
  - repair stop reason
  - suggested replan command

### 追加テスト

- `step_loop_uses_step_iteration_cap`
- `repair_loop_uses_repair_iteration_cap`
- `repair_without_missing_path_progress_gets_warning`
- `repair_repeated_write_path_is_reported`
- `repair_exhausted_report_contains_missing_paths`
- `repair_exhausted_report_contains_changed_files`
- `repair_exhausted_report_contains_suggested_replan`
- `plan_run_summary_contains_step_stop_reason`
- `run_session_string_wrapper_preserves_existing_cli_behavior`
- `changed_paths_are_workspace_relative_after_tool_success`
- `repair_verify_progress_counts_even_without_file_change`

### 受け入れ条件

- 1 step が全体 `max_iterations` まで引き伸ばされない。
- repair が進捗なしの場合、同じ失敗を繰り返さず診断情報を残す。
- eval が repair failure を具体分類できる。
- 既存 CLI/TUI の戻り値・表示文言が不要に変わらない。

## Phase R7: Profile Contract 補完

### 対象SG

- SG-25: Next.js Tailwind/rootDir/script weakening contract
- SG-34: data profile raw/input data protection

### 対象ファイル

- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `mvp/anvilminimal/src/planner/profiles/data.rs`
- `mvp/anvilminimal/src/planner/profile.rs`
- `mvp/anvilminimal/src/planner/runner.rs`

### 実装方針

#### Next.js

- Tailwind を使う場合は必要な toolchain を検査する。
  - `tailwindcss`
  - `postcss`
  - config file
  - globals css import
- `tsconfig.rootDir` が `src` などに固定されて Next.js 生成物を壊す場合は拒否する。
- `@/*` alias は `paths` と `baseUrl` の整合性を確認する。
- `scripts.build` / `scripts.dev` の弱体化を検出する。
  - `echo`
  - `true`
  - 空 script
  - `next dev` で port 指定なし when goal requires 3011

#### Data profile

- phase開始時に raw/input data snapshot を取る。
- protected path:
  - `data/raw/**`
  - `input/**`
  - scenarioで明示された dataset path
- phase後に削除・サイズ変更・hash変更を検出する。
- data profile の output は `reports/`, `output/`, `derived/` 等へ誘導する。
- profile lifecycle hook を追加する。
  - `before_phase(root, profile, goal) -> ProfileSnapshot`
  - `after_phase(root, profile, goal, snapshot) -> VerificationReport`
- snapshot は runtime memory 上で保持し、必要に応じて `.anvil/profile-snapshots/` に redacted summary を保存する。raw data 内容そのものは保存しない。
- 大きな入力ファイルは full hash を基本にし、サイズ上限を超える場合でも path/size/mtime/hash prefix を保存して削除・改変検出を維持する。

### 追加テスト

- `nextjs_rejects_script_weakening`
- `nextjs_rejects_missing_tailwind_toolchain_when_tailwind_used`
- `nextjs_rejects_tsconfig_rootdir_that_breaks_next`
- `nextjs_rejects_alias_without_baseurl_or_paths`
- `nextjs_accepts_complete_tailwind_app`
- `data_profile_snapshots_raw_inputs`
- `data_profile_rejects_raw_input_deletion`
- `data_profile_rejects_raw_input_hash_change`
- `data_profile_allows_derived_output_changes`
- `ultra_phase_profile_snapshot_runs_before_and_after_phase`
- `data_profile_snapshot_does_not_copy_raw_data_contents`
- `data_profile_detects_large_file_size_change`
- `profile_lifecycle_hooks_run_for_each_ultra_phase`

### 受け入れ条件

- Next.js profile が build不能な構成弱体化を profile verify で止める。
- data profile で raw/input data を破壊しても成功扱いしない。
- profile snapshot が raw data 内容を `.anvil` に複製しない。

## Phase R8: 最終 Eval / Traceability / DoD

### 対象SG

- 残SGすべて

### 対象ファイル

- `mvp/anvilminimal/tests/safety_parity_traceability.rs`
- `mvp/anvilminimal/tests/eval/**`
- `mvp/anvilminimal/scripts/eval_lib/**`
- `workspace/mvp/eval/002/*.md`

### 作業

- `safety_parity_traceability.rs` から対象SGの `defer:` を全て削除し、実テスト名へ置換する。
- `safety_traceability_has_no_defer_after_remaining_sg_completion` を追加し、対象SGの `defer:` が1件でも残れば default test suite が失敗するようにする。
- eval failure kind に以下を追加/確認する。
  - `relative_import_missing`
  - `edit_recoverable_error`
  - `workspace_policy_blocked`
  - `bash_timeout`
  - `bash_cancelled`
  - `repair_exhausted`
  - `profile_contract_failed`
  - `data_input_modified`
- deterministic eval fixture を追加する。
  - missing import
  - edit anchor mismatch
  - workspace metadata read attempt
  - bash timeout
  - repair no progress
  - Next.js script weakening
  - data raw input mutation
- SG別 fixture matrix を作る。
  - SG-07: missing relative import
  - SG-08/36: edit mismatch / already-applied / normalized fallback
  - SG-11/35: metadata read / node_modules glob / large grep
  - SG-14/15/16/21: typed plan / semantic lint / aggregated verify
  - SG-18/19/20: step cap / repair no progress / exhausted report
  - SG-25: Next.js script weakening / Tailwind config drift
  - SG-31: compaction evidence preservation
  - SG-32/38: bash timeout / large output summary
  - SG-34: raw data mutation
- speed-cloud eval は local LLM なしで実行する。
- local LLM eval は別枠で非ブロッキングにする。
- eval report に SG別 pass/fail を出し、どのSGが未検証かを `unknown` として残さない。

### 最終受け入れ条件

- `mvp/anvilminimal/tests/safety_parity_traceability.rs` に `defer:` が存在しない。
- `safety_traceability_has_no_defer_after_remaining_sg_completion` が default test suite で実行される。
- 対象SGすべてに実テストがある。
- `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
- `cargo clippy --manifest-path mvp/anvilminimal/Cargo.toml --all-targets -- -D warnings`
- `python3 -m unittest discover -s mvp/anvilminimal/tests/eval`
- `python3 scripts/eval-run.py --suite eval/suites/mvp-smoke.yaml --model-profile speed-cloud --modes minimal-loop,step-plan,plan-run,ultra-plan-run --runs 1 --run-root <tmp> --dry-run`
- deterministic fake eval で次を満たす。
  - `unclassified_process_failure=0`
  - `max_iterations=0`
  - SG別 fixture pass 100%
  - SG別 `unknown` 0件
  - required artifact postcheck pass 100%
  - profile contract failure は分類済み
  - repair exhausted は分類済み
- `workspace/mvp/eval/002/minimal_source_safeguard_phase0_7_implementation_report.md` を更新し、前回 `defer` としたSGがすべて完了済みへ移ったことを明記する。
- ignored live provider tests は別途 `.env` の `OPENAI_API_KEY` / `GEMINI_API_KEY` がある環境で filter付き実行する。
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test live_provider live_openai -- --ignored`
  - `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test live_provider live_gemini -- --ignored`

## 推奨コミット分割

| Commit | 内容 |
|---|---|
| 1 | R0/R1: typed plan + verify aggregation |
| 2 | R2/R3: workspace policy + Edit fallback |
| 3 | R4: Bash safety + output shaping |
| 4 | R5: import scanner + compaction |
| 5 | R6: bounded repair + repair report |
| 6 | R7: Next.js/data profile contract |
| 7 | R8: eval/traceability cleanup |

## 実装順の理由

1. Plan/Verify の型を先に固めないと、repair report と eval scoring が二重改修になる。
2. Workspace policy と tool output shaping を先に入れると、後続の loop/repair テストが安定する。
3. Bash timeout/cancel は test runtime に影響するため、repair stress test より前に入れる。
4. Repair は loop result / changed files / verification aggregation に依存する。
5. Profile contract は ultra phase verification と data snapshot に依存する。
6. 最後に traceability の `defer:` を削除し、全SG対応完了を機械的に確認する。
