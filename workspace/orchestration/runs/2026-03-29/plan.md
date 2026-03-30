# オーケストレーション実行計画

## 実行日時
2026-03-29

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #193 | feat: ANVIL.mdカスタムツール登録機能（toolsセクション） | FEATURE |

## 依存関係
- 単一Issueのため依存関係なし

## 影響ファイル
- `src/config/mod.rs` — ANVIL.mdパーサー拡張
- `src/agent/mod.rs` — システムプロンプトへカスタムツール追加
- `src/agent/tag_parser.rs` — カスタムツールタグパース
- `src/tooling/mod.rs` — カスタムツール実行

## 実行フロー
1. Worktree作成 → feature/issue-193-custom-tools
2. /pm-auto-issue2dev 193 で全自動開発
3. 品質確認
4. PR作成・マージ
