# 022-9 Comparative / Release Gate Operation 実装結果

作成日: 2026-06-30

## 対応範囲

- `eval_lib/parity_gate.py` に comparative / release gate 用の report 生成・閾値判定を追加した。
- `eval-preflight.py` に opt-in の parity gate 引数を追加した。
- MVP / anvildev の `summary.eval.tsv` を読み取り、同条件比較結果を `parity_gate_report.json` に出力できるようにした。
- release gate では `uat_evidence_paths` / `browser_readiness_evidence_paths` / `interaction_evidence_paths` / `tui_run_event_paths` が揃わない限り full pass にしない。
- `mvp-smoke.yaml` に comparative gate metadata を追加した。
- `mvp-provider-smoke.yaml` に network/provider probe gate metadata を追加した。
- `parity_gate_report.json` に 022-9 の comparative gate operation と rollback policy を反映した。

## 追加 CLI

`eval-preflight.py` に以下を追加した。

```bash
python3 mvp/anvilminimal/scripts/eval-preflight.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profile speed-cloud-5x \
  --gate-level comparative \
  --mvp-summary <mvp-summary.eval.tsv> \
  --anvildev-summary <anvildev-summary.eval.tsv> \
  --write-parity-gate-report <parity_gate_report.json>
```

Release gate では以下も渡す。

```bash
  --gate-level release \
  --uat-evidence <uat.md> \
  --browser-evidence <browser-readiness.json> \
  --interaction-evidence <interaction.json> \
  --tui-events <events.jsonl>
```

## 判定ルール

- MVP success rate が anvildev より 5pp 以上低い場合は warning。
- MVP success rate が anvildev より 10pp 以上低い場合は failure。
- 10pp 以上低くても intentional difference evidence がある場合は `intentional_difference` として扱う。
- success rate だけでなく、failure layer / failure kind / acceptance false positive / release gate evidence を report に残す。
- `--offline-ok` は API/network preflight の bypass には使えるが、明示 opt-in した parity gate failure は bypass しない。

## Rollback 方針

- 閾値 gate が重すぎる場合は fail を warning に落とせる。
- ただし blank failure kind の禁止は戻さない。
- build-only / title-only interactive app false positive の禁止は戻さない。
- comparison report と release evidence fields は残す。

## 現在の Gate 状態

G-S01〜G-S16 は `partial` のまま。

理由:

- 022-9 は運用 gate を実装した段階で、最新の MVP / anvildev same-condition comparison はまだ添付していない。
- release-grade browser / interaction / TUI run evidence も未添付。

## 検証

実行済み:

- `python3 -m pytest mvp/anvilminimal/tests/eval/test_parity_gate_report.py mvp/anvilminimal/tests/eval/test_eval_cli_contract.py`
- `python3 -m pytest mvp/anvilminimal/tests/eval`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --quiet`
- `python3 -m py_compile mvp/anvilminimal/scripts/eval-preflight.py mvp/anvilminimal/scripts/eval_lib/parity_gate.py`
- `python3 -c 'import json; json.load(open("workspace/mvp/eval/022/parity_gate_report.json")); print("parity_gate_report.json ok")'`

結果:

- Targeted pytest: 13 passed。
- Python eval: 182 passed, 1 skipped。
- Rust: 378 passed、integration tests も pass。
- `parity_gate_report.json` は JSON として valid。

未実施:

- 最新 MVP / anvildev same-condition cloud comparison。
- release-grade browser / interaction / TUI run evidence の添付。
