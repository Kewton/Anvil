# Phase 017 implementation result

## Summary

`minimal_loop_anvildev_gap_root_cause_countermeasure.md` と
`minimal_loop_anvildev_gap_work_breakdown.md` の Phase 0〜9 を実施した。

主な結論:

- MVP minimal-loop の false positive は focused eval 上で 0 まで抑制できた。
- MVP minimal-loop の acceptance success は直近 trend で 9〜10/12。
- anvildev は同条件 11/12 だが、acceptance false positive が 2 件残っている。
- MVP は「誤って成功扱いしない」方向に改善した一方、OpenAI 経路で verify repair が無編集停滞するケースが残る。

## Implemented Phases

| Phase | Status | 実施内容 |
| --- | --- | --- |
| Phase 0 | Done | 失敗傾向を fixture/test と live eval で再現可能にした |
| Phase 1 | Done | `CompletionContract` に required capabilities / deterministic oracles / required evidence を追加 |
| Phase 2 | Done | `minimal_loop::evidence` を追加し、runtime evidence gate を pure function として実装 |
| Phase 3 | Done | JS/Python/Rust の deterministic test/check evidence と weak evidence を検出 |
| Phase 4 | Done | weak postcheck / missing evidence を completion verify failure として扱うようにした |
| Phase 5 | Done | Next.js profile に dependency coherence、client component boundary、CSS declaration、tsconfig/Tailwind 契約を追加 |
| Phase 6 | Done | required evidence 不足時の bounded repair feedback と event を追加 |
| Phase 7 | Done | plan-run / ultra-plan-run の completion contract handoff に required capabilities/evidence を横展開 |
| Phase 8 | Done | eval summary に required evidence、runtime acceptance、failure layer を出力 |
| Phase 9 | Done | MVP/anvildev focused eval 比較と残課題分析を実施 |

## Code Changes

主な変更ファイル:

- `mvp/anvilminimal/src/minimal_loop/evidence.rs`
- `mvp/anvilminimal/src/minimal_loop/completion.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/scripts/eval-run.py`
- `mvp/anvilminimal/scripts/eval_lib/run_summary.py`
- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`
- `mvp/anvilminimal/tests/eval/test_completion_contract_snapshots.py`

## Verification

Static/unit verification:

```bash
cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml --check
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
python3 -m unittest discover mvp/anvilminimal/tests/eval -q
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

結果:

- Rust tests: 296 passed
- Python eval tests: 132 passed, 1 skipped
- Release build: success

## Focused Eval Results

条件:

- suite: `mvp/anvilminimal/eval/suites/mvp-acceptance.yaml`
- model profile: `speed-cloud`
- local LLM: unused
- mode: `minimal-loop`

| Run | Binary | Result | false positive | Notes |
| --- | --- | ---: | ---: | --- |
| `/private/tmp/anvilminimal-eval-017-mvp-minimal-r3-net` | MVP | 10/12 | 0 | Next.js Gemini success, Next.js OpenAI stopped as verify repair no-change |
| `/private/tmp/anvilminimal-eval-017-mvp-minimal-r4-net` | MVP | 9/12 | 0 | Next.js 2/2 success, Python linter 2/2 verify repair failure |
| `/private/tmp/anvilminimal-eval-017-anvildev-minimal-r1-net` | anvildev | 11/12 | 2 | success column remains legacy-oriented; acceptance still flags two false positives |
| `/private/tmp/anvilminimal-eval-017-mvp-python-focused-r1-net` | MVP | 1/2 | 0 | dependency_missing 誤分類は解消。OpenAI は assertion failure から無編集停止 |

## Current Failure Trend

### MVP remaining failures

1. `fix-js-date-helper-small` / OpenAI
   - `stop_reason=verify_repair_no_change`
   - `runtime_acceptance_primary_reason=missing_required_evidence:bound_verify_command`
   - 実装はあるが assertion-backed self-test がない。

2. `python-markdown-linter-medium` / OpenAI variance
   - `stop_reason=verify_repair_no_change` または `verify_repair_progress_unchanged`
   - `source_semantic_success=true`
   - verify command の assertion failure から repair edit に進めない場合がある。
   - `not found in ...` を dependency_missing と誤分類する問題は修正済み。

3. `nextjs-space-invaders-large` / OpenAI variance
   - r3 では CSS declaration repair 後に無編集停止。
   - r4 では 2 provider とも success。
   - Next.js profile に `"use client"` boundary と Next 14.0.x rejection を追加済み。

### anvildev remaining issues

anvildev は 11/12 success だが、acceptance false positive が 2 件ある。

- JS date helper OpenAI: legacy success だが deterministic test capability 不足。
- Next.js OpenAI: legacy success だが static-title-only と判定。

そのため、単純な legacy success では anvildev が高く見えるが、acceptance 基準では MVP の方が誤成功抑止が強い。

## Root Cause Updates

今回新たに確認した根本原因:

- MVP の旧 completion gate は required artifact existence に寄りすぎていた。
- eval の `functional_contract` が runtime completion contract に入っていなかった。
- weak verify command が deterministic evidence と同一視されていた。
- Next.js profile が App Router の client boundary と known-risk dependency range を十分に見ていなかった。
- verify command failure の stderr 分類で、`AssertionError: ... not found in ...` を dependency missing と誤認する広すぎる文字列判定があった。

## Residual Work

今回の Phase 017 で根本的な false positive 抑止は改善したが、成功率を anvildev 以上へ安定させるには次が必要。

1. verify repair feedback から無編集で止まる provider 挙動への対策。
2. assertion failure の target/excerpt をさらに短く明確に提示する修復誘導。
3. JS self-test 不足時に、実装ファイルへ bounded self-test を追加する具体例の改善。
4. Next.js profile failure が出たときの deterministic small repair hook の検討。

これらは eval scenario 名ではなく、language / profile / evidence / command failure shape に基づいて進める。
