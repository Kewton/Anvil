# Orchestration Plan — 2026-04-20（#408）

## 対象 Issue

| Issue | Title | 種別 | Priority |
|---|---|---|---|
| #408 | ux: suppress DEBUG noise by default, gate under --verbose | FEATURE | 7 |

## 依存関係

- 単独 issue、並列性なし
- 影響ファイル推定: `src/logging.rs` / `src/cli.rs` / `src/config.rs`
- llm-io.jsonl 経路は保全必須（Acceptance）

## Phase 対応

| Phase | 扱い |
|---|---|
| 1 | 本文書 |
| 2 | `feature/issue-408-log-levels` worktree |
| 2.5 | スキップ（bug ラベルなし） |
| 3 | `/pm-auto-issue2dev 408`、Sonnet 4.6 preset |
| 4 | 単独 issue のため簡易化 |
| 5 | fmt/clippy/test、実際に `anvil` 実行して DEBUG noise が消えるか確認 |
| 6 | PR 作成→merge |
| 7 | スキップ |
| 8 | summary-408.md |

## Acceptance（Issue 本文）

- [ ] default 実行で `DEBUG pooling idle connection` が出ない
- [ ] `--verbose` で tool 呼び出し詳細が見える
- [ ] `--trace` で全 debug が見える
- [ ] llm-io.jsonl は引き続き全部保存される
