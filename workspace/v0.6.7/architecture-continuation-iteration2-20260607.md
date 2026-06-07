# v0.6.7 Architecture Continuation Iteration 2 - 2026-06-07

## Scope

`future-architecture-direction-20260607.md` と前回の
`architecture-continuation-validation-20260607.md` を受けて、Rust TDD の
`repair_exhausted` 収束不全をさらに小さく検証した。

今回も実際のローカル LLM を使った。モデルは Ollama の `qwen3.5:9b` /
`qwen3.5:2b`、対象は Rust library の `is_palindrome` TDD タスクである。

## Implemented Small Steps

### 1. Diagnostic target rebind after semantic planning

Diagnostic LLM が `Cargo.toml` / setup を正しく選んでも、修復実行側の
`CorrectionJob` が古い `src/lib.rs` / implementation に残る問題を修正した。

`RepairJob::sync_correction_job_from_current_assessment` を追加し、semantic plan
の採用と legacy assessment の rebind 後に、現在の assessment から
`CorrectionJob` を再構築するようにした。

この変更により、diagnostic target authority が repair prompt / admission へ
同期される。

### 2. Role-scoped failure kind admission

`invalid_manifest` は setup 修復であることが多いが、従来の unscoped mapping では
implementation 修復扱いになり、`Cargo.toml` target が採用されにくかった。

`diagnostic_target_role_matches_failure_kind` を role-scoped
`legacy_kind_to_allowed_change_kind_for_role` へ寄せ、failure kind と target role
の整合判定を contract-aware にした。

これは task kind 固定分岐ではなく、failure kind と artifact role の typed
admission である。

### 3. Diagnostic JSON salvage for truncated semantic tail

実LLMの diagnostic reply は、legacy fields が正しくても
`failure_clusters` を繰り返し、末尾で truncation するケースがあった。

今回、legacy diagnostic fields が揃っている場合は、壊れた semantic tail を捨てて
legacy diagnosis を採用できるようにした。また、配列内 object の閉じ `}` が抜けた
まま次の array/object へ進むケースも、JSON stack に基づく小さな補修で受けられる
ようにした。

これは特定タスクの文言マッチではなく、diagnostic schema の shape drift に対する
境界正規化である。

### 4. Rejected attempt scope

過去 target の malformed patch rejected が残り、新しい active target の修復を
即座に blocked にする問題を修正した。

`repeated_rejected_attempt` は現在の repair attempt key に一致する rejection だけを
数える。これにより、`tests/palindrome.rs` の失敗履歴が `Cargo.toml` や
`src/lib.rs` の新しい修復計画を止めない。

## Actual LLM Validation

### Run 1: truncated diagnostic semantic tail

Session: `/private/tmp/anvil-v067-tdd-5`

結果:

- Generated test file contained invalid Rust syntax, `# Palindrome Tests`
- `cargo test` failed at syntax parse
- Diagnostic LLM selected `tests/palindrome.rs`
- But reply repeated `failure_clusters` until length truncation

インサイト:

- LLM は target meaning を概ね取れていた
- failure was not model reasoning alone; parser boundary could not preserve legacy diagnosis
- led to diagnostic semantic tail salvage

### Run 2: missing close before truncated clusters

Session: `/private/tmp/anvil-v067-tdd-6`

結果:

- `Cargo.toml` had `[[test]] path` but no `name`
- Sidecar reply had malformed semantic-only JSON
- Main reply contained usable legacy fields but an unclosed target object before
  `failure_clusters`

インサイト:

- local LLM can provide enough structured intent, but strict JSON acceptance loses it
- schema boundary should recover a complete legacy diagnosis when semantic extras fail
- led to object-close salvage before array end

### Run 3: stale correction target

Session: `/private/tmp/anvil-v067-tdd-7`

結果:

- Diagnostic LLM selected `Cargo.toml` / setup
- Accepted repair action still targeted `src/lib.rs` / implementation
- Run ended `repair_exhausted`

インサイト:

- diagnostic target authority did not propagate into `CorrectionJob`
- prompt improvements alone cannot fix this; controller state was stale
- led to `sync_correction_job_from_current_assessment`

### Run 4: setup target accepted, then blocked by old rejection ledger

Session: `/private/tmp/anvil-v067-tdd-8`

結果:

