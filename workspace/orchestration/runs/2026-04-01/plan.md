# オーケストレーション実行計画 2026-04-01 (Run 2)

## 対象Issue

| Issue | タイトル | 種別 | ラベル |
|-------|---------|------|--------|
| #240 | 並列ツール実行の個別進捗表示（可観測性改善） | FEATURE | enhancement |
| #241 | セッションメモ抽出によるcompaction品質の可視化（可観測性改善） | FEATURE | enhancement |
| #242 | ツール実行パターンに基づくフェーズ表示（可観測性改善） | FEATURE | enhancement |

## 影響ファイル分析

| Issue | 影響ファイル | 変更箇所 |
|-------|-------------|---------|
| #240 | `src/tooling/progress.rs`(新規), `src/spinner.rs`, `src/app/agentic.rs` | 並列実行ループ内のToolProgressEntry更新 |
| #241 | `src/session/mod.rs`, `src/app/agentic.rs` | ターン完了後のextract_session_notes()呼び出し+ログ |
| #242 | `src/app/agentic.rs` | ターンサマリーログへのphase=フィールド追加 |

## 依存関係

```
#240 (parallel progress) ──弱依存──┐
#241 (session notes)     ──弱依存──├── src/app/agentic.rs（変更箇所は異なる）
#242 (phase display)     ──弱依存──┘
```

## 並列実行判定: 完全並列（設計突合で確認）

## マージ推奨順序
1. #242（最小変更: agentic.rsのログ出力のみ）
2. #241（session/mod.rs + agentic.rs）
3. #240（tooling/progress.rs新規 + spinner.rs + agentic.rs）
