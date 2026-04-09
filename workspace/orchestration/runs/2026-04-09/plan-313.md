# Orchestration Plan — Issue #313 (2026-04-09)

## 対象Issue
| Issue | タイトル | 種別 |
|-------|---------|------|
| #313 | bug: large-file の file.edit recovery が mid-file context を返せず shell.exec drift / partial に落ちる | BUG |

## 影響ファイル
- `src/app/agentic.rs` — recovery hint, FILE_READ_RESULT_MAX_CHARS
- `src/app/tool_recovery_budget.rs` — blessed recovery primitive
- `src/tooling/mod.rs` — extract_edit_context() fallback

## 実行計画
1. Phase 2: Worktree (feature/issue-313-edit-recovery-mid-context)
2. Phase 2.5: Opus根本原因分析
3. Phase 3: /bug-fix 313
4. Phase 5: 品質確認
5. Phase 6: PR・マージ
6. Phase 8: 完了報告
