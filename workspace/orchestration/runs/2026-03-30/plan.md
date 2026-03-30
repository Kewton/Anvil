# オーケストレーション実行計画

## 実行日時
2026-03-30

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #195 | feat: サイドカーモデルによるコンテキスト圧縮（--sidecar-model） | FEATURE |

## 依存関係
- 単一Issueのため依存関係なし

## 影響ファイル
- `src/config/mod.rs` (cli_args) — --sidecar-model CLIオプション
- `src/config/mod.rs` (EffectiveConfig) — sidecar_model設定
- `src/provider/ollama.rs` — サイドカーモデルAPI呼び出し
- `src/session/mod.rs` / `src/state/mod.rs` — compact_history LLMベース要約

## 実行フロー
1. Worktree作成 → feature/issue-195-sidecar-model
2. /pm-auto-issue2dev 195 で全自動開発
3. 品質確認
4. PR作成・マージ
