# v0.4.16 方針レビュー

## 汎用性

評価:

- 方針自体は FastAPI CRUD 固有ではない。
- `FailurePacket -> RepairPlan -> PatchProposal -> VerifierDelta` は runtime 非依存。
- ただし現コードには Python/pytest 固有処理が多く、実装時に adapter へ閉じ込める必要がある。

指摘:

- `synthesized_missing_implementation_target_path_for_request` は request substring 依存であり、汎用性が弱い。
- pytest repair candidate を増やす方向は止めるべき。

対応:

- main path では `RepairStep` なしの deterministic repair を禁止する方針にした。
- runtime-specific parser は `DiagnosticHint` provider へ降格する方針にした。

## シンプルさ

評価:

- 新しく `RepairJob` 状態機械を作るため、一時的に構造は増える。
- ただし、現在は分岐が `turn.rs` に散っているため、長期的には単純化できる。

指摘:

- いきなり全置換するとリスクが高い。

対応:

- MVP は event / next action / terminal reason に絞る。
- 旧処理は削除ではなく fallback provider として接続し直す。

## 安定性

評価:

- 成功だけでなく actionable safe stop を正しい終端に含めた点は妥当。
- 同じ invalid repair を繰り返さない設計が必要。

指摘:

- safe stop を成功率に混ぜると品質を誤認する。

対応:

- `verified_success_rate` と `actionable_safe_stop_rate` を分離する。
- terminal reason を structured に記録する。

## セキュリティ

評価:

- LLM / PAM / README / verifier output を untrusted とする方針は妥当。
- dependency setup は便利だが、package install は明確なリスク。

指摘:

- `pyproject.toml` 由来の dependency をそのまま install する設計は慎重に扱うべき。

対応:

- dependency setup は runtime verifier adapter に閉じ込める。
- package name validation / workspace-local target / policy gate を必須にする。
- arbitrary dependency install に見える場合は safe stop とする。

## 保守性

評価:

- `turn.rs` が肥大化していることが最大の保守性リスク。
- repair meaning、patch shaping、terminal decision が混在している。

対応:

- `RepairJob` event model を別 module に切り出す。
- runtime-specific parser は adapter module へ移す。
- deterministic repair は provider interface に押し込める。

## 整合性

評価:

- 6 セクションの方針は整合している。
- 特に「旧処理をすぐ消さず観測しながら降格する」点が、安定性と保守性の両方に合っている。

残るリスク:

- 新 pipeline と旧 pipeline が長く併存すると、かえって複雑になる。

対応:

- 降格・削除条件を明文化した。
- provider 発火 telemetry を必須にし、削除判断できるようにする。

## 最終判断

この方針は、現時点の問題に対する妥当な次手である。

ただし、次の制約を守る必要がある。

- Python/pytest 個別 repair を増やさない。
- `RepairJob` の state transition をまず単体テストで固める。
- 旧 deterministic repair は安全境界と fallback に限定する。
- 成功率評価の前に safe stop 品質を確認する。

