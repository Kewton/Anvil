# Orchestration Plan — 2026-04-12 (Issue #359)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #359 | Phase2: balance fix_slice prompt under path ambiguity | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-359-fixslice-prompt-balance）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 359
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針（Issue 本文より）

### 即時対策 (1) 行動促進 guidance 追加
- build_fixslice_user_prompt の RULES 直後に encouragement block を追加
  - path ambiguity (./src/ vs src/) は ORIGINAL target_path を使えば OK
  - partial file context でも proposal 可
  - 不完全な proposal > proposal なし
  - reasoning ではなく FixSliceProposal JSON に集中

### 即時対策 (2) negative example の softening
- "This is WRONG" → "Avoid this pattern" へ寄せる
- model が proposal 回避に向かわないよう禁止→誘導へ

### 回帰テスト
- cycle 15 A1 が worker_observed=true に戻るか
- long reasoning loop が消えるか
- path_mismatch が増えすぎないか

## 関連ファイル
- `src/agent/subagent.rs` — fix_slice subagent prompt
- `tests/fixslice_subagent.rs` — 既存 49 件の subagent 回帰テスト
