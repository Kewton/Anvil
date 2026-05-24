# v0.4.16 作業計画

## 前提

v0.4.16 の主目的は、成功率を一気に上げることではない。

目的は verifier repair を、場当たり的な retry / deterministic patch の集合から、`RepairJob` が管理する lifecycle へ移すこと。

そのため、作業順序は「既存処理を削る」より先に「新しい中心構造を作り、旧処理を接続し直す」順にする。

## Phase 0: freeze inventory を確定する

成果物:

- `01-freeze-inventory.md`
- deterministic repair 発火箇所の一覧
- production main path / fallback / adapter / safety boundary の分類

完了条件:

- `turn.rs` にある Python/pytest 固有処理の扱いが分類済み。
- 追加で pattern repair を増やさない方針が明文化されている。

## Phase 1: RepairJob event model を追加する

追加する概念:

- `RepairJobEvent`
- `RejectedAttempt`
- `VerifierDelta`
- `RepairTerminalReason`
- `RepairNextAction`

ポイント:

- 既存 `RepairJob` を巨大化させすぎない。
- まずは event と transition の SSOT を作る。
- `turn.rs` の loop 分岐はすぐ全移植しない。

テスト:

- same reject repetition -> re-plan or safe stop
- malformed diagnostic -> retry until budget
- authority ambiguity -> safe stop
- verifier improved -> continue same plan
- verifier unchanged -> do not repeat same patch family

実施状況:

- 完了。
- `RepairJobEvent` / `RejectedAttempt` / `VerifierDelta` / `RepairTerminalReason` / `RepairNextAction` を追加した。
- `RepairJob::apply_event()` と `RepairJob::next_action()` の最小状態機械を追加した。
- 既存 verifier repair フローから patch applied / verifier observed / patch rejected を event として記録する接続を追加した。
- production main path の判断はまだ完全移行していない。次フェーズで `verifier_repair_decision` 側の分岐を段階的に `next_action()` へ寄せる。

## Phase 2: RepairPlanProposal / AcceptedRepairPlan を分ける

追加する概念:

- `RepairPlanProposal`
- `AcceptedRepairPlan`
- `RepairPlanValidationError`

方針:

- diagnostic LLM の JSON は proposal。
- Anvil の validation 後だけ accepted plan。
- patch proposal は accepted plan がないと生成できない。

テスト:

- source of truth なしの test expectation repair は reject。
- setup repair が implementation target を選んだら reject。
- verifier observation だけを authority にした plan は reject。
- ambiguity がある plan は safe stop または clarification report。

実施状況:

- 完了。
- `repair_plan.rs` を追加し、diagnostic LLM の出力をまず `RepairPlanProposal` として受ける境界を作った。
- `AcceptedRepairPlan` は `repair_authority` の authority validation を通過した場合だけ生成するようにした。
- 既存 `validate_verifier_repair_plan_admission()` は直接 authority action を作らず、proposal validation 経由に切り替えた。
- patch generation / deterministic fallback の main path 分離はまだ未完了。次フェーズで `PatchProposalProvider` 境界へ進める。

検証:

- `cargo test --lib repair_plan` pass。
- `cargo test --lib repair_job_` pass。
- `cargo test --lib` pass: 2866 tests。
- `cargo clippy --all-targets -- -D warnings` pass。
- `cargo build --release` pass。

## Phase 3: PatchProposalProvider 境界を作る

追加する概念:

- `PatchProposalProvider`
- `PatchProposal`
- `PatchAdmission`

provider の種類:

- main LLM edit provider
- diagnostic LLM assisted patch provider
- deterministic fallback provider

制約:

- provider は target / authority / allowed change kind を選ばない。
- provider は accepted `RepairStep` に対する具体 patch だけ返す。
- deterministic pytest repair はここに降格する。

実施状況:

- 部分完了。
- `patch_provider.rs` を追加し、`PatchProviderKind` / `PatchProviderRequest` / `PatchProviderOutput` / `PatchAdmission` を定義した。
- provider output は `AcceptedRepairPlan` と target contents に対して admission される形にした。
- LLM patch proposal の shadow validation を `AcceptedRepairPlan` ベースの provider admission 経由に変更した。
- まだ legacy validation が最終判定であり、deterministic fallback の全経路は provider 化されていない。次フェーズで deterministic candidate を provider admission の後段へ降格する。

検証:

- `cargo test --lib patch_provider` pass。
- `cargo clippy --all-targets -- -D warnings` pass。

## Phase 4: deterministic repair の main path 降格

対象:

- missing pytest import
- bare setup state reset
- provider mutable state isolation
- observed literal expectation update
- synthesized missing implementation target

作業:

- `RepairStep` なしで発火しないようにする。
- 発火時は telemetry に `provider_kind=deterministic_fallback` を出す。
- `turn.rs` から runtime-specific parser を runtime adapter module へ移す準備をする。

注意:

- すぐ削除しない。
- greenfield target synthesis は task contract / runtime adapter 経由に置き換える。

実施状況:

- 部分完了。
- controller deterministic repair candidate は、`AcceptedRepairPlan` の後に `PatchProviderKind::DeterministicFallback` として provider admission を受ける形へ寄せた。
- provider admission が失敗した deterministic candidate は即適用せず、既存 LLM repair path へフォールスルーする。
- telemetry に `provider_kind=deterministic_fallback` を出すようにした。
- Python/pytest 固有 candidate 自体はまだ `turn.rs` に残っている。削除ではなく、次段階で runtime adapter / fallback module へ移す。

検証:

- `cargo test --lib patch_provider` pass。
- `cargo clippy --all-targets -- -D warnings` pass。

## Phase 5: terminal reason を整備する

追加する terminal:

- `verified_done`
- `ambiguous_spec_safe_stop`
- `no_safe_repair_target`
- `repair_budget_exhausted`
- `diagnostic_unavailable`
- `patch_rejected_repeatedly`
- `verifier_unavailable`

表示:

- user-facing message は短く。
- dev log / run metadata は詳細に。

実施状況:

- 部分完了。
- `RepairTerminalReason` は Phase 1 で追加済み。
- `RepairTerminalReason::safe_stop_reason()` を追加し、既存 `StopReason` への投影を明示した。
- まだ production dispatch は既存 safe stop emit path が主体。次段階で `RepairJob::next_action()` の `SafeStop` を terminal dispatch の入口へ寄せる。

検証:

- `cargo test --lib repair_terminal_reason_projects_to_existing_safe_stop_reason` pass。

## Phase 6: 評価

順序:

1. state transition unit tests
2. synthetic verifier failure tests
3. diagnostic LLM contract tests
4. PAM なし smoke 5 回
5. 改善が見えたら PAM なし 20 回 / PAM あり 20 回

中断条件:

- 同じ structural failure が再現したら大量評価へ進まない。
- test weakening が validator を通ったら即修正。
- `assistant stopped before repairing` 系が残る場合は terminal reason を先に直す。

実施状況:

- 最小評価まで完了。
- state transition / plan boundary / provider boundary の単体テストを追加した。
- 大量 E2E 評価はまだ実施していない。production dispatch の全面移行前に 40 回評価へ進むのは早い。

検証結果:

- `cargo fmt --check` pass。
- `cargo clippy --all-targets -- -D warnings` pass。
- `cargo test --lib` pass: 2870 tests。
- `cargo build --release` pass。

## 実装上の注意

- `turn.rs` をさらに肥大化させない。
- 新規モジュールは小さく切る。
- runtime-specific logic は adapter に閉じ込める。
- LLM / PAM / verifier output / README は常に untrusted。
- 成功率だけでなく safe stop quality を測る。
