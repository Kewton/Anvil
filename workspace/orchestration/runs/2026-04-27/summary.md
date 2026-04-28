# Orchestration Summary — 2026-04-27 (Epic A 完了)

## 結論

`/orchestrate 443 450 451 452 453 454 455` を **Option A: 直列実行** で完遂。
**Epic A: Dynamic Precaution Runtime** (#444) の構成 6 Issue + 独立 bug 1 件、計 7 Issue を develop へ統合。

## 対象 Issue / PR / 統合 commit 一覧

| Phase | GitHub Issue | 種別 | PR | develop 統合 commit |
|---|---|---|---|---|
| A | #443 rustyline 14 umaskレース flaky test | bug | #474 | `4b261fb` |
| B-1 | #450 FeedbackFrame を導入する | feature | #475 | `7162b8d` |
| B-2 | #451 WorkingMemory に active_precautions を追加 | feature | #476 | `f57fc60` |
| B-3 | #452 Reminder Sidecar を実装 | feature | #477 | `200269a` |
| B-4 | #453 Act mode prompt に ACTIVE PRECAUTIONS を注入 | feature | #478 | `62c40cb` |
| B-5 | #454 /precautions コマンドを追加 | feature | #479 | `c1a5d91` |
| B-6 | #455 runtime recovery を Precaution 更新に接続 | feature | #480 | `32e39bf` |

全 PR は `--squash --delete-branch` で develop に統合、対応 Issue は close 済。

## 実行フェーズ結果

| Phase | 内容 | ステータス | 備考 |
|---|---|---|---|
| 0 | 初期設定（develop ブランチ確認、CommandMate 稼働確認） | ✅ 完了 | |
| 1 | 依存関係分析・実行計画 | ✅ 完了 | `plan.md` に記録、Track 1 の強い直列依存を検出し Option A 採用 |
| 2 | Worktree 準備 | ✅ 完了 | 各 Issue ごとに新規 worktree → 完了後 `--force` で削除 |
| 2.5 | 根本原因分析（バグ Issue #443） | ⏭️ スキップ | Issue 本文に既に詳細な根本原因分析（rustyline 14 umask race の解析、対処方針 A〜D）が記載済のため再分析を省略 |
| 3 | 並列開発（実態は Option A により直列実行） | ✅ 完了 | `/pm-auto-issue2dev` を順次送信、6 Issue × 1 worker |
| 4 | 設計突合（バリア） | ⏭️ 不要 | 直列実行のため不要 |
| 5 | 品質確認 | ✅ 完了 | 各 PR で CI green を確認 |
| 6 | PR 作成・マージ | ✅ 完了 | `gh pr create` → `gh pr checks --watch` → `gh pr merge --squash` |
| 7 | UAT | ⏭️ 未実行 | `--full` フラグ未指定 |
| 8 | 完了報告 | ✅ 完了 | 本ドキュメント |

## 統合検証結果（develop @ `32e39bf`）

| チェック項目 | 結果 |
|---|---|
| `cargo fmt --all -- --check` | ✅ clean |
| `cargo clippy --all-targets -- -D warnings` | ✅ pass (0 warnings) |
| `cargo test --all` | ✅ **767 passed / 0 failed / 8 ignored** |
| `cargo build` | ✅ pass (clippy 内で実施) |

## Anvil への影響（Epic A による runtime loop 拡張）

```
実行 (bash / auto_test / focused edit / native tool / parser)
  ↓
FeedbackFrame (#450) — 統一構造化、secret mask、excerpt cap
  ↓
record_feedback (#450) → SessionSnapshot.last_feedback (#450)
  ↓
Reminder Sidecar (#452) — JSON I/O, Reminder 専用 LLM、tool 不使用、code diff 不返却
  ↓
WorkingMemory.active_precautions (#451) — Active のみ prompt 注入、FIFO eviction、duplicate suppression
  ↓
Act mode prompt 注入 (#453) — severity / 関連 file 優先、token budget cap、Plan mode 非表示
  ↓
runtime recovery 統合 (#455) — no-progress / repeated bash / focused edit failure / unsafe block / parser failure / deterministic fallback を FeedbackFrame 化
  ↓
ユーザ操作 (#454): /precautions list/add/retire/clear で確認・修正可能
```

## 学び・知見

### 効果的だったこと

- **Option A（直列）の選択**: feature.md の Track 1 が示す通り、#1〜#6 は強い依存があり、並列実行はマージコンフリクトを多発させる。直列にしたことで衝突ゼロ
- **`--force` での worktree cleanup**: gitignored な `dev-reports/` が残るため `git worktree remove` がエラーになる。`--force` で問題なし
- **PR 作成は直接 gh CLI**: worker に `/create-pr` を依頼するより、commit 完了後に push + `gh pr create` を直接実行する方が確実かつ高速

### 想定外の挙動と対処

| 事象 | 対処 |
|---|---|
| 「API Error: Extra usage is required for 1M context」が `/pm-auto-issue2dev` の Phase 5 (TDD) などで頻発 | "a" 送信で再開 |
| worker が dev-reports は生成するが commit 前に idle に入る | 明示的な commit 指示メッセージで再起動 |
| #450 の test が CI Linux 環境で fail (locale-dependent な non-UTF-8 stdout 検証) | 修正コミット追加で対応 (`96ba850`) |
| #451 で worker が `workspace/v0.1.1/feature.md` を tracked file として commit に含めた | 結果オーライで develop に取り込み、以降の worktree でコピー不要に |
| `commandmatedev wait` が prompt 検出で exit 10 を返すが auto-yes が裏で対応する | wait をループで再起動、isProcessing で真の完了判定 |

### 次回以降の改善案

1. `/pm-auto-issue2dev` の Phase 5 で API limit が出やすいので、worker 側の `/extra-usage` を事前 enable する手順を組み込む
2. worker が untracked ファイル（個人作業ファイル）を tracked にしないよう、`.gitignore` または明示制約を渡す
3. CI locale-dependent test は最初から Linux runner で動作確認する仕組みを持つ
4. worker と直接対話する代わりに、commit/push/PR をオーケストレーター側で完結させる現在のフローは効率的なので維持

## 成果物

- 設計書: `dev-reports/design/issue-{N}-*-design-policy.md` (各 worktree、merged 済み worker artifact は揮発)
- 作業計画: `dev-reports/issue/{N}/work-plan.md` (同上)
- multi-stage-design-review: `dev-reports/issue/{N}/multi-stage-design-review/` (4 stages × 7 issues = 28 review report)
- ソースコード: `src/session/feedback.rs`, `src/session/precaution.rs`, `src/agent/loop_run/reminder.rs`, plus 多数の追加・修正
- テスト: `tests/session_tests.rs`, `tests/precaution_prompt_injection.rs`, `tests/precautions_repl.rs`, `tests/slash_commands_ssot.rs` 拡張など
- ドキュメント: `CLAUDE.md` (Reminder Sidecar の architecture 説明追加)
- このオーケストレーション計画: `workspace/orchestration/runs/2026-04-27/plan.md`
- 完了報告: `workspace/orchestration/runs/2026-04-27/summary.md`（本ドキュメント）

## Epic A 完了に伴う次の Track 候補

`workspace/v0.1.1/feature.md` の Track 整理に従い、以下が次の候補:

- **Track 2: Verification / Safety** — #456 (#7 AnvilScore), #457 (#8 auto_test integration), #458–#460 (#9–#11 temporary tests), #461 (#23 sandbox policy)
- **Track 3: Memory** — #462–#464 (#12–#14 Case Memory & CBR) ※ #12 は #1, #2, #7 安定後推奨
- **Track 4: Skills Architecture** — #465 (#15 AgentSkill), #466–#467 (#16–#17)
- **Track 5: Repo Context** — #468–#470 (#18–#20) ※ 他 Track と独立
- **Track 6: Observability / Eval** — #471 (#21 evaluation log), #472 (#22 A/B harness), #473 (#24 dataset export)
