# v0.6.11 Residual Hypothesis Validation Work Plan

作成日: 2026-06-10

参照:

- `workspace/v0.6.11/wp-known-issues-20260610.md`
- `workspace/v0.6.11/wp-e-api-contract-expectation-validation-20260610.md`
- `workspace/v0.6.11/wp-g-pam-availability-reporting-validation-20260610.md`
- `workspace/v0.6.11/future-architecture-direction-20260610.md`
- `workspace/v0.6.11/architecture-work-plan-20260610.md`

## 1. 目的

WP-A から WP-G で typed contract / evidence / deliverable / PAM availability の境界は前進した。ただし、まだ次の残課題がある。

- false-done: 成果物はあるが、schema/content が objective を満たしていないのに `done` になる。
- false-missing: 外部的には成果物や verifier が有効なのに、内部 terminal が `safe_stop_verifier_missing` / `repair_exhausted` になる。
- repair convergence: 診断は正しそうでも、修復 proposal が実行可能な差分に収束しない。
- non-coding leakage: data / research / ops に coding/edit obligation が漏れる。
- PAM effectiveness: availability は分離できたが、`pam_availability=injected` の品質寄与は未測定。
- complexity risk: `task_contract.rs` / loop 周辺に責務が戻ると、またパターンマッチング化する。

この計画は、残課題を benchmark 別の分岐で潰すのではなく、次の仮説を小さく実装・検証するための作業計画である。

## 2. 検証する中心仮説

### H1: terminal alignment は EvidenceObservation を中心にすれば改善する

現在は成果物存在、runner 結果、artifact obligation、terminal projection がまだ複数経路に分散している。`ObjectiveContract + DeliverableObligation + EvidenceObservation` から terminal を projection する経路を強めると、false-done / false-missing が同時に減るはず。

### H2: Data/API は prompt-bound contract だけでは足りず、evidence-side observation が必要

Data CSV と FastAPI は、contract を prompt に渡すだけでは不十分だった。実 artifact / test result / API behavior を typed observation として観測し、contract と比較した差分を repair target に渡す必要がある。

### H3: non-coding leakage は DeliverableObligation construction の責務分離で減る

data / research / ops の失敗は、model の能力不足よりも、coding 用の fresh edit / test evidence pressure が non-coding objective に混ざることが主因に見える。TaskKind ではなく deliverable/evidence contract から obligation を作れば漏れが減るはず。

### H4: repair_exhausted は diagnosis と executable repair action の接続不足で起きている

診断は正しい方向を示しても、repair proposal が malformed / ambiguous として落ちる。repair worker に渡す入力を prose ではなく typed delta に寄せ、action admissibility を先に確認すれば収束率が上がるはず。

### H5: PAM の品質寄与は injected availability を作らない限り評価できない

WP-G で `disabled` / `failed` / `pam_unavailable` は分離できたが、PAM context は注入されていない。まず `pam_availability=injected` を再現し、その上で no-PAM / PAM-unavailable / PAM-injected を分けて比較する必要がある。

### H6: 改善速度よりも責務境界を守る方が成功率に効く

個別パターンを足すほど短期 smoke は通りやすいが、長期的には複雑性と leakage が増える。新規ロジックは typed schema / manifest parser / observation / projection に閉じ、raw string rule を増やさない方が安定するはず。

## 3. 守る制約

- 各 WP は最小実装にする。
- 各 WP は unit / focused integration / 実 LLM smoke を通す。
- semantic / evidence / terminal / repair に影響する WP は必ず実 LLM を使う。
- 1-6 run の成功で改善主張をしない。これは仮説確認と regression 早期検知に限定する。
- 20-run は regression guard、50-run は改善主張候補に使う。
- 期待と違う挙動が出たら、次 WP の前に仮説と計画を更新する。
- coding / feature improvement / TDD / docs / data / research / ops を検証対象に含める。
- provider abstraction を増やさない。
- PAM / memory を terminal authority にしない。
- `WorkMode` や `TaskKind` だけで tool policy / completion authority を決めない。

## 4. complexity guard

各 WP の完了前に次を確認する。

- `task_contract.rs` の責務を増やしていない。少なくとも net increase を避ける。
- 新規分岐が benchmark 名、framework 名、具体 prompt 文字列に依存していない。
- deterministic rule は typed parser / typed observation / safety boundary に限定されている。
- LLM を使う箇所は semantic judgement / repair planning / ambiguity resolution に限定し、controller は typed result を扱う。
- 新しい状態や enum は terminal projection / repair / evidence のどこで使われるかが明確である。
- ログ追加は diagnosis 用であり、completion authority にはしない。

