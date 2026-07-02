# 022-8 Provider Probe Gate / Tool Args Recovery 実装結果

作成日: 2026-06-30

## 対応範囲

- `tools/args_recovery.rs` を追加し、provider が出しやすい recoverable tool args shape を小さく正規化した。
- `ToolRegistry::execute` に防御的な args recovery を追加した。
- minimal loop で `tool_call_raw` と `tool_args_recovered` events を出すようにした。
- OpenAI / Gemini / Ollama native / Ollama XML fallback parser で tool args recovery を適用した。
- workspace confinement / hidden metadata access を recoverable retry 対象から外した。
- provider probe JSONL を `eval-run.py --provider-probe-results` で取り込み、`summary.eval.tsv` / `provider_probe_summary.json` / `events.jsonl` に接続した。
- `runtime_trace.py` で `provider_probe` と `tool_args_recovered` を lifecycle stage に写像した。
- `runtime_semantics_gate_matrix.md` と `parity_gate_report.json` に 022-8 の状態を反映した。

## Recoverable と拒否の境界

Recoverable:

- `arguments` / `args` / `payload` / `params` / `input` / `data` wrapper
- object に decode できる JSON string arguments
- `file` / `file_path` / `filepath` / `filename` -> `path`
- `body` / `text` / `contents` -> `content`
- `cmd` -> `command`
- `query` -> `pattern`

Hard reject:

- absolute path
- `..` を含む path
- NUL byte を含む path
- `.anvil` / `.git` / `.next` など hidden metadata への normal task access

## Gate 状態

G-S07 / G-S15 は `partial` のまま。

理由:

- live OpenAI / Gemini provider probe は今回の通常 test では実行していない。
- source/MVP same-condition trace diff は未実施。
- provider probe は runtime success ではなく、prompt/tool-call/provider-sensitive fix の観測証跡として扱う。

## 検証

実行済み:

- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml args_recovery -- --nocapture`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml recoverable_provider_aliases_are_executed -- --nocapture`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml unsafe_alias_path_is_not_recoverable -- --nocapture`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test live_provider provider_probe_parser_fixtures_cover_tool_argument_shapes -- --nocapture`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test live_provider provider_probe_ollama_xml_fallback_tool_like_output -- --nocapture`
- `python3 -m pytest mvp/anvilminimal/tests/eval/test_eval_cli_contract.py mvp/anvilminimal/tests/eval/test_failure_snapshot_classification.py`
- `python3 -m pytest mvp/anvilminimal/tests/eval`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --quiet`
- `python3 -c 'import json; json.load(open("workspace/mvp/eval/022/parity_gate_report.json")); print("parity_gate_report.json ok")'`

結果:

- Rust: 378 passed, integration tests も pass、ignored probe は通常通り skip。
- Python eval: 177 passed, 1 skipped。
- `parity_gate_report.json` は JSON として valid。

未実施:

- live OpenAI / Gemini provider probe。通常 test では API key / network を必須にしないため、任意実行扱い。
- source/MVP same-condition provider trace diff。

## Rollback

問題が出た場合は以下を戻す。

- `mvp/anvilminimal/src/tools/args_recovery.rs`
- `mvp/anvilminimal/src/tools/mod.rs` の module export
- provider parser への `recover_tool_arguments` 適用
- minimal loop の `tool_args_recovered` event emission
- `eval-run.py` の provider probe result attachment
- `run_summary.py` の provider probe columns
- `runtime_trace.py` の provider probe / args recovery event mapping
