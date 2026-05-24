# v0.4.16 Repair Pipeline Rebuild Plan

## 目的

v0.4.15 までの修正で、成果物生成から verifier 到達までは前進した。
一方で、verifier repair はまだ安定収束していない。

v0.4.16 では、個別 failure pattern をさらに増やすのではなく、repair phase を次の原則で整理する。

- 成功だけを正解にしない。
- verifier pass と actionable safe stop を明確に分ける。
- verifier failure を retry trigger ではなく `RepairJob` の状態遷移として扱う。
- LLM は意味診断と狭い patch 生成に使い、最終判断は Anvil が行う。
- 旧 deterministic repair はすぐ削除せず、production main path から段階的に降格する。
- 評価は E2E 大量実行の前に、状態遷移と synthetic failure で固める。

## セクション別成果物

| section | file | purpose |
| --- | --- | --- |
| 1 | `01-freeze-inventory.md` | 現状差分を採用候補 / 暫定候補 / 削除候補に分類する。 |
| 2 | `02-completion-goals.md` | 完了条件を verifier pass / actionable safe stop に再定義する。 |
| 3 | `03-repair-job-state-machine.md` | `FailurePacket -> RepairPlan -> PatchProposal -> VerifierDelta` の中心構造を定義する。 |
| 4 | `04-llm-role-boundaries.md` | main LLM / diagnostic LLM / Anvil / PAM の責務境界を定義する。 |
| 5 | `05-deterministic-repair-demotion.md` | 旧 deterministic repair の降格・観測・削除条件を定義する。 |
| 6 | `06-evaluation-method.md` | 評価手順を段階化し、無駄な大量評価を避ける。 |

## 現時点の判断

現在の課題は FastAPI CRUD 固有ではない。

根本原因は、verifier repair において source of truth、repair target、patch、validator reject、rerun delta が 1 つの lifecycle に閉じていないこと。

そのため、次の方針を採る。

1. Python/pytest の個別 deterministic repair をこれ以上増やさない。
2. 既存の Python/pytest 補修候補は runtime adapter / fallback として扱い、main decision から切り離す。
3. `RepairJob` を唯一の repair lifecycle owner にする。
4. LLM diagnostic は structured `RepairPlan` proposal を返すだけにする。
5. Anvil は authority / safety / retry budget / terminal condition を機械的に決める。

## 期待する改善

短期的には、成功率だけでなく `actionable safe stop` 率の向上を重視する。

中期的には、PAM なしでも「失敗理由が明確で、同じ無効 repair を繰り返さない」状態を作る。

長期的には、Python/FastAPI 以外の runtime でも同じ repair pipeline を使えるようにし、runtime 固有処理は adapter として閉じ込める。

## 2026-05-24 実装メモ

Phase 1 の最小実装として、既存 `RepairJob` に以下を追加した。

- `RepairTerminalReason`
- `VerifierDelta`
- `RepairAttemptKey`
- `RejectedAttempt`
- `RepairJobEvent`
- `RepairNextAction`
- `RepairJob::apply_event`
- `RepairJob::next_action`

今回の実装では、既存の verifier repair 分岐を一気に置き換えていない。
まずは lifecycle event と next action の小さな状態機械を追加し、既存フローから次の観測イベントだけを記録する。

- repair patch が repo edit として適用された。
- verifier rerun の delta が観測された。
- invalid repair が target / reason 付きで reject された。

この接続は production behavior を大きく変えず、次の migration で `turn.rs` の分散分岐を `RepairJob::next_action()` へ寄せるための足場である。

追加した単体テスト:

- assessment がない場合は diagnostic を要求する。
- accepted target がある場合は patch を要求する。
- patch 適用後は verifier rerun を要求する。
- verifier pass は `VerifiedDone` になる。
- 同じ patch reject が続く場合、budget 内では re-plan し、budget 超過後は safe stop する。
- authority ambiguity は patch に進まず safe stop する。
- malformed diagnostic は budget 内で再診断し、budget 超過後は diagnostic unavailable で止まる。

検証結果:

| check | result |
| --- | --- |
| `cargo test --lib repair_job_` | pass: 20 tests |
| `cargo test --lib repair_plan` | pass: 10 tests |
| `cargo test --lib` | pass: 2866 tests |
| `cargo clippy --all-targets -- -D warnings` | pass |
| `cargo build --release` | pass |

注意:

- sandbox 内の `cargo test --lib` は `mockito` が local server を起動できず失敗したため、通常権限で再実行して pass を確認した。
- `RepairJob::next_action()` はまだ production main path の唯一の判断関数ではない。v0.4.16 の次段階で、既存 `verifier_repair_decision` / invalid repair retry / safe stop emit を段階的に寄せる。

## 2026-05-24 Phase 2 実装メモ

`RepairPlanProposal` と `AcceptedRepairPlan` を分離した。

目的は、diagnostic LLM の出力をそのまま実行計画として扱わないこと。
LLM が返すものは proposal であり、Anvil の authority validation を通過して初めて accepted plan になる。

追加したもの:

- `repair_plan.rs`
- `RepairPlanProposal`
- `AcceptedRepairPlan`
- `RepairPlanValidationError`
- `validate_repair_plan_proposal()`

既存フローへの接続:

