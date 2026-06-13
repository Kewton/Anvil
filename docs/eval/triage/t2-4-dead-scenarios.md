# T2-4 Dead Scenarios Check Audit

Date: 2026-06-12 JST

対象:

- Legacy root: `.anvil/benchmarks/20260612T024532-70925`
- Minimal root: `.anvil/benchmarks/20260612T145227-40162`
- Compare root: `.anvil/benchmarks/20260612T145227-combined-t2-4-fixed-rerun`

本調査は成果物ディレクトリ、`summary.tsv`、`meta.json`、`session.json` の突き合わせのみ。success_check の修正や緩和はしていない。

## Scope Note

ユーザー指示では「両エンジン success 0/5 の9シナリオ」とされていたが、今回の meta fallback 済み集計では該当は8シナリオだった。

該当8シナリオ:

- `fix-json-normalizer`
- `long-session-data-report`
- `multi-file-rust-library`
- `new-rust-cli-small`
- `new-typescript-formatter`
- `scaffold-fastapi-service`
- `scaffold-next-dashboard`
- `scaffold-rust-cli`

9本目は現在の比較データからは確認できないため、推測で追加しない。

## Summary

| scenario | direct cause | classification | check fix candidate |
|---|---|---|---|
| `fix-json-normalizer` | 両エンジンとも `src/normalizeJson.ts` を作成し `normalizeJson` を含むが、15-25行で `min_lines:30` に届かない | (1) success_check bug / overstrict | `min_lines` を下げるか削除し、stable sorted JSON の意味的 check に置換 |
| `multi-file-rust-library` | `Cargo.toml`、`src/lib.rs`、`tests/math_tests.rs` は揃い、`median` も含むが `src/lib.rs` が20-28行で `min_lines:30` に届かない | (1) success_check bug / overstrict | `src/lib.rs` の `min_lines` を現実的な値に下げる。可能なら `cargo test` 系の semantic check へ寄せる |
| `new-rust-cli-small` | legacy は正しい8行実装を作るが `min_lines:20` で落ちる。minimal は5/5で対象ファイル未作成 | mixed: legacy は (1)、minimal は (3) | `min_lines` を8-10程度に下げるか実行 check に置換。修正後は minimal の no-write failure を測れる |
| `new-typescript-formatter` | legacy は全件8行実装。minimal は4/5で8行実装、1/5は未作成。いずれも `min_lines:20` 起因が主 | mixed: 主に (1)、一部 (3) | `min_lines` を8-10程度に下げるか formatter の入出力 check に置換 |
| `long-session-data-report` | legacy は5/5でCSV/レポート未作成。minimal は2/5でCSV/レポート作成済みだが grep が小文字 exact `top products` で失敗、他3/5は未完 | mixed: (1) + (3) | grep を case-insensitive にする、または見出し/本文の語彙を複数許容する。未作成 run は能力/loop failure として残す |
| `scaffold-fastapi-service` | minimal は必要ファイルを作るが各ファイルが compact で `min_lines` に届かない。legacy は `pyproject.toml`、routes、tests が未作成 | mixed: minimal は (1)、legacy は (3) | scaffold の `min_lines` を下げる。別PRで「最小 scaffold」を許容する基準へ変更 |
| `scaffold-next-dashboard` | minimal は3/5で未作成、2/5で一部作成するが行数/欠落で失敗。legacy は page のみ作成し component/CSS 欠落 | mixed: (1) + (3) | 行数条件を下げる余地はあるが、欠落 run が多いため check 修正だけで全通過にはならない |
| `scaffold-rust-cli` | minimal は5/5で対象 scaffold 未作成。legacy は `Cargo.toml` と `src/args.rs` に偏り、`src/main.rs` と tests 欠落 | (3) current agent/model limit | 現時点では check 修正候補なし。loop/recovery 側の課題として扱う |

## Classification Counts

| category | scenarios | count |
|---|---|---:|
| (1) success_check bug / overstrict only | `fix-json-normalizer`, `multi-file-rust-library` | 2 |
| mixed: check bug plus real agent failure | `new-rust-cli-small`, `new-typescript-formatter`, `long-session-data-report`, `scaffold-fastapi-service`, `scaffold-next-dashboard` | 5 |
| (2) environment cause | none found | 0 |
| (3) model/agent capability or loop failure | `scaffold-rust-cli` | 1 |

環境起因の証拠は確認できなかった。scaffold 系も `npm create` や `cargo generate` のネットワーク失敗ではなく、手書きの `Write` 欠落、行数基準未達、または legacy recovery の偏りが直接原因だった。

## Scenario Notes

### `fix-json-normalizer`

`success_check`:

- `src/normalizeJson.ts`, `min_lines: 30`
- grep `normalizeJson`

観測:

- legacy: 5/5 `rc=0`、ファイルあり、grep true、15-19行。
- minimal: 5/5 `rc=0`、ファイルあり、grep true、18-25行。
- 代表 minimal 実装は `sortValue` で object key を sort し、`JSON.stringify(..., 2)` を返している。

