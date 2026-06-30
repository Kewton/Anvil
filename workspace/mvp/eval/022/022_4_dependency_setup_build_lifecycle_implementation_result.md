# 022-4 Dependency / Setup / Build Lifecycle Implementation Result

作成日: 2026-06-30

## 目的

`workspace/mvp/eval/022/README.md` の 022-4 に従い、MVP anvilminimal の dependency/setup/build lifecycle を source semantics に寄せた。

## Source 参照

- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_request_helpers.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/node_runner_manifest.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_probe.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/project_verifier.rs`
- `/Users/maenokota/share/work/github_kewton/Anvil-develop/src/agent/loop_run/verifier_command_policy.rs`

## 実装内容

### 1. Node dependency setup kind の追加

`mvp/anvilminimal/src/minimal_loop/dependency_setup.rs` に `NodeDependencySetupKind` を追加した。

- `next_build_dependencies`
- `node_test_runner_manifest`

これにより、Next build dependency setup と Node test runner manifest setup を同じ lifecycle に載せつつ、setup の意味を event 上で区別できる。

### 2. Node test runner manifest completion の追加

source の `node_runner_manifest.rs` に寄せ、以下を deterministic setup として実装した。

- `tests/*.test.js` / `test/*.spec.js` / `__tests__` などの Node test artifact を検出
- `package.json` がない場合は minimal manifest を作成
- `package.json` があり `scripts.test` がない場合は `node --test` を追加
- malformed manifest や既に bindable な manifest は clobber しない
- network install は実行しない

### 3. build/test verifier lifecycle の統合

`mvp/anvilminimal/src/minimal_loop/build_verifier.rs` を拡張し、`npm test` / `npm run test` / `pnpm test` / `yarn test` を build verifier lifecycle に含めた。

これにより以下が同じ `dependency_build_lifecycle` event taxonomy で記録される。

- dependency boundary check
- setup blocked
- setup passed
- build/test rerun
- verification passed / failed / dependency_missing

### 4. planner/minimal completion verify の早期 return を除去

`planner/verify.rs` と `minimal_loop/completion.rs` で、`npm test` かつ `package.json` missing の場合に即 `dependency_missing` へ落としていた早期判定を外した。

現在は `BuildVerifierLifecycleObservation` を経由するため、plan-run / ultra-plan-run / minimal-loop の event taxonomy が揃う。

### 5. trace normalizer の補正

`runtime_trace.py` の `dependency_build_lifecycle` stage 判定を修正した。

旧挙動では `lifecycle_stage=dependency_setup_build` の文字列だけで setup attempted 扱いになり得た。現在は `setup_attempted=true` または lifecycle stages の `setup_passed/setup_attempted` を見て `dependency_setup_attempted` に写像する。

## 追加 fixture

### Rust

- dependency missing -> setup blocked
  - `node_test_runner_missing_manifest_setup_blocked_records_lifecycle`
  - `node_test_runner_manifest_setup_is_blocked_without_authority`
- dependency missing -> setup allowed -> rerun
  - `node_test_runner_setup_allowed_then_test_rerun_records_lifecycle`
  - `node_test_runner_manifest_setup_creates_package_manifest_without_network`
  - `node_test_runner_manifest_setup_adds_script_without_clobbering_manifest`
- setup-only / manifest-only rejection
  - 既存の `setup_only_does_not_satisfy_implementation_obligation`
  - 既存の `manifest_only_nextjs_build_verify_is_not_success`
- plan-run / ultra-plan-run / minimal-loop shared event taxonomy
  - `dependency_build_lifecycle_event_uses_same_taxonomy_for_modes`
  - `plan_run_emits_dependency_build_lifecycle_event`
  - minimal-loop completion verify emits `dependency_build_lifecycle`

### Python

- `test_runtime_semantics_trace.py` の dependency lifecycle normalizer fixture を更新

## Gate 更新

G-S09 は `fail` から `partial` に移動した。

理由:

- lifecycle 実装と fixture は入った
- `cargo test` / `pytest` は通過
- ただし same-condition eval smoke と source/MVP normalized trace diff は未実施
- release-grade UAT/browser evidence も未登録

## 検証

実行済み:

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
python3 -m pytest mvp/anvilminimal/tests/eval
python3 -m pytest mvp/anvilminimal/tests/eval/test_parity_gate_report.py mvp/anvilminimal/tests/eval/test_runtime_semantics_trace.py
python3 -m json.tool workspace/mvp/eval/022/parity_gate_report.json
```

結果:

- cargo test: pass
- pytest eval: pass
- targeted pytest: pass
- parity gate report JSON validation: pass

## 残課題

- G-S09 の same-condition MVP smoke / anvildev trace comparison は未実施
- G-S10 repair targeting、G-S12 final acceptance は引き続き fail
- G-S09 を pass にするには、fresh eval trace で stage regression がないこと、かつ dependency lifecycle が plan-run / ultra-plan-run / minimal-loop で同じ taxonomy として観測されることを確認する必要がある
