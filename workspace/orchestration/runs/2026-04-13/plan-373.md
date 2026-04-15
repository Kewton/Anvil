# Orchestration Plan — 2026-04-13 (Issue #373)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #373 | feat: make native tool calling the primary provider path | FEATURE |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-373-native-tool-calling）
2. Phase 2.5: スキップ（機能Issue）
3. Phase 3: /pm-auto-issue2dev 373
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 概要

native tool calling を first-class 実行パスにする。
- provider contracts に tool definitions / tool_calls を追加
- capability-based routing（native → ANVIL text → repair fallback）
- ANVIL protocol は fallback に格下げ
- native tool calls を内部 typed form に正規化