- `validate_verifier_repair_plan_admission()` を proposal validation 経由に変更した。
- authority 判定そのものは既存 `repair_authority` に寄せ、`repair_plan` は境界の薄い wrapper に留めた。

この変更で、diagnostic LLM / PAM / verifier output が直接 patch 実行へ流れ込む経路を狭めた。
ただし、patch proposal provider と deterministic fallback の降格は未完了であり、次フェーズの対象である。

## 2026-05-24 Phase 3 実装メモ

`PatchProposalProvider` の admission boundary を追加した。

追加したもの:

- `patch_provider.rs`
- `PatchProviderKind`
- `PatchProviderRequest`
- `PatchProviderOutput`
- `PatchAdmission`
- `admit_patch_provider_output()`

この境界では、provider は target / role / allowed change kind / authority を選べない。
それらは `AcceptedRepairPlan` から与えられ、provider が返した `PatchProposal` は target contents に対して exact edit validation を受ける。

既存フローへの接続:

- LLM patch proposal の shadow validation を `AcceptedRepairPlan` ベースの provider admission 経由に変更した。
- 旧 legacy validation はまだ最終判定として残している。

検証結果:

| check | result |
| --- | --- |
| `cargo test --lib patch_provider` | pass: 3 tests |
| `cargo clippy --all-targets -- -D warnings` | pass |

未完了:

- deterministic fallback candidate はまだ全て provider admission 後段に降格できていない。
- `PatchAdmission` は telemetry / shadow boundary に接続した段階であり、production main path の唯一の gate ではない。

## 2026-05-24 Phase 4 実装メモ

deterministic repair candidate を production main path から一段降格した。

変更点:

- controller deterministic candidate を `PatchProviderKind::DeterministicFallback` として扱う。
- deterministic candidate は `AcceptedRepairPlan` に対する provider admission を通過した場合だけ既存 validator / apply に進む。
- admission が失敗した場合は即適用せず、既存 LLM repair path にフォールスルーする。
- telemetry に `provider_kind=deterministic_fallback` を追加した。

この変更の意味:

- deterministic repair が target / authority / allowed change kind を自分で選んで進む経路を狭めた。
- 旧処理を即削除せず、accepted plan 後段の fallback provider として観測可能にした。

未完了:

- Python/pytest 固有 candidate の実装自体はまだ `turn.rs` に残っている。
- runtime adapter への移動と、同等以上の新 pipeline が確認できた処理の削除は後続作業。

## 2026-05-24 Phase 5 実装メモ

terminal reason の投影境界を追加した。

変更点:

- `RepairTerminalReason::safe_stop_reason()` を追加した。
- `verified_done` は safe stop ではないため `None` にする。
- ambiguous / no target / exhausted / diagnostic unavailable / verifier unavailable を既存 `StopReason` に明示的に対応させた。

この変更は production dispatch の全面切替ではない。
目的は、`RepairJob::next_action()` が terminal decision を握る段階で、文字列 parse ではなく enum-to-enum mapping で safe stop report に接続できるようにすることである。

検証結果:

| check | result |
| --- | --- |
| `cargo test --lib repair_terminal_reason_projects_to_existing_safe_stop_reason` | pass |

## 2026-05-24 Phase 6 評価メモ

現時点では、大量 E2E 評価には進んでいない。
理由は、v0.4.16 の変更がまだ「新 pipeline を production main path の唯一の制御構造にする」段階ではなく、旧処理と並走する移行フェーズだからである。

実施した評価:

| check | result |
| --- | --- |
| `cargo fmt --check` | pass |
| `cargo clippy --all-targets -- -D warnings` | pass |
| `cargo test --lib` | pass: 2870 tests |
| `cargo build --release` | pass |

次に評価すべきこと:

1. `RepairJob::next_action()` を production dispatch の判断へ段階的に接続する。
2. synthetic verifier failure で `FailurePacket -> RepairPlanProposal -> AcceptedRepairPlan -> PatchAdmission -> VerifierDelta` の一連の流れを固定する。
3. deterministic fallback が `provider_kind=deterministic_fallback` として観測され、accepted plan なしに適用されないことを E2E で確認する。
4. その後に PAM なし 5 回 smoke を実施する。

## 2026-05-24 Smoke 評価結果

詳細は `08-smoke-evaluation-2026-05-24.md` に記録した。

実施:

- PAM なし 5 回
- PAM あり 5 回

結果:

| condition | success | failure |
| --- | ---: | ---: |
| PAMなし | 0 | 5 |
| PAMあり | 0 | 5 |

結論:

- 現時点では期待挙動に到達していない。
- PAM 有無で改善は見えない。
- 支配的な failure mode は memory ではなく、verifier repair control 側の構造問題。

新たに強く確認できた問題:

- `.anvil-state/verifier-python/site/...` が user repo edit として大量に数えられている。
- invalid controller repair proposal が長く繰り返され、safe stop までが遅い。
- `AcceptedRepairPlan` / `SemanticRepairPlan` がない状態でも repair pass 側へ進み、late reject されている。

次の最優先:

1. `.anvil-state/` を artifact ledger / edited summary / changed candidates から除外する。
2. `AcceptedRepairPlan` なしでは patch provider を呼ばない。
3. `RepairJob::next_action()` を production dispatch の SSOT にする。
