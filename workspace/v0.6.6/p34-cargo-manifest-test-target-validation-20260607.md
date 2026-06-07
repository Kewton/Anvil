# P34 Cargo Manifest Test Target Validation

Date: 2026-06-07

## 目的

Rust/Cargo の verifier preflight が、`tests/<stem>.rs` から機械的に `cargo test --test <stem>` を合成するだけでなく、`Cargo.toml` の明示 `[[test]] path -> name` を尊重するようにする。

最終 verifier は引き続き full `cargo test` のままにする。今回の修正対象は generated test の compile preflight だけであり、部分実行を成功判定に戻すものではない。

## 実装内容

- `CargoManifestEvidence` の行単位 scan を拡張し、`[[test]]` section の `name` と `path` を収集する。
- Cargo preflight で bound test artifact path に対応する manifest target name があれば、それを `--test <name>` に使う。
- manifest に対応がない場合は従来どおり `tests/<stem>.rs -> --test <stem>` にフォールバックする。

## Unit / deterministic 検証

通過:

- `cargo test --offline --lib cargo_test_targets_from_manifest_maps_explicit_path_to_name`
- `cargo test --offline --lib cargo_manifest_target_name_precedes_path_stem_for_preflight`
- `cargo test --offline --lib verifier_command_from_cargo_test_runs_full_suite`

前回の実LLM生成物を使った手動 preflight:

```text
cargo test --offline \
  --manifest-path /private/tmp/anvil-p34-workmode-tdd-work/Cargo.toml \
  --no-run --test password_strength_tests
```

結果:

- compile preflight 成功。
- `tests/password_strength.rs` は `password_strength_tests` target として解決された。

## Actual local LLM 検証

同じ password strength TDD 依頼を `qwen3.6:27b-coding-mxfp8` で再実行した。

結果:

- WorkMode は引き続き `generic-code`, high confidence skip。
- run は verifier まで到達せず、`tool_call_format_error: unterminated <anvil_tool_call>` で終了。
- 最後の raw response は `<anvil_tool_call>{"arguments":{...}}` 形で、tool `name` が欠落し、closing tag も欠けていた。

このため、今回の Cargo preflight 修正は unit / deterministic preflight では確認済みだが、実LLM E2E での到達確認は未完了。次のボトルネックは、local LLM が tool name を落とす malformed XML tool call をどう扱うかである。

## アーキテクチャ上の意味

この修正は Rust 専用 provider を増やさず、EvidenceRunner の Cargo runner が manifest に既に存在する構造的事実を利用するだけに留めた。

汎用性の観点では、verifier は「ファイル名から実行単位を推測する」よりも「project/manifest が持つ実行単位を優先する」方が正しい。これは Node の package script や Python の pytest config にも同じ考え方で拡張できる。
