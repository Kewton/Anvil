# Orchestration Plan — 2026-04-11 (Issue #347)

## 対象Issue

| Issue | タイトル | 種別 |
|-------|---------|------|
| #347 | Phase2: ANVIL_FIXSLICE_MAX_ITERATIONS missing from env whitelist | BUG |

## 実行計画

1. Phase 2: Worktree準備（feature/issue-347-fixslice-env-whitelist）
2. Phase 2.5: スキップ（Issue本文に詳細分析済み、単純な whitelist 漏れ）
3. Phase 3: /bug-fix 347
4. Phase 5: 品質確認
5. Phase 6: PR作成・developマージ
6. Phase 8: 完了報告

## 修正方針（Issue 本文より）

### 即時対策
- `src/config/mod.rs:503-549` の `apply_env_overrides()` の whitelist に `"ANVIL_FIXSLICE_MAX_ITERATIONS"` を 1 行追加

### 恒久対策（別 Issue で扱う可能性）
- config key / env key の mapping を単一 source of truth に寄せる
- `apply_map()` と `apply_env_overrides()` の別管理を解消

### 予防
- env override が EffectiveConfig に反映されることを確認する integration test を tunable 追加時の標準回帰テストに追加

## 背景
- PR #346 (Issue #345) で `fixslice_max_iterations` を tunable にしたが、env 経由で設定しても反映されない
- Phase 2 cycle 4 bench で `ANVIL_FIXSLICE_MAX_ITERATIONS=8` を渡しても実効 iteration budget は default 3 のまま
- `apply_map()` は parse できるが、`apply_env_overrides()` の whitelist に入っていないため process env から読まれない
