# P33 Generic Code WorkMode Validation

Date: 2026-06-07

## 目的

Rust/Cargo/TDD の依頼が、WorkMode second-pass LLM によって `python` へ誤補正される入口を閉じる。

これは Rust 専用の provider abstraction を増やす対応ではない。既存の compatibility label では Rust / Go / Java / Node CLI などは `generic-code` に属するため、first-pass が明確な generic code stack signal を見つけた場合は高信頼にして、LLM 確認で別モードへ上書きされないようにする。

## 実装内容

- `classify_work_mode_json` に `request_has_generic_code_stack_signal` を追加。
- explicit edit intent と generic code stack signal が同時にある場合、`GenericCode` candidate の confidence を `0.92` に引き上げる。
- Python / TypeScript UI の明示シグナルがある場合は既存の専用モードを優先し、generic-code 側の引き上げをしない。

## Unit 検証

通過:

- `cargo test --offline --lib infer_work_mode_detects_specific_edit_domains`
- `cargo test --offline --lib orchestrator_skips_high_confidence`

確認したこと:

- Rust library + `cargo test` の依頼は `GenericCode`。
- password strength の Rust/Cargo TDD 依頼は `GenericCode`。
- confidence は `WORK_MODE_CONFIRM_CONFIDENCE_THRESHOLD` 以上。
- `should_request_confirmation` は false。

## Actual local LLM 検証

コマンド:

```text
target/debug/anvil --oneshot --fresh-session --no-footer --trace -y \
  -m qwen3.6:27b-coding-mxfp8 \
  --state-dir /private/tmp/anvil-p34-workmode-tdd-state \
  --cwd /private/tmp/anvil-p34-workmode-tdd-work \
  --max-iterations 6 --chat-timeout-secs 180 \
  -p "TDDで進めてください。まず tests/password_strength.rs に失敗するテストを書き、その後 src/lib.rs に password_score(password: &str) -> u8 を実装してください。期待仕様: 8文字未満は0、8文字以上で1、英小文字と英大文字を両方含むと+1、数字を含むと+1、記号を含むと+1。cargo test --manifest-path Cargo.toml が成功するまで進めてください。"
```

結果:

- `agent.work_mode.classified`: `work_mode="generic-code"`, `confidence=0.92`, `evidence=["edit-intent","generic-code-stack-signal"]`
- `agent.work_mode.skipped`: `reason="high_confidence"`, `parse_status="not_invoked"`
- `session.json`: `mode_state.work_mode = "GenericCode"`
- 実LLMは `Cargo.toml`, `tests/password_strength.rs`, `src/lib.rs` を編集し、verifier 起動まで到達した。

## 限界と次のボトルネック

この対応は WorkMode の誤入力を減らす土台改善であり、TDD成功を直接保証しない。

今回の短い run は `max_iterations` で停止した。verifier failure は `no test target named password_strength` で、LLM が `[[test]] name = "password_strength_tests"` を追加した一方、verifier が `password_strength` test target を期待していた。次の小検証では、named Cargo test target を生成しない、または verifier が manifest の test target 名を尊重する設計のどちらが保守性の高い修正かを確認する。

## アーキテクチャ上の意味

WorkMode は最終的な tool policy の唯一の根拠にしない方針を維持する。ただし、WorkMode が明らかに誤っていると prompt / verifier / recovery に不要なノイズが入るため、観測可能な objective / project signal で first-pass の品質を上げることは有効。

この変更は、coding 以外にも適用できる「低信頼な自然言語分類を LLM に丸投げしない」方針の一部である。LLM は曖昧な場合の確認に使い、明確な artifact/runtime signal は controller が保持する。
