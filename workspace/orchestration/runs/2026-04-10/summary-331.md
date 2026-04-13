## オーケストレーション完了報告 — Issue #331（リカバリー含む）

### 結果サマリー

| 項目 | 内容 |
|------|------|
| 元の Issue | Kewton/Anvil#331 (誤登録) |
| 正規化後 | Kewton/localllm-test#3 |
| 修正 PR | Kewton/localllm-test#2 (commit `762c3a6`) |
| 前提 PR | Kewton/localllm-test#1 (commit `4c91280`) |

### 発生した問題

1. Issue #331 が誤って Kewton/Anvil に登録された（修正対象は localllm-test 配下）
2. `/orchestrate 331` が Anvil-develop で実行された
3. ワーカー (claude) が越境して localllm-test を編集
4. ワーカーが localllm-test の main に直接 commit (`dee1192`) - feature branch を経由せず

### リカバリー経緯

| Step | 内容 | 結果 |
|------|------|------|
| 1 | localllm-test の状態確認・依存関係解析 | `dee1192` は `834fe9f` の上に依存していることを発見 |
| 2 | バックアップ作成 (`backup/dee1192`, `backup/834fe9f`) + stash | 完了 |
| 3 | PR1 用 feature branch (`feature/benchmark-source-smoke-workflow`) を `834fe9f` から作成 | 完了 |
| 4 | localllm-test main を `origin/main` (= `f6c293b`) まで巻き戻し | 完了 |
| 5 | PR1 push → 作成 → マージ | Kewton/localllm-test#1 (`4c91280`) |
| 6 | PR2 用 feature branch (`feature/issue-331-bench-expectation-wiring`) を main から作成 | 完了 |
| 7 | `dee1192` を cherry-pick (新コミット `3201374`) | clean に成功 |
| 8 | PR2 push → 作成 → マージ | Kewton/localllm-test#2 (`762c3a6`) |
| 9 | localllm-test 一時ブランチ削除 + stash pop | 完了 |
| 10 | Anvil worktree (`Anvil-feature-issue-331-...`) と branch 削除 | 完了 |
| 11 | CommandMate 同期 | 完了 (`deleted=1`) |
| 12 | Kewton/localllm-test#3 として正規 Issue 新規登録 | 完了 |
| 13 | Kewton/Anvil#331 に経緯コメント追加 (PR #2 マージで自動 close 済み) | 完了 |

### 変更内容（PR2 = Kewton/localllm-test#2）

- `anvil_test/packs/autonomy-v2-p2-fix-slice-v1/manifest.json` (新規作成, +28)
- `commandindextest/scripts/run_issue193_benchmark.py` (+201/-5)
- `commandindextest/scripts/test_run_issue193_benchmark.py` (新規作成, +260)

### 教訓

1. **Issue 登録時のリポジトリ判定**: 修正対象ファイルが属するリポジトリに登録する
   - Anvil コード変更 → Kewton/Anvil
   - bench infrastructure 変更 → Kewton/localllm-test
2. **オーケストレーター実行ディレクトリ**: 対象リポジトリのトップで実行する
3. **ワーカー安全装置**: worktree 外への編集を禁止すべき (将来の改善項目)
4. **直 commit 禁止**: feature branch + PR フローを徹底
