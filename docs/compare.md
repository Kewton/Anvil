# anvil bench compare

`scripts/compare.py` は 2 つの `scripts/bench.sh` 実行結果（BENCH_ROOT）を比較し、
メトリクスの差分を Markdown / JSON で出力する CLI ツール。

## 1. 使い方

```
scripts/compare.py [-h] [--metric METRIC[,METRIC...]]
                   [--format {markdown,json}]
                   [--threshold FLOAT]
                   baseline_dir experiment_dir
```

- `baseline_dir` / `experiment_dir`: `bench.sh` が生成した `<timestamp>-<pid>` ディレクトリ
- `--metric`: 集計対象キー（省略時は全キー）。カンマ区切り、エイリアス可
- `--format`: `markdown`（既定）または `json`
- `--threshold`: 改善/悪化判定の閾値（既定 `0.05` = 5% または 5pt）

### 例

```bash
# デフォルト (markdown, 全メトリクス)
scripts/compare.py .anvil/benchmarks/20260101-12345 .anvil/benchmarks/20260101-12400

# 特定メトリクスだけを JSON で
scripts/compare.py --metric rc,elapsed_s --format json baseline/ experiment/

# しきい値を 10% に緩める
scripts/compare.py --threshold 0.10 baseline/ experiment/

# PAM advisory A/B 用の固定 suite を同一 prompt set で実行
scripts/bench.sh pam-ab-general --model qwen3.5:122b --runs 5 --pam-ab
```

出力は stdout、警告は stderr。`--output` は v1 で提供しない。

`--pam-ab` は各 benchmark case を `pam_on` /
`pam_off` の 2 variant で実行し、`summary.tsv` に `case` と
`pam_variant` を記録する。PAM 採用 context と suppression 理由は
各 run の `logs/eval.jsonl` / `logs/llm-io.jsonl` から追跡する。
`scripts/report.py` は `task_kind` 別に terminal success (`rc==0`) と
artifact-level postcheck (`postcheck_success`) を分離集計し、PAM variant 別の
比較表も出力する。各表には Anvil 判定と postcheck の
`true_positive` / `false_positive` / `false_negative` / `true_negative`
件数を含め、`--format json` では同じ taxonomy を機械可読 summary として
出力する。

## 2. 期待する入力レイアウト

```
<baseline_dir>/
└── <model_slug>/
    ├── run-1/
    │   ├── session.json
    │   ├── meta.json
    │   └── logs/llm-io.jsonl
    ├── run-2/
    └── ...
```

suite/PAM run の場合は以下の入れ子 layout もサポートする。

```
<baseline_dir>/
└── <model_slug>/
    └── <case_slug>/
        └── <pam_variant>/
            ├── run-1/
            ├── run-2/
            └── ...
```

- `model_slug` ディレクトリは各 root 直下に 1 個のみ（複数/0 個は exit 1）
- `baseline` と `experiment` の `model_slug` は一致必須（不一致は exit 1）
- `run-*` はシンボリックリンクならスキップ

## 3. メトリクスエイリアス

| alias | 正式キー |
|---|---|
| `rc0` | `rc` |
| `postcheck` | `postcheck_success` |
| `page_game` | `page_tsx_has_game_keywords` |

正式キーは `analyze_run.py` 出力のキー。現時点で対応しているもの:

| 正式キー | 集計種別 | 改善方向 |
|---|---|---|
| `rc` | bool_rate (rc==0 を成功) | up |
| `postcheck_success` | bool_rate | up |
| `page_tsx_has_game_keywords` | bool_rate | up |
| `we_total` | informational | - |
| `elapsed_s` | continuous | down |
| `iter_count` | continuous | down |
| `error_500_count` | continuous | down |

## 4. 判定 (verdict) 規則

| metric_type | 判定ロジック | 絵文字 |
|---|---|---|
| bool_rate (up) | `delta > threshold` → `improved` / `delta < -threshold` → `regressed` | ✅/❌ |
| bool_rate (down) | `delta < -threshold` → `improved` / `delta > threshold` → `regressed` | ✅/❌ |
| continuous (up) | `delta_pct > threshold` → `improved` / `delta_pct < -threshold` → `regressed` | ✅/❌ |
| continuous (down) | `delta_pct < -threshold` → `improved` / `delta_pct > threshold` → `regressed` | ✅/❌ |
| informational | `delta != 0` → `informational` / `delta == 0` → `unchanged` | ℹ️/➖ |
| baseline mean が 0 かつ directional | `delta` の符号のみで判定、`delta_pct = null` | 同上 |

信頼区間:

- continuous: t 分布 95%CI（df = n-1）
- bool_rate: Wilson 95%CI（z=1.96）

## 5. JSON 出力スキーマ (v1)

```json
{
  "schema_version": 1,
  "baseline_dir": "/path/to/baseline",
  "experiment_dir": "/path/to/experiment",
  "model_slug": "qwen3:8b",
  "generated_at": "2026-04-20T15:00:00Z",
  "threshold": 0.05,
  "metrics": {
    "rc": {
      "baseline":   {"mean": 0.8, "ci_low": 0.49, "ci_high": 0.94, "n": 5},
      "experiment": {"mean": 1.0, "ci_low": 0.57, "ci_high": 1.00, "n": 5},
      "delta": 0.2,
      "delta_pct": null,
      "verdict": "improved"
    }
  }
}
```

- `metrics` のキー: 正式メトリクスキー（エイリアスは展開済み）
- 個別 metric 内の `key` / `metric_type` は JSON 非出力（内部管理専用）
- `null` は「計測不能」を意味する（NaN / Inf は JSON では `null`）

## 6. exit code

| exit code | 意味 |
|---|---|
| 0 | 成功 |
| 1 | 引数 / セキュリティ / shape エラー（symlink / model_slug 不一致 / MAX_RUNS 超過 / 未知メトリクス） |
| 2 | ディレクトリ不在 / run-dir 0 件 |
| 3 | 有効 run 数が `MIN_VALID_RUNS` (=3) 未満（rc==130 除外後） |

`analyze_run.py` が returncode != 0 や JSON パース失敗した run は warning（stderr）を出しつつ除外する。

## 7. セキュリティ

- 入力パスは `Path.resolve(strict=True)` 後に top-level symlink を拒否
- `scripts/analyze_run.py` は compare.py と同一 repo の regular file のみ許可
- `subprocess.run` 呼び出しは `shell=False`、`PYTHONPATH` 等を遮断した env、`-I` で isolated mode
- `run-*` は `MAX_RUNS = 100` で上限、超過は exit 1
- `os.umask(0o077)` を `main` の先頭で設定

## 8. バージョン管理

- `SCHEMA_VERSION = 1`
- 破壊的変更時は `schema_version` を整数インクリメント、golden fixture を同時更新
- `analyze_run.py` の schema v2 へのバンプ時は compare.py / docs / golden を同時更新
