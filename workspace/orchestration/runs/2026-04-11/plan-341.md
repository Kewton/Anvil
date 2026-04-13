# Orchestration Plan — 2026-04-11 (Issue #341)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #341 | Phase2: agent.fix_slice worker receives insufficient target context and falls back to parent-side file.edit | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-341-fixslice-target-context）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 341
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針
1. `ToolInput::AgentFixSlice` ハンドラで subagent へ渡す user prompt に `target_path` と `max_lines` を明示的に埋め込む
2. FixSlice 専用の structured instruction block 化
3. target file read の先頭実行を prompt で示唆
4. proposal の target_path が input と一致することを保証
5. 回帰テスト:
   - subagent 初回 prompt に target_path が含まれる
   - prompt construction test
   - positive: worker success path
   - negative: parent-side salvage only
