# Ultra Phase Context Continuity Eval Result

作成日: 2026-06-29

## 実行済み検証

対象実装直後に以下を実行した。

- `cargo check --manifest-path mvp/anvilminimal/Cargo.toml`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml --test tui_integration tui_ultra_plan_run_smoke_fake_clients`
- `python3 -m unittest mvp/anvilminimal/tests/eval/test_runtime_scoring.py`

結果はいずれも成功。

## 追加確認内容

`tui_ultra_plan_run_smoke_fake_clients` では以下を確認した。

- ultra-plan-run は 2 phase を完了する。
- planner request は 2 回で、message history は共有されない。
- execution request は 2 回で、phase 2 の request message 数が phase 1 より増える。
- phase 2 の execution request に phase 1 の成果物内容が残る。

`test_runtime_scoring.py` では以下を確認した。

- `ultra_context_initialized` / `ultra_phase_context_attached` / `ultra_phase_context_updated` から `ultra_context_continuity_score` が算出される。
- shared session、phase 2 context attachment、bounded context、session message growth、partial outcome 記録を subscore として分解できる。

## Targeted Live Eval

ネットワーク許可付きで MVP の targeted eval を 1 run 実行した。

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml \
  --model-profiles mvp/anvilminimal/eval/model_profiles.yaml \
  --model-profile openai-main-gemini-plan \
  --modes ultra-plan-run \
  --scenario nextjs-space-invaders-large \
  --runs 1 \
  --parallel 1 \
  --provider-limit 1 \
  --context-budget 65536 \
  --binary mvp/anvilminimal/target/release/anvilminimal \
  --binary-kind anvilminimal \
  --run-root /private/tmp/anvilminimal-021-2-ultra-context-live \
  --timeout-sec 1200
```

結果:

- success: 0/1
- failure_layer: bridge
- failure kind: `step_verify_failure`
- stderr: `phase setup-project-configuration profile verification failed: ProfileContractFailed("src/app/page.tsx uses browser/client APIs and must start with \"use client\"")`
- `ultra_context_continuity_score`: 100.0
- `ultra_shared_session_observed`: 100.0
- `ultra_context_attached_after_first_phase`: 100.0
- `ultra_context_bounded`: 100.0
- `ultra_session_message_growth_observed`: 100.0
- `ultra_partial_outcome_recorded`: 100.0

この run は accepted artifact までは到達していない。ただし 021-2 の主対象である shared execution session / bounded context event の観測は成功した。

失敗原因は phase context continuity ではなく、non-final phase で profile verification が final completeness / client boundary を要求して停止する既知の別論点である。これは 021-2 の対象外であり、次フェーズでは phase-aware profile verification として扱うべきである。

## 未実施 / 後続確認

manual TUI UAT は未実施。Phase 7 の live eval では context continuity の観測はできたが、手動 TUI 表示・ログ保存の確認は別途必要。
