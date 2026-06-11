# WP2 Weak Verifier Repair Shadow

作成日: 2026-06-11

参照:

- `workspace/v0.6.12/wp1-verifier-weak-observability-20260611.md`
- `workspace/v0.6.12/architecture-work-plan-20260611.md`

## 1. Scope

WP2 の目的は、`safe_stop_verifier_weak` をただの terminal stop ではなく、将来 repair へ流せる typed target として shadow projection すること。

この WP では active terminal behavior は変えていない。

実装:

- `VerifierWeakRepairTarget` を追加。
- `VerifierWeakReason` から shadow repair target を投影。
  - `structured_selection_unbound` -> `missing_evidence_job`
  - `weak_plan_metadata_only` -> `missing_evidence_job`
  - `exit_zero_without_owned_binding` -> `evidence_failed_job`
- `agent.verifier.weak_repair_shadow` event を追加。
- event payload は `weak_reason`, `target_job`, `expected_delta`, `adoption_state=shadow_only`, `terminal_behavior=unchanged_safe_stop` を持つ。

設計上の制約:

- benchmark case 名による分岐は追加していない。
- failure log 断片の pattern matching は追加していない。
- WP3 までは active repair adoption を行わない。

## 2. Deterministic Verification

実行:

```text
cargo test --lib verifier_weak_repair_target -- --nocapture
cargo test --lib safe_stop_verifier_weak -- --nocapture
cargo build
```

結果:

- `verifier_weak_repair_target`: 2/2 pass
- `safe_stop_verifier_weak`: 1/1 pass
- `cargo build`: pass

確認内容:

- weak reason から shadow target への projection が typed に固定された。
- `SafeStopVerifierWeak` の terminal mapping は変わっていない。

## 3. Real LLM Smoke

実行:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence rust_word,rust_word,rust_word,rust_ndjson,rust_ndjson,python_markdown \
  --variant no_pam \
  --run-id wp2-weak-verifier-repair-shadow-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

結果:

| Case | Runs | Pass | HQ | Exit |
| --- | ---: | ---: | ---: | --- |
| rust_word | 3 | 3 | 3 | done x3 |
| rust_ndjson | 2 | 2 | 2 | done x2 |
| python_markdown | 1 | 1 | 1 | done x1 |
| total | 6 | 6 | 6 | done x6 |

Artifact:

- `workspace/v0.6.11/eval-runs/wp2-weak-verifier-repair-shadow-20260611/results.csv`

観測:

- この 6-run では `safe_stop_verifier_weak` は発火しなかった。
- `agent.verifier.weak_repair_shadow` event も実 LLM run では発火しなかった。
- regression は見られない。小規模 smoke なので成功率改善は主張しない。

## 4. WP2 Decision

WP2 は完了。

次 WP で限定採用する場合の安全条件:

- unknown / untyped reason は active repair に採用しない。
- `structured_selection_unbound` / `weak_plan_metadata_only` は MissingEvidence 系の bounded repair に限定する。
- `exit_zero_without_owned_binding` は EvidenceFailed 系の rerun/binding repair に限定する。
- false-done 0 を維持する。