## 5. Work Packages

## RWP-0: residual baseline と failure signature 固定

目的:

- 以降の改善が本当に残課題に効いたか判断できるように、代表 failure signature を固定する。

作業:

- WP10/WP11/WP-A-G の既存ログから false-done / false-missing / repair_exhausted / non-coding leakage / PAM failed rows を再分類する。
- 代表ケースを小さな suite に固定する。
  - data CSV schema mismatch
  - TOML false-missing
  - FastAPI request body/status mismatch
  - research artifact task
  - ops command report task
  - Python coding
  - feature improvement
  - TDD
- 各ケースについて expected deliverable / evidence / terminal を表形式にする。

最小検証:

- 既存 eval log の読み取りのみ。
- 必要なら実 LLM で各カテゴリ 1 run ずつ再現確認する。

完了条件:

- `workspace/v0.6.11/residual-baseline-YYYYMMDD.md` に failure signature が記録される。
- 以降の RWP で使う smoke case sequence が固定される。

見直し条件:

- 代表 failure が再現せず、既存ログだけから原因を推定するしかない場合。

## RWP-1: EvidenceObservation terminal shadow

目的:

- terminal 判定を変える前に、EvidenceObservation 由来の shadow terminal と現行 terminal の差分を観測する。

作業:

- `EvidenceObservation` から `ShadowTerminalProjection` を作る小さな pure module を追加する。
- 現行 terminal は変えず、eval log に次を追加する。
  - shadow terminal class
  - missing evidence ids
  - satisfied evidence ids
  - conflict between current terminal and shadow terminal
- false-done / false-missing を shadow 上で分類できるようにする。

最小検証:

- unit:
  - artifact exists + schema failed => shadow failure
  - artifact exists + evidence missing => shadow missing_evidence
  - verifier passed + artifact bound => shadow success
  - verifier unavailable but artifact evidence sufficient => shadow success or not_applicable
- focused integration:
  - TOML evidence
  - data CSV evidence
  - docs artifact evidence

実 LLM 検証:

- data CSV 3-run
- TOML 3-run
- docs 2-run
- Python coding 2-run

完了条件:

- behavior change はない。
- 現行 terminal と shadow terminal の不一致がログから説明できる。
- 不一致の分類が benchmark 固有文字列に依存しない。

見直し条件:

- shadow terminal が現行 terminal のコピーに近く、原因分析に使えない場合。

## RWP-2: EvidenceObservation terminal limited adoption

目的:

- RWP-1 で安全に見えた範囲だけ、terminal 判定を EvidenceObservation projection に寄せる。

作業:

- adoption 範囲を data/docs/TOML のような artifact-bound evidence に限定する。
- `done` は objective-bound evidence が satisfied の場合だけ許可する。
- `missing_verification` 系は `missing_evidence` へ internal projection し、旧 label は compatibility output に限定する。
- failure 時は repair target に missing evidence ids を渡す。

最小検証:

- unit:
  - data artifact exists but schema failed => no `done`
  - TOML external valid evidence => no false-missing
  - docs required artifact satisfied => `done`
- integration:
  - terminal projection tests
  - eval taxonomy tests

実 LLM 検証:

- data CSV 6-run
- TOML 6-run
- docs 3-run
- Python coding 3-run
- TDD 2-run

完了条件:

- data CSV false-done が 0/6。
- TOML false-missing が 0/6 または明確に減る。
- coding/TDD regression がない。

見直し条件:

- `done` が過度に厳しくなり、成果物が正しいのに safe_stop が増える場合。

## RWP-3: StructuredDataObservation を schema parser 境界に寄せる

目的:

- data false-done を prompt 文字列ではなく typed schema observation で減らす。

作業:

- CSV/JSON の minimal parser を `StructuredDataObservation` に分離する。
- inferred schema は次に限定する。
  - explicit required columns / keys
  - explicit row count
  - explicit literal rows
  - explicit output path
- 未指定の順序・型・余分な意味制約は要求しない。
- schema mismatch を repair delta に渡す。

最小検証:

- unit:
  - required columns pass
  - extra columns fail only when exact columns are explicit
  - missing columns fail
  - row count explicit mismatch fail
  - JSON key mismatch fail
- integration:
  - data completion gate
  - repair target delta

実 LLM 検証:

- data CSV 8-run
- data JSON 4-run
- docs 2-run
- Python coding 2-run

完了条件:

- simple data false-done が 0。
- schema inference が過剰要求を作らない。
- implementation が CSV 固有の ad hoc branch になっていない。

見直し条件:

