# Orchestration Summary — 2026-04-28 / 2026-04-29 (Epic B 完了)

## 結論

`/orchestrate 456 457 458 459 460 461` を **Option B: 段階的並列** で完遂。
**Epic B: Verification & Temporary Testing** (#445) の構成 6 Issue を develop へ統合。

## 対象 Issue / PR / 統合 commit 一覧

| サブトラック | GitHub Issue | サブトラック内位置 | PR | develop 統合 commit |
|---|---|---|---|---|
| 2a | #456 AnvilScore | 1/2 | #484 | `782e121` |
| 2a | #457 auto_test → AnvilScore 接続 | 2/2 | #487 | `31f47ba` |
| 2b | #458 Temporary Test Workspace | 1/3 | #485 | `0ccd760` |
| 2b | #459 Tester Skill v1 | 2/3 | #486 | `7d41192` |
| 2b | #460 /tests promote/discard | 3/3 | #488 | `28825fa` |
| 2c | #461 sandbox policy (Phase A) | 1/1 | #483 | `73c2c18` |

全 PR は `--squash --delete-branch` で develop に統合、対応 Issue は close 済。

## 実行フェーズ結果

| Phase | 内容 | ステータス | 備考 |
|---|---|---|---|
| 0 | 初期設定 | ✅ 完了 | develop ブランチ確認、CommandMate 稼働確認 |
| 1 | 依存関係分析・実行計画 | ✅ 完了 | `plan.md` に記録、3 サブトラック構造を識別 |
| 2 | Worktree 準備 | ✅ 完了 | Step 1 で 3 worktree 並列作成、Step 2-4 で順次追加 |
| 3 | 並列開発 | ✅ 完了 | Step 1 (#456 + #458 + #461) → Step 2/3 重複 (#457 + #459) → Step 4 (#460) |
| 4 | 設計突合 | ⏭️ 部分実施 | Option B では暗黙、merge 時の conflict 解消で代替 |
| 5 | 品質確認 | ✅ 完了 | 各 PR で CI green を確認 |
| 6 | PR 作成・マージ | ✅ 完了 | 6 PR 全て squash merge |
| 7 | UAT | ⏭️ 未実行 | `--full` フラグ未指定 |
| 8 | 完了報告 | ✅ 完了 | 本ドキュメント |

## 統合検証結果（develop @ `28825fa`）

| チェック項目 | 結果 |
|---|---|
| `cargo fmt --all -- --check` | ✅ clean |
| `cargo clippy --all-targets -- -D warnings` | ✅ pass (0 warnings) |
| `cargo test --all` | ✅ **1054 passed / 0 failed / 8 ignored** |
| `cargo build --release` | ✅ 8.6 MiB (`target/release/anvil`) |

## Anvil への影響（Epic B による検証パス拡張）

```
auto_test (#457) → AutoTestResult
  ↓ build_anvil_test_summary
AnvilScore (#456) → SessionSnapshot.last_anvil_score
  ↓
Reminder Sidecar (#452) ← anvil_score として注入

Tester Skill (#459) → tmp-tests/files/<rel>/smoke.rs (Tester Workspace #458)
  ↓ Bash 実行
FeedbackFrame ← TestPass/TestFailure 記録 (#450)

/tests slash command (#460) → list/promote/discard
  ↓ promote
Write approval → repo path

sandbox policy (#461 Phase A):
  - bash::check_blocked_command (SSoT) — 全 destructive command を SSoT で判定
  - registry::preflight_bash_command — execute / execute_bash_with_outcome 共通の preflight
  - logging::mask_payload_inplace — secret-like key を recursive mask
  - DR4-004: rendered block reason を primary_error に入れ raw command を遮蔽
```

## Option B の評価

### 効果的だったこと

- **Step 1 で 3 並列起動**: #456 + #458 + #461 を同時開発で時間短縮
- **依存解消即時起動**: #456 merge → #457 開始、#459 merge → #460 開始 のように依存解消を検知して即座に次着手
- **#461 (sandbox) の独立性**: 他と完全独立だったので merge 順を後ろ回しにしても問題なし
- **Python 並列待機ループ**: bash 互換問題を回避し、複数 worker を同時に追跡

### 想定外の挙動と対処

| 事象 | 対処 |
|---|---|
| 並列 3 worker 同時起動で API Error 頻発 | 各 worker に "a" 送信で個別再開、最大 5 回まで escalation |
| #461 (sandbox) と #458 (tmp_tests) が `src/tools/registry.rs` で衝突 | 手動 merge resolve、両方の test 群を統合 |
| #461 が #459 マージ後にも再度 conflict | bash.rs の `run_with_outcome` シグネチャ変化（#459 の Tester で 6 引数化）に追随 |
| #459 worker session が途中で消失 | git worktree 状態は保持、CI から PR 直接マージで対応 |
| #456 で CI failure (`Cargo.lock` v4 + lossy lockfile) | develop 側の修正コミット (`440d644`) を merge して取り込み |
| #456 で test failure (`non_utf8_stdout`) | locale-dependent な Linux CI 失敗、worker に修正依頼で `96ba850` 追加 |
| #486 (#459) で Clippy failure (`unnecessary_sort_by`) | `.sort_by` → `.sort_by_key` に書き換える修正コミットで対応 |
| #460 worker が Issue review stage 6 で停止 | 明示的な「Phase 5 へ skip して実装に直行」指示で完走 |
| Bash の `for wt in $WTS` が zsh 環境で word-splitting されない | Python スクリプトに切り替えて並列待機を実装 |

### 比較

- **Option A (Epic A)**: 7 サイクル直列、衝突ゼロ、所要 ~半日
- **Option B (Epic B)**: 4 サイクル相当、conflict 解消 4 回、所要 ~1 日（API Error の頻度が高くなった分）

理論上 Option B のほうが効率的だが、API Error の頻発で実時間差は小さい。一方、conflict 解消の手戻りは確実に発生するため、ファイル衝突可能性が低い Issue 群に限定して採用すべき。

## 成果物

- 設計書: `dev-reports/design/issue-{N}-*-design-policy.md` (各 worktree、merged 済み worker artifact は揮発)
- 作業計画: `dev-reports/issue/{N}/work-plan.md`
- multi-stage-design-review: `dev-reports/issue/{N}/multi-stage-design-review/` (4 stages × 6 issues)
- ソースコード:
  - `src/session/anvil_score.rs` (#456 NEW)
  - `src/session/tmp_tests.rs` (#458 NEW)
  - `src/agent/loop_run/tester.rs` (#459 NEW)
  - `src/util/file_classify.rs` (#456 NEW)
  - `src/agent/loop_run/auto_test.rs` 拡張 (#457)
  - `src/agent/loop_run/turn.rs` 拡張 (#456 / #457)
  - `src/agent/loop_run/commands.rs` 拡張 (#460 /tests dispatcher)
  - `src/tools/bash.rs` 強化 (#461)
  - `src/tools/registry.rs` 強化 (#458 routing + #459 confinement + #461 preflight)
  - `src/logging.rs` 強化 (#461 secret mask)
- テスト: `tests/tmp_tests_e2e.rs`, `tests/tester_skill_smoke.rs`, `tests/tests_slash_command.rs`, etc.
- ドキュメント: `CLAUDE.md` 更新 (各 module の説明追加)
- 計画/報告: `workspace/orchestration/runs/2026-04-28/{plan,summary}.md`

## Epic B 完了に伴う次の Track 候補

`workspace/v0.1.1/feature.md` に従い、以下が次の候補:

- **Track 3: Memory** — #462 (#12 CaseRecord), #463 (#13 retrieval), #464 (#14 anti-pattern)
- **Track 4: Skills Architecture** — #465 (#15 AgentSkill), #466 (#16 skill registry), #467 (#17 trust tier)
- **Track 5: Repo Context** — #468–#470 (#18–#20、他と独立)
- **Track 6: Observability / Eval** — #471 (#21 evaluation log), #472 (#22 A/B harness), #473 (#24 dataset export)

Track 3 と Track 4 は相互に独立でなく、Track 4 の AgentSkill が固まれば Reminder/Verifier/Tester が再整理可能。

main へのリリースは Epic A の時のように develop → main PR を切ることで実施可能（前回は #481）。
