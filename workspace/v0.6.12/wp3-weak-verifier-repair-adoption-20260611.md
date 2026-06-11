# WP3 Weak Verifier Repair Adoption

作成日: 2026-06-11

参照:

- `workspace/v0.6.12/wp2-weak-verifier-repair-shadow-20260611.md`
- `workspace/v0.6.12/architecture-work-plan-20260611.md`

## 1. Scope

WP3 の目的は、typed weak verifier reason がある場合だけ、`safe_stop_verifier_weak` を bounded repair lifecycle へ流すこと。

実装:

- `active_reason_for_safe_stop` を追加し、adoption 条件を SSOT 化。
- 条件は `SafeStopReason::VerifierWeak` かつ `weak_reason: Some(_)` のみ。
- `weak_reason: None` と `VerifierMissing` は従来どおり safe-stop / missing verifier path。
- active adoption は新しい repair loop を作らず、既存の `MissingVerifierJob` に流す。
- adoption 時に `agent.verifier.weak_repair_adopted` を出す。

設計上の注意:

- unknown / untyped reason は採用しない。
- benchmark 名や failure log substring による branch は追加していない。
- repair budget は既存 `MissingVerifierJob` の retry budget を使い、無限修復にはしない。

## 2. Deterministic Verification

実行:

```text
cargo test --lib verifier_weak_repair_target -- --nocapture
cargo test --lib safe_stop_verifier_weak -- --nocapture
cargo test --lib missing_verifier_job_retry_budget_is_bounded -- --nocapture
cargo build
```

結果:

- `verifier_weak_repair_target`: 4/4 pass
- `safe_stop_verifier_weak`: 1/1 pass
- `missing_verifier_job_retry_budget_is_bounded`: 1/1 pass
- `cargo build`: pass

確認内容:

- typed weak reason がある時だけ active adoption される。
- weak reason がない場合は adoption されない。
- `VerifierMissing` は adoption されない。
- retry budget は既存 MissingVerifierJob の bounded lifecycle に残る。

## 3. Real LLM Smoke

### 13-run targeted smoke

実行:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence rust_word,rust_word,rust_word,rust_word,rust_word,rust_word,rust_ndjson,rust_ndjson,rust_ndjson,python_markdown,python_markdown,docs_runbook,data_json \
  --variant no_pam \
  --run-id wp3-weak-verifier-repair-adoption-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

結果:

| Case | Runs | Pass | HQ | Notes |
| --- | ---: | ---: | ---: | --- |
| rust_word | 6 | 6 | 6 | all `done` |
| rust_ndjson | 3 | 2 | 2 | one `verifier_failed` import drift |
| python_markdown | 2 | 2 | 1 | one `repair_exhausted`, external grader pass true but HQ false |
| docs_runbook | 1 | 1 | 1 | done |
| data_json | 1 | 1 | 1 | done |
| total | 13 | 12 | 11 | false-done 0, false-missing 0 |

Artifact:

- `workspace/v0.6.11/eval-runs/wp3-weak-verifier-repair-adoption-20260611/results.csv`

### post-helper 2-run smoke

helper 抽出後に追加実行:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence rust_word,python_markdown \
  --variant no_pam \
  --run-id wp3-weak-verifier-repair-adoption-post-helper-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

結果:

- rust_word: 1/1 pass, HQ 1/1
- python_markdown: 1/1 pass, HQ 1/1

Artifact:

- `workspace/v0.6.11/eval-runs/wp3-weak-verifier-repair-adoption-post-helper-20260611/results.csv`

## 4. Observations

- この targeted smoke では `safe_stop_verifier_weak` が発火しなかったため、active adoption event は実 LLM では観測されなかった。
- Rust word は v0.6.12 baseline では 0/6 `safe_stop_verifier_weak` だったが、今回の 6+1 run では全て `done`。
- ただし、これは小規模 smoke であり、WP3 だけの改善効果として断定しない。
- Rust NDJSON の 1 件は unresolved import による `verifier_failed`。weak verifier adoption とは別の binding/import drift。
- Python markdown の 1 件は repair exhaustion。既存の repair routing 問題であり、weak verifier adoption ではない。
- docs/data に regression は見られない。

## 5. WP3 Decision

WP3 は完了。

次へ進む条件:

- false-done 0 / false-missing 0 は維持。
- active adoption が発火するケースは今回観測できていないため、今後 20-run guard では `agent.verifier.weak_repair_adopted` を明示的に追う。
- Rust word の改善傾向はあるが、偶然や生成 variance の可能性を排除できないため、20-run / 50-run 前に改善主張はしない。
