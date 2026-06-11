# WP0 Failure Composition Baseline

作成日: 2026-06-11

参照:

- `workspace/v0.6.12/README.md`
- `workspace/v0.6.12/future-architecture-direction-20260611.md`
- `workspace/v0.6.12/architecture-work-plan-20260611.md`

## 1. Baseline

v0.6.12 の 50-run は、以後の WP で比較する固定 baseline とする。

| Mode | Runs | Pass | Rate |
| --- | ---: | ---: | ---: |
| no-PAM | 25 | 8 | 32.0% |
| PAM | 25 | 6 | 24.0% |
| overall | 50 | 14 | 28.0% |

直近系列:

- v0.6.9: 44%
- v0.6.10: 42%
- v0.6.11: 32%
- v0.6.12: 28%

安全性指標:

- false-done: 0
- false-missing: 1
- docs aggregate: 48/48
- code aggregate: 81/382

この baseline では、false-done 0 は維持されている。一方で、code case は 12-round aggregate でも 21.2% に留まり、直近 2 round は refactor churn の影響を受けている。

## 2. Case Composition

| Case | v0.6.12 | v0.6.11 | Read |
| --- | ---: | ---: | --- |
| Docs SRE runbook | 6/6 | 6/6 | 安定 |
| Python sales CLI | 3/6 | 0/6 | 部分回復 |
| FastAPI notes API | 2/6 | 2/6 | 横ばい |
| Rust slug library | 2/4 | 3/4 | 低下 |
| Node JSON formatter | 1/6 | 1/6 | 横ばい |
| Python markdown lint CLI | 0/4 | 2/4 | 低下 |
| Node CSV to JSON CLI | 0/4 | 2/4 | 低下 |
| Rust NDJSON merge CLI | 0/4 | 0/4 | 横ばい |
| Rust word counter | 0/6 | 0/6 | 横ばい、ただし失敗理由が変化 |
| Python TOML merge CLI | 0/4 | 0/4 | 継続壁 |

Python sales は runner/style 分離により 0/6 から 3/6 へ戻ったが、v0.6.9/v0.6.10 の水準には戻っていない。Python markdown と Node CSV の低下が相殺し、全体は 28% へ低下した。

## 3. Terminal / Failure Signatures

次 WP で追うべき主な terminal / failure signature:

- `safe_stop_verifier_weak`
  - Rust word: 6/6
  - Rust NDJSON: 1 件
  - Rust slug: 1 件
  - 現状は「弱い verifier」を成功扱いしない安全側の gate として働いているが、repair target へ流れていない。
- `repair_exhausted`
  - Python sales の残り失敗
  - Python markdown 0/4
  - Node CSV 0/4
- `max_iterations`
  - TOML の継続壁に含まれる。
- false-missing
  - 1 件に留まるが、safe-stop / repair target の変更時に増やさないことが必須。

## 4. PAM / Memory Read

PAM は v0.6.12 でも no-PAM を上回っていない。

- no-PAM: 8/25
- PAM: 6/25
- 12 round 継続して明確な PAM benefit は確認できない。

したがって、WP1-WP11 では PAM を authority にしない。PAM は advisory / observability の対象に留め、成功率改善の主因としては扱わない。

## 5. 20-run Guard Sequence

WP9 以降の regression guard は、次の case sequence を固定候補にする。

```text
python_sales,
python_markdown,
node_csv,
node_json,
rust_word,
rust_ndjson,
fastapi_notes,
toml_merge,
docs_runbook,
data_json_or_csv,
research_brief,
ops_command_observation,
feature_discount
```

配分方針:

- 20-run では code hard cases を厚めにする。
- docs/data/research/ops は non-coding regression guard として最低 1 run ずつ含める。
- `feature_discount` は BehaviorDeltaObligation の対象として必ず含める。
- no-PAM / PAM の比較は記録するが、WP の合否は PAM benefit では判断しない。

判定軸:

- pass rate overall / task-kind 別
- false-done
- false-missing
- `safe_stop_verifier_weak`
- `repair_exhausted`
- `max_iterations`
- weak verifier reason / repair target shadow or adoption
- behavior delta obligation の projection / binding

## 6. WP0 Decision

WP0 では controller behavior を変更しない。既存 artifact で baseline 固定に必要な情報は足りているため、追加 LLM run は実施しない。

次 WP では、まず `safe_stop_verifier_weak` の reason を typed に観測できるようにする。ここでは pass rate 改善を主張せず、以後の repair target 化に必要な観測性を作る。
