# P32 Tool Call Terminal Closer Validation

Date: 2026-06-07

## 目的

TDD / coding 系の実LLM検証で、XML fallback の `<anvil_tool_call>` 内 JSON がほぼ正しいにもかかわらず、末尾に余分な `}` が1つ付いて `tool_call_format_error` になるケースを最小修正する。

この修正は coding 専用の特殊ルールではなく、local LLM が出しやすい「構造は成立しているが末尾だけ閉じ括弧が過剰」という汎用的な JSON 復元として扱う。

## 実装内容

- `parse_json_relaxed` に JSON 文字列内制御文字の escape 復元を追加。
- `repair_json_candidate` に、文字列外の bracket balance が `(-1, 0)` または `(0, -1)` の場合だけ、最後の非空白終端 `}` / `]` を1つ削る復元を追加。
- `parse_generate_response` からも、TOML `[[test]]` を含む `Edit` tool call が復元される回帰テストを追加。

## 最小検証

### Unit tests

通過:

- `cargo test --offline --lib extracts_edit_with_toml_array_header_in_string_argument`
- `cargo test --offline --lib parse_generate_response_accepts_recovered_edit_with_nested_toml_arrays`
- `cargo test --offline --lib parse_generate_response_rejects_malformed_tool_call_markup`

全体:

- sandbox 内の `cargo test --offline --lib` は `mockito` local server が `Operation not permitted` で失敗。
- 権限付き再実行では `3832 passed; 0 failed`。

### Actual local LLM validation

コマンド:

```text
target/debug/anvil --oneshot --fresh-session --no-footer --trace -y \
  -m qwen3.6:27b-coding-mxfp8 \
  --state-dir /private/tmp/anvil-p33-tdd-parser-state \
  --cwd /private/tmp/anvil-p33-tdd-parser-work \
  --max-iterations 24 --chat-timeout-secs 180 \
  -p "TDDで進めてください。まず tests/password_strength.rs に失敗するテストを書き、その後 src/lib.rs に password_score(password: &str) -> u8 を実装してください。期待仕様: 8文字未満は0、8文字以上で1、英小文字と英大文字を両方含むと+1、数字を含むと+1、記号を含むと+1。cargo test --manifest-path Cargo.toml が成功するまで進めてください。"
```

結果:

- 以前の類似ケースでは、`Cargo.toml` への `Edit` tool call が malformed XML/JSON と判定され `tool_call_format_error` で停止した。
- 今回は `Cargo.toml` edit、`tests/password_strength.rs` write、`src/lib.rs` write まで進み、verifier repair に到達した。
- 最終結果は `repair_exhausted: patch_rejected_repeatedly`。
- 手動 `cargo test --offline --manifest-path /private/tmp/anvil-p33-tdd-parser-work/Cargo.toml` でも `test_with_symbol` と `test_max_score` の2件が失敗。

## 得られたインサイト

今回の修正で、local LLM の軽微な tool-call 末尾崩れは controller に届くようになった。これは成功率向上に寄与するが、TDD成功を保証する修正ではない。

次の構造的ボトルネックは、verifier repair の authority 判定である。今回の失敗では、仕様上 `abc!2345` は「8文字以上 + 記号」で2点、`Abc123!@` は「8文字以上 + 大小文字 + 数字 + 記号」で4点のはずだが、生成テストは最大スコアを5点として期待していた。diagnostic は implementation 修正を選んだが、repair plan が `test_expectation_without_authority` や `ambiguous_authority` で拒否され、仕様と生成テストのどちらを修正すべきかを controller が十分に決めきれていない。

## 次に検証すべき仮説

1. ObjectiveContract に user-specified expectation を明示的に保持し、generated test expectation より高い authority として verifier repair に渡す必要がある。
2. verifier repair は `owned_test_artifacts` を持っている場合、生成テストの期待値も修正対象にできるが、ユーザー仕様に反する期待値だけを限定的に直す gate が必要。
3. `WorkMode` は今回も session 上で `Python` に補正されており、Cargo/Rust の実体と矛盾している。tool policy は `WorkMode` だけで決めず、ControllerStatePacket / ObjectiveContract / observed project facts を合わせて決める必要がある。
