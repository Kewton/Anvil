# UltraPlan Generation Source Parity Implementation Result

作成日: 2026-06-29

## 実施範囲

`workspace/mvp/eval/021-1/ultra_plan_generation_source_parity_plan.md` と `ultra_plan_generation_source_parity_work_breakdown.md` の Phase 0〜8 を実施した。

対象にしたもの:

- UltraPlan generation system/user prompt
- profile generation rules
- invalid output retry
- planner tool-call rejection
- generated metadata normalization
- retry exhaustion fail-fast
- deterministic fallback の通常成功 path からの除去
- eval event / failure classification の補助
- unit / integration / targeted eval 確認

対象外のまま残したもの:

- phase-aware verification
- phase 間 context continuity
- runtime recovery / repair handoff
- final capability oracle 強化
- UltraPlan YAML schema 変更
- JSON UltraPlan parser への移行

## 主な変更

### UltraPlan generation

変更ファイル:

- `mvp/anvilminimal/src/planner/runner.rs`

変更内容:

- `generate_ultra_plan_with_ui` を system + user prompt に変更。
- `ULTRA_PLAN_GENERATION_ATTEMPTS = 3` の bounded retry を追加。
- planner tool call を invalid output として retry / fail-fast。
- parse 後に `goal/profile/style/intent` を request context で正規化。
- parse / lint retry exhaustion 後は `invalid generated UltraPlan after corrective retries` で fail-fast。
- `UltraPlan::deterministic(...)` を planner failure の通常成功 fallback として使わない。
- `ultra_plan_generation_*` events を追加。

### Profile rules

変更ファイル:

- `mvp/anvilminimal/src/planner/profile.rs`
- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`

変更内容:

- `profile_generation_rules(profile, intent)` を追加。
- Next.js create/fix/research/default の generation rules を追加。
- Next.js rules は prompt guidance に限定し、Space Invaders 固有 hard lint は入れていない。

### Lint

変更ファイル:

- `mvp/anvilminimal/src/planner/lint.rs`

変更内容:

- UltraPlan phase prompt が `/plan-run ...` のような REPL command だけでなく、`npm ...` など shell command そのものになるケースも拒否。
- これは profile 固有ではなく、phase prompt は natural-language /plan-run goal であるという汎用契約。

### Eval diagnostics

変更ファイル:

- `mvp/anvilminimal/scripts/eval-run.py`
- `mvp/anvilminimal/scripts/eval_lib/failure_classification.py`

変更内容:

- `extras_json` に UltraPlan generation attempt / retry / failed / tool-call rejection / metadata normalization count を出す。
- `invalid generated UltraPlan after corrective retries` を planner failure として分類。
- 過去の planner retry error より、後段の `step_verify_failure` / `ultra_phase_failed` を優先して分類できるように補正。

## テスト

実行済み:

```text
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::runner:: -- --nocapture
cargo test --manifest-path mvp/anvilminimal/Cargo.toml planner::lint:: -- --nocapture
python3 -m unittest mvp/anvilminimal/tests/eval/test_failure_classification.py
python3 -m unittest discover -s mvp/anvilminimal/tests/eval -p 'test_*.py'
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
python3 mvp/anvilminimal/scripts/eval-run.py --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml --model-profile speed-cloud --model-profiles mvp/anvilminimal/eval/model_profiles.yaml --modes ultra-plan-run --scenario nextjs-space-invaders-large --runs 1 --dry-run --run-root /private/tmp/anvilminimal-021-1-dry-run
```

結果:

- Rust 全体テスト: 322 passed
- Rust integration tests: all passed
- Python eval tests: 150 passed, 1 skipped
- release build: passed
- eval dry-run: passed

## 追加した主なテスト観点

- UltraPlan prompt が source parity rules と YAML shape を含む。
- Next.js profile rules が prompt に入る。
- prompt system 側に scenario-specific game terms を焼き込んでいない。
- invalid UltraPlan output は retry される。
- planner tool call は retry される。
- metadata echo 揺れは request context で正規化される。
- retry exhaustion 後に `.anvil/plans` が作成されない。
- UltraPlan phase prompt の shell command を lint で拒否する。
- UltraPlan generation failure が eval classifier で unclassified にならない。
- 後段 step verify failure が古い planner retry error に上書きされない。

## 実 LLM targeted eval

実行:

```text
python3 mvp/anvilminimal/scripts/eval-run.py --suite mvp/anvilminimal/eval/suites/mvp-smoke.yaml --model-profile openai-main-gemini-plan --model-profiles mvp/anvilminimal/eval/model_profiles.yaml --modes ultra-plan-run --scenario nextjs-space-invaders-large --runs 1 --parallel 1 --provider-limit 1 --timeout-sec 900 --binary mvp/anvilminimal/target/release/anvilminimal --run-root /private/tmp/anvilminimal-021-1-ultra-targeted-net
```

結果:

- success: false
- UltraPlan generation: succeeded on attempt 1
- UltraPlan phase count: 3
- metadata normalization: goal を正規化
- saved UltraPlan: deterministic fallback ではなく planner-generated plan
- failure stage: phase 2 execute
- direct failure: `step_verify_failure`
- failed command: `npm run build`
- error: `Cannot find module or type declarations for side-effect import of './globals.css'`

重要な解釈:

- 021-1 の主目的だった「UltraPlan 生成が deterministic fallback に落ちる」問題は、この targeted eval では再現していない。
- 残失敗は UltraPlan generation ではなく、plan-run / step runtime bridge と verify repair 側で発生している。
- これは 021-1 の非対象であるため、次段階で扱うべき。

## 残課題

今回の targeted eval で残った課題:

- phase 実行中の step verify failure を repair しきれていない。
- `global.d.ts` は存在するが、Next.js build の side-effect CSS import 型エラーを解消できていない。
- summary の旧 run では planner retry error が failure kind に見えたが、classifier 補正後は `step_verify_failure` として分類できる。

次に扱うべき領域:

- phase-aware verification
- step verify repair の target-specific edit 強化
- CSS side-effect import / global declarations の Next.js runtime bridge
- ultra phase failure の recovery handoff
