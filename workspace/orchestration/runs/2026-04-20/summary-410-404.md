# オーケストレーション完了報告 — 2026-04-20（#410, #404）

## 対象 Issue

| Issue | タイトル | ステータス |
|---|---|---|
| #404 | harness: analyze_run.py to compute standard metrics from a run dir | 完了（PR #417 マージ済み） |
| #410 | harness: model matrix execution in bench.sh | 完了（PR #418 マージ済み） |

## 実行フェーズ結果

| Phase | 内容 | ステータス |
|---|---|---|
| 1 | 依存関係分析・実行計画 | 完了（2 issue、独立並列） |
| 2 | Worktree 準備 | 完了（2 worktree 同時） |
| 2.5 | 根本原因分析 | スキップ（bug ラベルなし） |
| 3 | 並列開発 | 完了（両 worker 並走、Sonnet 4.6 preset） |
| 4 | 設計突合 | 完了（bench.sh / ci.yml 共通改修の衝突を早期検知） |
| 5 | 品質確認 | 完了（fmt / clippy / test / shellcheck / ruff / py_compile 全 Pass） |
| 6 | PR 作成・マージ | 完了（#417 clean → #410 rebase → #418 merge） |
| 7 | UAT | スキップ（`--full` 未指定） |
| 8 | 完了報告 | 本文書 |

## コミット

| Issue | PR | 統合 SHA | Squashed Subject |
|---|---|---|---|
| #404 | #417 | `7f1a9c0` | feat(issue-404): add analyze_run.py with metric JSON output and CI smoke tests (#417) |
| #410 | #418 | `e789c99` | feat(issue-410): add model matrix execution to bench.sh with matrix-report.md (#418) |

## 成果物

### #404
- `scripts/analyze_run.py`（556 行）— 安定 JSON 出力
- `docs/metrics.md`（168 行）— 指標定義
- `tests/golden/`（full / no-meta / legacy-session / duplicate-root）— snapshot テスト
- `tests/test_analyze_run_security.py` — symlink / dotdot / oversized-file 耐性
- `tests/scripts/test_bench_smoke.sh` — CI bench smoke
- `.github/workflows/ci.yml`: Python ジョブ（ruff + snapshot + security + bench smoke）追加
- `scripts/bench.sh` に `--bench-no-debug` / `BENCH_DEBUG=0` 追加（CI で debug なしで smoke 可能に）

### #410
- `scripts/bench.sh` 拡張:
  - `validate_models_array()` — 重複モデル / slug 衝突の事前検出
  - `generate_matrix_report()` — POSIX awk で `summary.tsv` を集計し `matrix-report.md` を atomic rename 生成
  - 進捗 stderr `[model N/M | run N/M] name`
  - `on_interrupt` が Ctrl-C 時も partial report を書く
  - trap 登録位置を `cleaned_models` populate 後に移動（`set -u` クラッシュ防止）
- `.github/workflows/ci.yml`: mikefarah/yq v4.44.1 を SHA256 checksum 検証付きで install、matrix + single-model dry-run smoke ジョブ

## 品質チェック（develop HEAD = e789c99）

| チェック | 結果 |
|---|---|
| cargo fmt --check | Pass |
| cargo clippy --all-targets | Pass（警告 0） |
| cargo test --all | Pass |
| shellcheck -S warning scripts/bench.sh | Pass |
| python3 -m py_compile scripts/analyze_run.py | Pass |
| `scripts/bench.sh --help` | 想定通り（first-write / --models / --bench-no-debug 全て含む） |
| CI（PR #418 最終実行） | 6 ジョブ全 Pass（Format / Clippy / Test / Build / ShellCheck / Python） |

## Acceptance（Issue 本文基準）

### #404
| # | 内容 | 結果 |
|---|---|---|
| 1 | スクリプトが stable な JSON を吐く | Pass |
| 2 | 指標定義が docs/metrics.md に書かれている | Pass |
| 3 | 過去 run dir でも同じ出力（backward compatible） | Pass（no-meta / legacy-session fixture） |

### #410
| # | 内容 | 結果 |
|---|---|---|
| 1 | `--models 'A,B,C'` で全 model 実行 | Pass |
| 2 | `matrix-report.md` で model 別指標表 | Pass |
| 3 | 途中中断で partial 結果が残る | Pass（`on_interrupt` から `generate_matrix_report`） |

## インシデント対応

1. **`scripts/bench.sh` / `.github/workflows/ci.yml` の衝突**
   - #410 は matrix 実行、#404 は CI smoke 用に `--bench-no-debug` を追加 — 同一ファイルの別箇所を両方触ったため merge 時に conflict。
   - 対応: #404 を先にマージ → #410 worker に rebase 指示 → worker が手作業で併合・force-push → CI 再実行 → merge。
2. **CI の yq checksum 検証失敗**
   - mikefarah/yq の Release assets にある `checksums` ファイルは sha256sum 形式ではなく表形式。
   - 対応: #410 worker に修正指示 → `grep "yq_linux_amd64$" | sha256sum --check --status` という抽出方式で対応（rebase と同一 round で完了）。
3. **worker の context 逼迫**
   - 指示送信直後に `new task? /clear to save 106.2k tokens` の UI 表示が出たが、1 回 "a" を送って再開、そのまま完走。
4. **`/model sonnet` preset の効果**
   - #402 で学んだ 1M context 課金エラーは両 worker とも発生せず、最初から最後まで Sonnet 4.6 で通せた。
