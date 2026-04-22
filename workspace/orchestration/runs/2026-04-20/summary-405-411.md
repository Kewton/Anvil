# オーケストレーション完了報告 — 2026-04-20（#405, #411）

## 対象 Issue

| Issue | タイトル | ステータス |
|---|---|---|
| #405 | harness: 5-run aggregation to markdown report | 完了（PR #419 マージ済み） |
| #411 | harness: A/B comparison script for experiment evaluation | 完了（PR #420 マージ済み） |

## 実行フェーズ結果

| Phase | 内容 | ステータス |
|---|---|---|
| 1 | 依存関係分析・実行計画 | 完了（2 issue、ci.yml のみ弱依存） |
| 2 | Worktree 準備 | 完了（2 worktree 同時） |
| 2.5 | 根本原因分析 | スキップ（bug ラベルなし） |
| 3 | 並列開発 | 完了（Sonnet 4.6 preset、両 worker 並走） |
| 4 | 設計突合 | 完了（事前 merge-test で ci.yml 衝突を確認） |
| 5 | 品質確認 | 完了（fmt/clippy/test + ruff + py_compile 全 Pass） |
| 6 | PR 作成・マージ | 完了（#419 clean → #420 rebase → merge） |
| 7 | UAT | スキップ |
| 8 | 完了報告 | 本文書 |

## コミット

| Issue | PR | 統合 SHA | Subject |
|---|---|---|---|
| #405 | #419 | `9603140` | feat(issue-405): add report.py for 5-run aggregation to markdown (#419) |
| #411 | #420 | `430516b` | feat(issue-411): add compare.py for A/B run set diff with JSON option (#420) |

## 成果物

### #405 (`scripts/report.py`)
- BENCH_ROOT 配下の全 run を `analyze_run.py` で分析 → GFM Markdown 生成
- `--compare A/ B/` で A/B 差分 markdown
- CV > 0.3 警告、null は N/A 表示・集計除外、tool_calls 名の Markdown エスケープ
- symlink 耐性（BENCH_ROOT は exit 1、model/run-dir はスキップ）
- snapshot 8 + security 10 + E2E 互換テスト、golden fixtures 6 種
- README.md にベンチマークセクション追加

### #411 (`scripts/compare.py`)
- 2 run set → 1-page diff markdown（improvement / regression / equivalent verdict）
- `--json` オプションで機械可読出力
- `docs/compare.md` に metric 比較方法論（threshold、信頼区間メモ）
- snapshot + security + smoke テスト

### 両方の共有
- `.github/workflows/ci.yml` Python ジョブ拡張（report / compare 各ステップ）

## 品質チェック（develop HEAD = 430516b）

| チェック | 結果 |
|---|---|
| cargo fmt --check | Pass |
| cargo clippy --all-targets | Pass（警告 0） |
| cargo test --all | Pass |
| python3 -m py_compile scripts/{report,compare,analyze_run}.py | Pass |
| ruff check scripts/*.py | Pass |
| CI（#420 最終実行） | 6 ジョブ全 Pass（Format / Clippy / Test / Build / ShellCheck / Python） |

## Acceptance

### #405
| # | 内容 | 結果 |
|---|---|---|
| 1 | markdown 表として正常レンダリング | Pass |
| 2 | analyze_run.py の JSON を入力に使う | Pass |
| 3 | `--compare A/ B/` で差分 markdown | Pass |

### #411
| # | 内容 | 結果 |
|---|---|---|
| 1 | 2 つの dir を食わせて 1 ページの markdown | Pass |
| 2 | 重要 metric の改善率を符号で示す | Pass（improvement/regression/equivalent） |
| 3 | JSON 出力オプション | Pass（`--json`） |

## インシデント対応

1. **ci.yml の衝突**（想定内）
   - 両 worker とも Python ジョブに新ステップ追加 → 事前 merge-test で検出、#419 先 merge → #411 worker に rebase 指示 → 1-2 分で force-push 完了。
2. **rebase 所要時間の改善**
   - 前回（#410）は worker 側 context 逼迫で 40+ 分かかったが、今回 #411 は 2 分で完了。工程慣れ or context 余裕の差。
3. **設計責務の明確化**
   - plan-405-411.md で「#405 の `--compare` は副次、#411 の compare.py が主」と事前整理 → 両 PR とも重複なく完走。

## ベンチマーク ハーネスの完成度

Priority 順に #402 / #403 / #404 / #405 / #410 / #411 が全て develop にマージ完了。

- #402: logs/sessions/plans を XDG state 配下へ永続化
- #403: bench.sh 基盤（smoke / 5-run / matrix）
- #404: analyze_run.py（metric JSON）
- #405: report.py（5-run aggregation + A/B 差分 markdown）
- #410: bench.sh matrix-report + yq SHA256 検証
- #411: compare.py（A/B 差分 markdown + JSON 出力）

次回からのベンチマーク実験は `bench.sh` → `analyze_run.py` → `report.py` / `compare.py` の一気通貫で自動化可能。