- Accepted action finally targeted `Cargo.toml` / setup
- But previous malformed patch rejections on another target still caused repair exhaustion

インサイト:

- target rebind worked
- rejection ledger needed active-target scoping
- led to current-attempt-key scoped rejected attempt counting

### Run 5: progress moved to repair editor stability

Session: `/private/tmp/anvil-v067-tdd-9`

結果:

- Initial failure moved to integration-test import mismatch
- Diagnostic accepted `tests/palindrome.rs` / test as first target
- Later diagnostic moved to `src/lib.rs` / implementation
- Repair editor repeatedly emitted almost-correct edit JSON with syntax defects:
  missing quoted `new_string` key or one extra trailing `]`
- Run still ended `repair_exhausted`

インサイト:

- target authority and rejection scoping improved enough to move across targets
- current bottleneck is now repair editor proposal format stability and cheap-check
  admission, not only diagnostic target selection
- difficult Rust TDD remains failing, so success-rate gain is not yet guaranteed

## Verification

Unit checks:

- `cargo test verifier_assessment_parser --lib`
- `cargo test diagnostic_parser_recovers_legacy_fields_from_truncated_failure_clusters --lib`
- `cargo test diagnostic_parser_recovers_missing_target_object_close_before_truncated_clusters --lib`
- `cargo test invalid_manifest_diagnostic_allows_setup_target_role --lib`
- `cargo test correction_job_sync_uses_rebound_assessment_target --lib`
- `cargo test repair_job_repeated_rejects_do_not_block_different_active_target --lib`
- `cargo check --lib`
- `cargo build`

All passed.

## Architecture Assessment

目指す方向には近づいた。

理由は、追加した処理が task-specific な if 文ではなく、次の typed boundary に
収まっているため。

- LLM diagnostic output
- Diagnostic parser normalization
- Semantic repair plan admission
- RepairJob state synchronization
- Rejected attempt ledger policy

一方で、まだ十分ではない。

Rust TDD の実LLM検証では、`repair_exhausted` は解消していない。失敗点は
diagnostic target から repair editor proposal へ移動しただけである。

## Next P0 Hypotheses

### H1: Repair proposal parser needs a generic normalization boundary

LLM は意味としては近い edit proposal を出しているが、JSON syntax が少し崩れて
rejected されている。

ただし、`new_string` だけを個別補修するような処理を増やすとルールベース化する。
次は repair proposal schema 全体の extraction / normalization boundary を
diagnostic parser と同じ設計で作るべきである。

判断基準:

- top-level object / array extraction
- one extra closing array token の除去
- object key quote 欠落は、schema key set に限定して最小補修できるか検証する
- 補修後も typed validation と cheap check は必須

### H2: Rust integration-test import repair should be evidence-guided

integration test で `use crate::is_palindrome` が失敗した場合、修正候補は
`use <package_name>::is_palindrome` である。

これは task-specific prompt ではなく Rust test framework semantics である。
既存の Rust binding repair がこのケースを扱えるかを先に確認し、足りなければ
framework finding として repair prompt へ渡す。controller が直接書き換える
処理は最小に留める。

### H3: Hard-task validation set must remain mandatory

docs / data / ops が動いても、coding / TDD / feature improvement が失敗するなら
成功率改善は限定的である。

今後の各 iteration は、少なくとも次を含める。

- docs section task
- data schema task
- ops command observation task
- Node CLI coding task
- Rust TDD integration-test task
- existing-code feature improvement task

## Direction Update

P0 は引き続き `ObjectiveContract` / `ControllerStatePacket` による typed control
である。ただし今回の結果から、次の順に進める。

1. repair proposal parsing / normalization を diagnostic parser と同じ責務境界へ
   分離する
2. cheap-check failure と malformed proposal を同じ rejection bucket に入れず、
   current target / failure class ごとに再計画できるようにする
3. Rust / Node の framework semantics は prompt 文言ではなく framework findings
   として渡す
4. 実LLM validation matrix を小さく継続し、成功率だけでなく terminal reason と
   repair target transition を記録する

現時点で、方向性は妥当。ただし今回の対応だけで成功率が大きく上がるとは見ない。
期待できる改善は、diagnostic target が正しくても stale state で別 target に戻る
失敗を減らすこと。hard TDD の最終 pass には repair proposal boundary の次段対応が
必要である。
