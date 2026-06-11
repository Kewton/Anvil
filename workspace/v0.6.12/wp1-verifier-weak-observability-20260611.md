# WP1 Verifier Weak Observability

作成日: 2026-06-11

参照:

- `workspace/v0.6.12/wp0-failure-composition-20260611.md`
- `workspace/v0.6.12/architecture-work-plan-20260611.md`

## 1. Scope

目的は `safe_stop_verifier_weak` の理由を typed に観測できるようにすること。WP1 では terminal behavior は変えない。

実装:

- `VerifierWeakReason` を追加。
  - `structured_selection_unbound`
  - `exit_zero_without_owned_binding`
  - `weak_plan_metadata_only`
- `CompletionDecision::SafeStop` / `ArtifactRecoveryAction::SafeStop` / `TaskContractVerifierOutcome::SafeStop` に optional `weak_reason` を追加。
- `agent.verifier.weak` と `agent.task_contract.safe_stop` に `weak_reason` / `repairability_hint` を additive に出力。
- `SafeStopLinkage` に optional `weak_reason` を追加し、job report からも参照できるようにした。
- `TurnState` に turn-local な `verifier_weak_reason_this_turn` を追加。

設計上の注意:

- benchmark case name や failure log substring による分岐は追加していない。
- `SafeStopReason::VerifierWeak` の terminal projection は維持した。
- job report の変更は optional field の追加のみで、既存 JSON の deserialize 互換を保つ。

## 2. Deterministic Verification

実行:

```text
cargo test --lib verifier_weak_reason -- --nocapture
cargo test --lib safe_stop_verifier_weak -- --nocapture
cargo test --lib agent::loop_run::job_report::tests::safe_stop_linkage_serde_round_trip_with_defaults -- --nocapture
cargo test --lib turn_state_reset_clears_grouped_dedup_state -- --nocapture
cargo build
```

結果:

- `verifier_weak_reason`: 3/3 pass
- `safe_stop_verifier_weak`: 1/1 pass
- `safe_stop_linkage_serde_round_trip_with_defaults`: 1/1 pass
- `turn_state_reset_clears_grouped_dedup_state`: 1/1 pass
- `cargo build`: pass

補足:

- `cargo test --lib safe_stop_linkage -- --nocapture` は広い filter により既存の logging 初期化前提を持つ E2E も拾い、2 件失敗した。追加した serde test 自体は exact path で pass。今回の変更による新規失敗ではない。

## 3. Real LLM Smoke

実行:

```text
python3 workspace/v0.6.11/wp_eval_matrix.py \
  --suite wp11 \
  --case-sequence rust_word,rust_word,rust_ndjson \
  --variant no_pam \
  --run-id wp1-verifier-weak-observability-20260611 \
  --anvil-bin target/debug/anvil \
  --timeout-secs 600 \
  --chat-timeout-secs 240
```

結果:

| Case | Runs | Pass | HQ | Exit |
| --- | ---: | ---: | ---: | --- |
| rust_word | 2 | 2 | 2 | done x2 |
| rust_ndjson | 1 | 1 | 1 | done x1 |
| total | 3 | 3 | 3 | done x3 |

Artifact:

- `workspace/v0.6.11/eval-runs/wp1-verifier-weak-observability-20260611/results.csv`

観測:

- この 3-run では `safe_stop_verifier_weak` は発火しなかった。
- `agent.verifier.weak` / `weak_reason` の実ログ出力は今回の LLM smoke では確認対象にならなかった。
- regression は見られない。小規模 smoke なので成功率改善は主張しない。

## 4. WP1 Decision

WP1 は完了。次 WP では、この typed weak reason を repair target へ shadow projection する。

期待する次の確認:

- `structured_selection_unbound` / `exit_zero_without_owned_binding` / `weak_plan_metadata_only` を、個別 benchmark branch ではなく evidence-binding repair target として扱えるか。
- terminal behavior を変えずに shadow log / eval summary へ出せるか。
