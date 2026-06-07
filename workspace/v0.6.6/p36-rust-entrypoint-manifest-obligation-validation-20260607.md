# P36 Rust Entrypoint Manifest Obligation Validation

Date: 2026-06-07

## 目的

p36 の実LLM検証では、Rust/TDD タスクで `src/lib.rs` と `tests/password_strength.rs` は作れたが、`Cargo.toml` が ObjectiveContract の required deliverable に入っていなかった。そのため verifier が選べず `safe_stop_verifier_missing` になった。

今回の目的は、Rust 固有の verifier 例外や SetupBootstrap の優先度変更ではなく、明示された実装エントリポイントから project shape を推定し、既存の inferred manifest obligation 経路で `Cargo.toml` を MissingDeliverableJob に乗せること。

## 実装内容

- `infer_project_shape` に explicit entrypoint hint を追加。
- `src/lib.rs` / `lib.rs` を `ProjectShape::Library` として扱う。
- `src/main.rs` / `main.py` を `ProjectShape::Cli` として扱う。

この修正は `EvidenceRunner` や provider abstraction を増やさず、既存の `ProjectIntent -> inferred_artifact_obligations_from_project_intent -> ArtifactRecovery` lifecycle を使う。

## Unit / deterministic 検証

追加した regression:

- `cargo test --offline --lib rust_tdd_request_with_lib_path_requires_manifest_obligation`

このテストは修正前に失敗した:

- `ProjectIntent.shape == Unknown`
- `Cargo.toml` setup obligation が生成されない

修正後に確認した関連テスト:

- `cargo test --offline --lib rust_tdd_request_with_lib_path_requires_manifest_obligation`
- `cargo test --offline --lib rust_library_contract_requires_manifest_impl_test_and_readme_obligations`
- `cargo test --offline --lib project_intent_classifies_rust_cli_word_counter_prompt`
- `cargo build --offline`

## Actual local LLM 検証

同じ password strength TDD 依頼を `qwen3.6:27b-coding-mxfp8` で再実行した。

Command shape:

```text
target/debug/anvil --oneshot --fresh-session --no-footer --trace -y \
  -m qwen3.6:27b-coding-mxfp8 \
  --state-dir /private/tmp/anvil-p37-manifest-obligation-state \
  --cwd /private/tmp/anvil-p37-manifest-obligation-work \
  --max-iterations 16 --chat-timeout-secs 180 \
  -p "TDDで進めてください。まず tests/password_strength.rs に失敗するテストを書き、その後 src/lib.rs に password_score(password: &str) -> u8 を実装してください。期待仕様: 8文字未満は0、8文字以上で1、英小文字と英大文字を両方含むと+1、数字を含むと+1、記号を含むと+1。cargo test --manifest-path Cargo.toml が成功するまで進めてください。"
```

Result:

- `Objective Contract` に `path=Cargo.toml role=setup kind=file` が入った。
- 実LLM は `src/lib.rs`, `tests/password_strength.rs`, `Cargo.toml` を作成した。
- `agent.project_unit.verifier_selection` は `roles=implementation,test,setup manifests=Cargo.toml verifiers=cargo_manifest:short_unit_test:cargo test confidence=high`。
- final outcome は `done`。
- `cargo test` が実行され、integration test 6件が pass した。

## アーキテクチャ上の意味

この修正は「Rust なら Cargo.toml を後から特別に作る」という局所パッチではない。ユーザーが明示したエントリポイントを project shape の根拠にし、その結果として必要な setup deliverable を ObjectiveContract に含める。

汎用性の観点では、これは Node の `src/index.js` / `package.json`、Python CLI の `main.py` / test runner config へ同じ pattern で拡張できる。重要なのは、WorkMode や tool policy だけで判断せず、ObjectiveContract が成果物と evidence prerequisite を持つことである。

## 残課題

p37 は成功したが、初回の `Cargo.toml` Write は `src/lib.rs` targeted recovery 中だったため一度 reject された。最終的には setup target に遷移して成功したが、成果物依存順序としては setup prerequisite を implementation/test より先に出せる方が無駄が少ない。

次の小検証候補:

- MissingDeliverableJob の target selection で、evidence prerequisite setup を implementation/test より先に選ぶべきか。
- ただし docs/data/research で setup role を過剰に優先しないよう、ObjectiveContract 上の `evidence spec: test_run` と project manifest obligation がある場合に限定する。