- LLM が作った妥当なデータ成果物を schema が過剰に reject する場合。

## RWP-4: ApiContractObservation と repair delta

目的:

- FastAPI 固有ではなく HTTP API 一般の evidence-side observation で repair convergence を改善する。

作業:

- `ApiContractExpectation` に対応する `ApiContractObservation` を追加する。
- test failure / HTTP response mismatch から次を抽出する。
  - method
  - path
  - expected request body fields
  - observed request binding issue
  - expected status policy
  - observed status mismatch
- repair prompt には prose ではなく compact typed delta を渡す。
- framework 固有分岐は追加しない。

最小検証:

- unit:
  - POST body field missing
  - query/body binding mismatch
  - unspecified status should not force exact 201
  - status explicitly specified should be honored
- integration:
  - artifact-directed recovery receives API observation
  - repair target decision receives API delta

実 LLM 検証:

- FastAPI 8-run
- Node API or HTTP-style task 3-run
- Python non-API coding 3-run
- docs/data regression 3-run

完了条件:

- FastAPI high_quality が baseline より明確に改善する。
- status drift が減る。
- non-API tasks に API-specific leakage がない。

見直し条件:

- extraction が framework/test-output 文字列に強く依存し始める場合。

## RWP-5: Non-coding DeliverableObligation admission audit

目的:

- data/research/ops に coding edit obligation が漏れる経路を潰す。

作業:

- `DeliverableObligationPlan` を contract input projection から独立させる。
- research/ops/data/docs/coding の obligation を同じ型で表す。
- coding fresh edit guard は coding change / feature improvement に限定する。
- verification-only coding は明示的に edit-free objective として扱えるようにする。
- admission log に obligation source と authority を出す。

最小検証:

- unit:
  - research report requires report artifact, not source edit
  - ops command report requires command observation/report artifact, not source edit
  - data output requires output artifact/schema evidence
  - coding feature requires source edit
  - verification-only coding does not require fresh edit
- integration:
  - terminal diagnostics missing obligation ids

実 LLM 検証:

- research 6-run
- ops 6-run
- data 4-run
- coding feature improvement 4-run
- verification-only coding 2-run

完了条件:

- research/ops `missing_repo_edits` が 0/12。
- coding feature improvement の false-done が増えない。
- obligation source がログで説明できる。

見直し条件:

- coding fresh edit guard が弱まり、既存 artifact だけで coding change が done になる場合。

## RWP-6: Typed repair action admissibility

目的:

- diagnosis は正しいが repair proposal が malformed / ambiguous で落ちる問題を減らす。

作業:

- `RepairTargetDecision` から `RepairActionPlan` を作る最小型を追加する。
- action plan は次を持つ。
  - target artifact
  - expected evidence delta
  - allowed tool category
  - rejection reason if not admissible
- LLM repair proposal の前に controller が admissible action space を提示する。
- LLM の自由文 diagnosis は残すが、実行判断は typed action plan に落とす。

最小検証:

- unit:
  - missing data schema => edit output artifact
  - API body mismatch => edit implementation or test expectation depending authority
  - verifier unavailable => rerun/setup evidence, not arbitrary rewrite
  - no target => safe stop with reason
- integration:
  - repair loop receives action plan
  - malformed proposal is re-asked with same typed action space once

実 LLM 検証:

- FastAPI 6-run
- Python repair 4-run
- Node repair 3-run
- data repair 4-run
- TDD 2-run

完了条件:

- repair_exhausted が対象 smoke で減る。
- malformed/ambiguous rejection が減る。
- action plan が過度な command/tool 強制にならない。

見直し条件:

- action plan が narrow すぎて、LLM が正しい別解を出せなくなる場合。

## RWP-7: PAM injected availability reproduction

目的:

- PAM 効果測定の前提である `pam_availability=injected` を再現可能にする。

作業:

- `context_pack_failed` の原因を切り分ける。
  - Photon enabled state
  - sidecar model availability
  - timeout
  - context pack input size
  - parse failure
- PAM availability log に failure phase を追加する。
- injected が作れない場合は、効果測定ではなく availability failure として作業を閉じる。

最小検証:

- unit:
  - failed phase projection
  - injected summary projection
  - no-PAM disabled projection
- local diagnostic:
  - available model list
  - sidecar call smoke
  - context pack minimal input smoke

実 LLM 検証:

- PAM minimal 3-run
- no-PAM minimal 3-run
- injected が確認できた場合だけ mixed 10-run

完了条件:

- `pam_availability=injected` が少なくとも 1-run で観測できる、または失敗 phase が特定される。
- PAM failure が task failure と混ざらない。

