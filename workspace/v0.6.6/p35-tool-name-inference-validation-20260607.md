# P35 Tool Name Inference Validation

Date: 2026-06-07

## 目的

p35 の Rust/TDD 実LLM検証では、LLM が `<anvil_tool_call>{"arguments":{...}}` という形で tool `name` と closing tag を落とし、`tool_call_format_error` で停止した。

今回の目的は、tool 名が明示されていなくても、引数形状から allowed tool が一意に決まる場合だけ XML fallback parser で復旧すること。状態制御や WorkMode 判定に新しいドメイン別ルールを足すのではなく、LLM 出力 protocol の局所的な壊れ方を parser の責務として扱う。

## 実装内容

- `parse_tool_call_object` が explicit `name` / `tool` を優先する挙動は維持。
- explicit name がない場合だけ、`arguments` の形状から tool 名を推定する。
- 推定候補が1つだけなら採用する。
- `pattern` / `query` / `glob` のように `Grep` と `Glob` が競合しうる形状は復旧しない。

引数形状の最小 mapping:

- `command` / `cmd`: `Bash`
- `path` + `old_string`/`old` + `new_string`/`new`: `Edit`
- `path` + `content`/`contents`/`body`/`text`: `Write`
- `path` only: `Read`
- `pattern` / `query` / `glob`: ambiguous unless only one matching tool is allowed

## Unit / deterministic 検証

通過:

- `cargo test --offline --lib infers_unterminated_write_call_from_argument_shape`
- `cargo test --offline --lib does_not_infer_ambiguous_pattern_tool_name`
- `cargo test --offline --lib extracts_edit_with_toml_array_header_in_string_argument`

追加した主な確認:

- p35 の実LLM raw に近い `{"arguments":{"path":"tests/password_strength.rs","content":"..."}}` を `Write` として復旧できる。
- `{"arguments":{"pattern":"*.rs"}}` は `Grep` / `Glob` のどちらかに勝手に倒さず、未抽出のまま残す。

## Actual local LLM 検証

同じ password strength TDD 依頼を `qwen3.6:27b-coding-mxfp8` で再実行した。

Command shape:

```text
target/debug/anvil --oneshot --fresh-session --no-footer --trace -y \
  -m qwen3.6:27b-coding-mxfp8 \
  --state-dir /private/tmp/anvil-p36-tool-name-infer-state \
  --cwd /private/tmp/anvil-p36-tool-name-infer-work \
  --max-iterations 14 --chat-timeout-secs 180 \
  -p "TDDで進めてください。まず tests/password_strength.rs に失敗するテストを書き、その後 src/lib.rs に password_score(password: &str) -> u8 を実装してください。期待仕様: 8文字未満は0、8文字以上で1、英小文字と英大文字を両方含むと+1、数字を含むと+1、記号を含むと+1。cargo test --manifest-path Cargo.toml が成功するまで進めてください。"
```

Result:

- `tool_call_format_error` では停止しなかった。
- 実LLM は `src/lib.rs` と `tests/password_strength.rs` を `Write` した。
- terminal outcome は `safe_stop_verifier_missing`。
- failure authority は `verifier_setup` / `verification_environment_failure`。

重要な観察:

- fresh run では p35 と同じ missing-name 出力は再発しなかったため、missing-name 復旧自体は p35 raw 由来の unit 回帰で確認した。
- p36 は、parser 変更後も難しめの Rust/TDD タスクで tool extraction と file write が前進し、次の停止点が parser ではなく setup/evidence 状態制御側に移ることを確認した。

## 次のボトルネック

p36 の最初の応答では LLM が `Cargo.toml` 作成を試みたが、ArtifactDirectedRecovery が `src/lib.rs` の missing implementation に限定していたため拒否された。その後 `src/lib.rs` と `tests/password_strength.rs` は作成されたが、`Cargo.toml` がないため verifier が選べず `safe_stop_verifier_missing` になった。

これは parser 問題ではなく、ObjectiveContract が test evidence を要求している場合に setup artifact を deliverable dependency として扱えていない問題である。

保守性の観点では、Rust 専用の Cargo.toml 例外を増やすより、EvidenceRunner が必要とする setup prerequisite を MissingDeliverableJob に渡し、次の allowed target にできる仕組みへ寄せるべきである。これは Node の `package.json`、Python の `pyproject.toml` / `requirements.txt`、docs の required heading、data の schema file にも同じ lifecycle で拡張できる。
