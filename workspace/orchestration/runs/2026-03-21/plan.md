# オーケストレーション実行計画

## 実行日時
2026-03-21

## 対象Issue（親: #127 の子Issue 6件）

| Issue | タイトル | Wave | 依存 |
|-------|---------|------|------|
| #128 | Wave 1: multi-tier tool protocol and resilient editing | 1 | なし |
| #132 | Wave 2: prompt tiering and model adaptation for local LLMs | 2 | #128 |
| #130 | Wave 3: structured working memory and stateful compaction | 3 | なし |
| #133 | Wave 4: retrieval upgrade for large repositories | 4 | なし |
| #131 | Wave 5: shell execution policy split and offline hardening | 5 | なし |
| #129 | Wave 6: sub-agent redesign for local-model-first workflows | 6 | なし |

## 共通ファイル重複

| ファイル | 関連Issue |
|---------|----------|
| src/agent/mod.rs | #128, #132, #130 |
| src/app/mod.rs | #132, #130, #131 |
| src/tooling/mod.rs | #128, #131 |

## 並列実行グループ
- 全6件を並列開発（worktreeで隔離）
- #132は#128に明示依存あるが並列開発してマージ順序で対応

## マージ推奨順序
1. #128 (Wave 1) → 基盤
2. #132 (Wave 2) → #128依存
3. #130 (Wave 3)
4. #133 (Wave 4)
5. #131 (Wave 5)
6. #129 (Wave 6)
