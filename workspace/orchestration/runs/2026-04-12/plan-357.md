# Orchestration Plan — 2026-04-12 (Issue #357)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #357 | Phase2: enforce FixSliceProposal contract in fix_slice prompt | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-357-fixslice-prompt-contract）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 357
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針（Issue 本文より）

### 即時対策（prompt 変更のみ）
1. Strict contract statement を prompt 冒頭に追加
   - FINAL message は ANVIL_FINAL 内の FixSliceProposal JSON object のみ許可
   - markdown / explanations / tool call JSON は final に含めると fail
2. target_path exact-match requirement を明示
   - request で渡された path を directory prefix 含め exact に再利用
   - basename-only path や prefix drop を禁止
3. Inline schema definition を prompt に埋め込む
   - target_path / line_range / replacement / rationale
4. Few-shot examples を追加
   - positive 2 件 + negative 1 件
5. Final message rules を末尾に追加

### 恒久対策
- prompt と Rust validator で schema を single source of truth 化
- 失敗例を prompt 内の負例へ継続的に反映できる仕組み

### 回帰テスト
- subagent final output を FixSliceProposal として複数回 parse し、揺れに対する耐性を確認
- A1/A2 の正常 path が regression しないこと
- prompt 長が context 65536 内に収まること

## 関連ファイル
- `src/agent/subagent.rs` — fix_slice subagent prompt 構築
- `tests/fixslice_subagent.rs` — 既存 48 件の subagent 回帰テスト