見直し条件:

- local environment 依存が強く、コード改善ではなく運用設定問題と判断される場合。

## RWP-8: complexity reduction slice

目的:

- 改善で複雑性を増やさないよう、責務分離を継続する。

作業:

- `task_contract.rs` から次のいずれかをさらに分離する。
  - evidence observation construction
  - deliverable obligation planning
  - terminal projection
  - repair action planning
- behavior change と refactor を混ぜない。
- public interface を小さく保つ。

最小検証:

- moved module の unit tests。
- existing focused tests。
- `cargo build`。

実 LLM 検証:

- no-op refactor に近い場合でも docs/data/Python coding 各 1-run。

完了条件:

- `task_contract.rs` の行数または責務が減る。
- regression がない。
- 新しい module の責務が一文で説明できる。

見直し条件:

- 抽象が薄すぎて call-through だけになり、理解コストだけ増える場合。

## RWP-9: 20-run regression guard

目的:

- RWP-1 から RWP-8 の採用が、局所 smoke だけの偶然でないことを確認する。

実施タイミング:

- terminal / evidence / repair / deliverable obligation の behavior change が 1 つ以上入った後。
- ただし known regression が unit/focused smoke で明らかな場合は実施しない。

構成:

- coding: Python sales, Node JSON/CSV, Rust/TOML/NDJSON のうち複数
- feature improvement: existing project change
- TDD: test-first request
- data: CSV/JSON
- docs: runbook
- research: source/report
- ops: command observation report
- API: FastAPI or HTTP-like task

判定:

- pass / high_quality / false_done / false_missing / repair_exhausted / missing_repo_edits を見る。
- 20-run は regression guard であり、成功率改善の最終主張には使わない。

完了条件:

- high_quality が baseline より落ちない。
- false-done / false-missing が増えない。
- repair_exhausted が増えない。
- task kind 別に悪化がない。

## RWP-10: 50-run improvement claim candidate

目的:

- 20-run で regression がない場合だけ、改善主張できるか確認する。

実施タイミング:

- RWP-9 が通過した後。
- PAM injected 効果を主張する場合は、`pam_availability=injected` が再現できている後。

構成:

- no-PAM / PAM-unavailable / PAM-injected を区別する。
- coding / feature / TDD / data / docs / research / ops / API を含める。
- task kind 別 summary を必ず出す。

判定:

- 全体成功率だけでなく、task kind 別 high_quality を見る。
- false-done / false-missing を成功に数えない。
- PAM failed を PAM 効果として扱わない。

完了条件:

- baseline より high_quality が改善している。
- false terminal alignment が減っている。
- 改善が一部 task kind だけの偏りでない。

見直し条件:

- 全体は改善しても non-coding / TDD / feature improvement が悪化する場合。

## 6. 推奨実行順序

1. RWP-0: residual baseline 固定
2. RWP-1: EvidenceObservation terminal shadow
3. RWP-2: shadow 結果に基づく limited adoption
4. RWP-3: StructuredDataObservation
5. RWP-4: ApiContractObservation
6. RWP-5: Non-coding DeliverableObligation audit
7. RWP-6: Typed repair action admissibility
8. RWP-7: PAM injected availability reproduction
9. RWP-8: complexity reduction slice
10. RWP-9: 20-run regression guard
11. RWP-10: 50-run improvement claim candidate

RWP-2 以降で仮説と異なる結果が出た場合、RWP-4/RWP-5/RWP-6 を前倒しまたは延期してよい。ただし、benchmark-specific rule を足して短期改善を作る方向には進めない。

## 7. 記録方針

各 RWP は次の形式で記録する。

- `workspace/v0.6.11/rwp-N-<topic>-validation-YYYYMMDD.md`

必須項目:

- 仮説
- 実装範囲
- deterministic tests
- 実 LLM validation
- observed regression
- known issues
- 次に仮説を維持するか、修正するか

## 8. 次の最小着手

最初に着手すべきは RWP-0 と RWP-1 である。

理由:

- いきなり terminal 判定を変えると、false-done と false-missing のどちらを改善したのか分からなくなる。
- shadow projection なら behavior change を抑えたまま、EvidenceObservation 中心化が本当に効きそうかを確認できる。
- ここで不一致分類が有効なら、RWP-2 で小さく採用する根拠になる。

RWP-0/RWP-1 の最小 LLM smoke は次を推奨する。

- data CSV: 3-run
- TOML: 3-run
- docs: 2-run
- Python coding: 2-run
- TDD: 1-run

この段階では改善主張をしない。観測品質と regression absence だけを評価する。
