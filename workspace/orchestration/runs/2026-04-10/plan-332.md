# Orchestration Plan — 2026-04-10 (Issue #332)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #332 | Phase2: worker path is not reached because fix_slice escalation is too narrow | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-332-broaden-fixslice-escalation）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み）
3. Phase 3: /bug-fix 332
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 影響ファイル
- `src/app/agentic.rs`
- `src/config/mod.rs`
- `src/contracts/mod.rs`

## 修正方針（Issueより）
1. fix_slice escalation を same-path consecutive failures 以外でも発火
2. stagnation + failed mutation attempts でも worker escalation
3. 遅い repair-turn-only mutation を Phase 2 worker path として不十分とみなし早期 escalation
4. テレメトリで escalation の種類を区別
