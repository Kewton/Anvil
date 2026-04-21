# オーケストレーション実行計画 2026-03-31

## 対象Issue

| Issue | タイトル | 種別 | 優先度 |
|-------|---------|------|--------|
| #219 | feat: background session memory extraction for better compaction | FEATURE | high |
| #220 | feat: richer parallel tool progress and cancellation controls | FEATURE | high |
| #221 | feat: prompt suggestion UX for likely next input | FEATURE | medium |

## 依存関係グラフ

```
#219 (session memory) ──弱依存──> #221 (prompt suggestion)
#220 (parallel tool)  ──弱依存──> #221 (prompt suggestion)
#219 ←─独立─→ #220
```

## 並列実行グループ

- **Group A (並列)**: #219, #220, #221 — 全件並列開発
- **設計突合**: Phase 4 で #219↔#221, #220↔#221 の共通ファイルをクロスチェック

## マージ推奨順序

1. #219 (session memory) — foundation
2. #220 (parallel tool progress) — contracts/tui 変更
3. #221 (prompt suggestion) — 最後（他2件の変更を取り込み）

## 共通ファイル競合マトリクス

| ファイル | #219 | #220 | #221 |
|----------|------|------|------|
| src/session/mod.rs | ✅ | - | ✅ |
| src/app/mod.rs | ✅ | - | ✅ |
| src/contracts/mod.rs | - | ✅ | ✅ |
| src/app/render.rs | - | ✅ | ✅ |
| src/tui/mod.rs | - | ✅ | ✅ |
