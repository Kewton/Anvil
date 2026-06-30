# 022-2 Runtime Trace Capture Implementation Result

作成日: 2026-06-30

## 実装内容

022-2 の目的に対して、runtime semantics parity をコード読解だけでなく trace artifact で比較できる基盤を追加した。

追加/変更:

- `mvp/anvilminimal/scripts/eval_lib/runtime_trace.py`
  - raw event を normalized lifecycle stage へ写像する。
  - stage を G-S01〜G-S16 gate ids へ対応付ける。
  - failed run で per-run events がない場合、`silent_exit_without_events` を G-S14/G-S16 の gate failure として出す。
  - summary failure も `diagnostic_emitted` として trace に含める。
  - command の task prompt / secret を redaction して manifest に出す。
- `mvp/anvilminimal/scripts/eval-trace.py`
  - 既存 eval run root から trace manifest/report を後付け生成できる。
  - source/MVP trace report の stage/gate 差分 JSON を生成できる。
- `mvp/anvilminimal/scripts/eval-run.py`
  - eval 完了後に `runtime-semantics-normalized-events.jsonl`、`runtime-semantics-trace-report.json`、`runtime-semantics-trace-manifest.md` を自動生成する。
- `mvp/anvilminimal/tests/eval/test_runtime_semantics_trace.py`
  - normalized event sequence の fixture test。
  - silent exit without events の gate failure test。
  - manual/TUI `run_start` event の G-S16 trace evidence test。
  - prompt/API key 相当文字列の redaction test。
  - source/MVP trace report diff test。

## 出力 artifact

各 eval run root に以下を出力する。

| Artifact | 内容 |
| --- | --- |
| `runtime-semantics-normalized-events.jsonl` | normalized lifecycle stage / gate ids / failure kind |
| `runtime-semantics-trace-report.json` | subject / binary / stage counts / gate counts / silent exit count |
| `runtime-semantics-trace-manifest.md` | run ごとの redacted command / trace status |
| `runtime-semantics-trace-diff.json` | `eval-trace.py` の compare mode で出力 |

## Gate 状態

今回の実装で、以下は「証跡を出せる状態」になった。

- G-S05 phase context continuity
- G-S06 step prompt construction
- G-S14 diagnostics
- G-S16 TUI/manual run observability

ただし、source same-condition trace と manual TUI trace は未登録であるため、該当 gate は `partial` のままとする。trace writer の存在だけで pass にはしない。

## 検証

実行済み:

```bash
python3 -m pytest mvp/anvilminimal/tests/eval/test_runtime_semantics_trace.py
python3 -m pytest mvp/anvilminimal/tests/eval
```

結果:

- `test_runtime_semantics_trace.py`: 5 passed
- `mvp/anvilminimal/tests/eval`: 171 passed, 1 skipped

その後、MVP と `anvildev --engine minimal` の同条件 eval を実行し、両方の `runtime-semantics-trace-report.json` から `runtime-semantics-trace-diff.json` を生成する。