判定: check の `min_lines:30` が過剰。これは能力差を見る前に別PRで直すべき。

### `multi-file-rust-library`

`success_check`:

- `Cargo.toml`
- `src/lib.rs`, `min_lines: 30`
- `tests/math_tests.rs`, `min_lines: 20`
- grep `median` in `src/lib.rs`

観測:

- legacy: 必要ファイルあり、`median` あり、`src/lib.rs` は20-25行。
- minimal: 必要ファイルあり、`median` あり、`src/lib.rs` は27-28行。

判定: `src/lib.rs` の行数基準だけが両エンジンを落としている。現状の check は判別力を消している。

### `new-rust-cli-small`

`success_check`:

- `src/bin/word_count.rs`, `min_lines: 20`
- grep `words=`

観測:

- legacy: 5/5で `src/bin/word_count.rs` を作成。8行の stdin word count 実装で `words=` を出力。
- minimal: 5/5で対象ファイル未作成。

判定: legacy 側は check bug、minimal 側は no-write failure。check を直すと legacy 5/5 vs minimal 0/5 に近い、判別力のあるシナリオになる。

### `new-typescript-formatter`

`success_check`:

- `src/formatTitle.ts`, `min_lines: 20`
- grep `export function formatTitle`

観測:

- legacy: 5/5で8行実装。
- minimal: 4/5で8行実装、1/5で未作成。

判定: 主因は `min_lines:20` の過剰さ。1件だけ minimal の no-write failure が残る。

### `long-session-data-report`

`success_check`:

- `data/sample-sales.csv`, `min_lines: 12`
- `reports/sales-analysis.md`, `min_lines: 40`
- grep exact `top products`

観測:

- legacy: 5/5でCSV/レポート未作成。
- minimal: run-2/run-3 はCSVとレポートを作成し、レポートも85行/106行あるが、exact lowercase `top products` に一致せず失敗。run-1/run-4/run-5 は欠落あり。

判定: check bug と real failure が混在。case-sensitive grep は妥当性が低いが、legacy の未作成と minimal の3/5欠落は check 修正では救えない。

### `scaffold-fastapi-service`

`success_check`:

- `pyproject.toml`
- `app/main.py`, `min_lines: 25`
- `app/routes/health.py`, `min_lines: 20`
- `tests/test_health.py`, `min_lines: 20`
- grep `FastAPI`

観測:

- legacy: `app/main.py` は作るが、`pyproject.toml`、route、tests が未作成。
- minimal: 必要ファイルは概ね作るが、`app/main.py` 6-12行、`health.py` 8行、tests 10-11行で `min_lines` 未達。

判定: minimal については check が最小実装を不当に落としている。legacy については scaffold 欠落。

### `scaffold-next-dashboard`

`success_check`:

- `src/app/page.tsx`, `min_lines: 50`
- `src/components/MetricCard.tsx`, `min_lines: 30`
- `src/app/globals.css`, `min_lines: 30`
- grep `MetricCard` in page

観測:

- legacy: page のみ作るが22行で、component/CSS 欠落。
- minimal: run-1/run-2/run-3 は未作成。run-4 は3ファイル作成だが page 32行、MetricCard 24行。run-5 は page/CSS 作成、MetricCard 欠落。

判定: check の行数条件は過剰だが、欠落 run も多い。check 修正だけでは成功率は限定的にしか上がらない。

### `scaffold-rust-cli`

`success_check`:

- `Cargo.toml`
- `src/main.rs`, `min_lines: 30`
- `src/args.rs`, `min_lines: 25`
- `tests/cli_smoke.rs`, `min_lines: 15`
- grep `--name` in `src/args.rs`

観測:

- legacy: `Cargo.toml` と `src/args.rs` に偏る。`src/main.rs` と tests は未作成。ログ上は artifact recovery が `src/args.rs` へ繰り返し誘導している。
- minimal: 5/5で対象 scaffold 未作成。空 workdir を確認後、作成意図を本文で述べて終了する no-write pattern。

判定: 現時点では check 修正ではなく loop/recovery 側の失敗。dead scenario のまま残す。

## Follow-up Candidates

別PRの check 修正候補:

1. `fix-json-normalizer`: `min_lines` の削除または semantic check 化。
2. `multi-file-rust-library`: `src/lib.rs` 行数基準を下げる。
3. `new-rust-cli-small`: 行数基準を最小CLIに合わせるか実行 check 化。
4. `new-typescript-formatter`: 行数基準を最小 formatter に合わせるか入出力 check 化。
5. `long-session-data-report`: `top products` grep を case-insensitive または語彙許容にする。
6. `scaffold-fastapi-service`: compact scaffold を許容する行数基準へ変更。
7. `scaffold-next-dashboard`: 行数基準は修正余地あり。ただし欠落 run が多いため、修正効果は限定的。

check 修正対象外:

- `scaffold-rust-cli`: 現時点では agent/loop failure として残す。
