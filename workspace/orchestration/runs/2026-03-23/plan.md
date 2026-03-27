# オーケストレーション実行計画

## 実行日時
2026-03-23

## 対象Issue

| Issue | タイトル | 推定影響モジュール |
|-------|---------|------------------|
| #142 | ファイル読み取りキャッシュの導入 | src/tooling/mod.rs, src/app/agentic.rs |
| #143 | file.edit エラー時のリカバリー改善 | src/tooling/mod.rs, src/app/agentic.rs |
| #144 | ANVIL_FINAL の早期発火防止 | src/agent/mod.rs |
| #145 | 反復ループの検出と自己修正 | src/agent/mod.rs, src/app/agentic.rs |
| #146 | タイムアウト設定の外部化 (--timeout) | src/config/mod.rs, src/config/cli_args.rs |
| #147 | --max-iterations のデフォルト値の見直し | src/config/mod.rs, src/agent/mod.rs |

## 依存関係分析

### 共通ファイル重複リスク
- src/tooling/mod.rs: #142, #143
- src/app/agentic.rs: #142, #143, #145
- src/agent/mod.rs: #144, #145, #147
- src/config/mod.rs: #146, #147

### 依存関係
- #146 と #147 は config/cli_args に関連するが変更箇所は異なる可能性（弱依存）
- その他は独立

### 並列実行判定
全6件を並列開発可能（worktreeで隔離、マージ時にコンフリクト解消）

## マージ推奨順序
1. #146 (config変更、基盤)
2. #147 (config変更、#146依存の可能性)
3. #142 (tooling)
4. #143 (tooling)
5. #144 (agent)
6. #145 (agent/agentic)
