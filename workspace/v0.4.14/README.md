# v0.4.14 評価結果と構造分析

補足資料:

- [構造的問題・根本原因・解決仮説](root-cause-hypotheses.md)
- [ローカル LLM 仮説検証結果](llm-hypothesis-verification.md)
- [修正方針](fix-policy.md)
- [設計方針](design-policy.md)
- [修正方針・設計方針レビュー](policy-review.md)
- [作業計画](work-plan.md)

## 評価条件

- 実行日: 2026-05-24
- 対象バイナリ: `target/release/anvil` (`anvildev` symlink 経由)
- コマンド:
  `anvildev -m qwen3.6:27b-coding-nvfp4 --sidecar-model qwen3.5:9b -y --fresh-session --no-footer --deterministic-fallback full --max-iterations 50`
- 評価要求:
  `FastAPIでcrudのAPIを開発してください。使用方法をREADME.mdに記述してください。テストコードも実装してください。`
- 打ち切り条件: ユーザー指示により 10 回で停止
- 実施範囲: PAM なし 10 回
- PAM あり評価: 未実施

補足: 11 回目のログはプロセス停止前に一部だけ作成されたが、集計対象外。

## 集計

| 指標 | 結果 |
| --- | ---: |
| 評価回数 | 10 |
| 成功 | 1 |
| 失敗 | 9 |
| 成功率 | 10.0% |
| 平均所要時間 | 406.1 秒 |
| 平均 iteration | 23.1 |
| 成果物生成 | 10/10 で `app/main.py`, `tests/test_main.py`, `README.md` を生成 |
| verifier 到達 | 10/10 |
| 主な失敗種別 | 9/9 が `verifier_failed` |

## 実行別結果

| Run | 結果 | terminal reason | 秒 | 主な最終失敗 |
| --- | --- | --- | ---: | --- |
| nopam_01 | failed | `verifier_failed` iter 30 | 514 | `POST` が 201 だが test は 200、`DELETE` が 204 だが test は 200 |
| nopam_02 | failed | `verifier_failed` iter 29 | 441 | 201 vs 200、in-memory state が test 間で漏れる |
| nopam_03 | failed | `verifier_failed` iter 21 | 539 | 201 vs 200、204 vs 200、state leak |
| nopam_04 | failed | `verifier_failed` iter 21 | 604 | 201 vs 200、204 vs 200、validation 期待 422 に対して 201 |
| nopam_05 | failed | `verifier_failed` iter 21 | 394 | 201 vs 200、204 vs 200、state leak |
| nopam_06 | failed | `verifier_failed` iter 18 | 249 | 201 vs 200 |
| nopam_07 | failed | `verifier_failed` iter 23 | 271 | state leak。fixture はあるが SUT の実状態を reset できていない |
| nopam_08 | failed | `verifier_failed` iter 30 | 416 | `NameError: name 'get_db' is not defined` |
| nopam_09 | success | `done` iter 22 | 244 | pass |
| nopam_10 | failed | `verifier_failed` iter 22 | 389 | 201 vs 200、204 vs 200、state leak、存在しない `/health` を test が要求 |

## 何が改善しているか

成果物生成フェーズはかなり前進している。今回の 10 回では、全 run が実装・テスト・README を生成し、verifier 実行まで到達した。

過去に多かった「README まで到達しない」「test artifact を作らず止まる」「既存 workspace を誤検出する」問題は、この評価では主因になっていない。

## まだ未達な点

本質的な未達は verifier repair の収束性。

Anvil は verifier 失敗後に診断し、修復対象を選び、controller-applied repair を何度も実行している。しかし、修復が verifier pass に収束しない。特に以下が目立つ。

- HTTP API 仕様の曖昧さを解消できない
  - `POST /items` を 201 とする実装と、200 を期待する generated test が衝突する。
  - `DELETE /items/{id}` を 204 とする実装と、200 + body を期待する generated test が衝突する。
  - CRUD API としてはどちらもあり得るため、ユーザー要求だけでは一意に決まらない。
- test isolation を直し切れない
  - in-memory store が test 間で残り、件数が 2 ではなく 3 になる。
  - run 09 は provider module state を直接 reset する fixture を入れて成功した。
  - run 07 は `get_db` dependency override 風の fixture を作ったが、SUT 側が `get_db` を使っていないため無効だった。
- 無効な repair を繰り返して時間を消費する
  - 失敗 run では `Rejected invalid controller repair proposal` が繰り返し発生。
  - 最終的に `verifier repair exhausted: no safe repair target remains` になるが、実際には単純な status code / test isolation 修正が残っている場合がある。
- 一部で test が実装に存在しない内部 helper を仮定する
  - run 08 は `app.dependency_overrides[get_db]` を使うが、`get_db` が import されていない。
  - これは test 修復が public API ではなく存在しない内部構造を仮定した例。

## 構造的な根本原因

### 1. repair が「ジョブ」ではなく「局所 edit の繰り返し」になっている

現状は以下の部品が分かれている。

- verifier failure から target を選ぶ
- diagnostic LLM で failure_kind / target を得る
- patch proposal を得る
- deterministic validator で reject / apply する
- verifier を再実行する
- progress を分類する

ただし、これらが「この failure set を pass まで収束させる一つの修復ジョブ」として十分に統合されていない。

そのため、複数の失敗が同時に残ると、1 個ずつ場当たり的に編集し、次の verifier failure に流される。今回の 201/204/status/test isolation のような複合失敗では、全体を一貫して解く修復計画になっていない。

### 2. source of truth の決定が実運用上まだ弱い

`SpecAuthority` / `BehaviorContract` / `SemanticRepairPlan` は導入されているが、今回のような「ユーザー要求が詳細 HTTP status を指定していない」ケースでは、どちらを正とするかを安定して決められていない。

CRUD API という要求からは、以下のどちらも成立する。

- 実装を 200/200 に寄せる
- test を 201/204 に寄せる

ここを一意に決めないまま repair に入るため、実装側を直したり、test 側を直そうとして validator に拒否されたり、結果として収束しない。

必要なのは「曖昧な仕様を推測で固定する」ことではなく、生成済み実装・README・test の整合性から採用する仕様を repair job 内で一度決め、以後の patch はその決定に従わせること。

### 3. FailurePacket が構造化情報を十分に運んでいない

`FailurePacket` には `affected_cases` や `observed_expected_pairs` があるが、現在の production path では空の `Vec` が渡されている箇所がある。

結果として、v0.4.13 で意図した「構造化された失敗情報を repair に渡す」設計が十分に効いていない。repair LLM はまだ bounded output excerpt 依存になりやすい。

これは設計思想そのものではなく、実装の結線不足に近い。

### 4. deterministic candidate が Python/FastAPI 近傍に寄りすぎている

`controller_repair_candidate_for_job()` 配下には Python test / pytest / provider mutable state 向けの候補生成が入っている。

これは今回の FastAPI CRUD には効く可能性があるが、ローカル LLM 向け汎用 coding agent という設計思想からは危険がある。

- 成功時は強いが、外したときに別の局所ルールを追加したくなる
- Python 以外、pytest 以外、API 以外に広がりにくい
- 「ルールで repair する」方向に寄り、LLM の汎用推論を活かしにくい

deterministic 側は安全・境界・検証に寄せ、意味解釈と修復方針は LLM に構造化出力させる方が汎用性は高い。

### 5. cheap check が syntactic で、semantic failure を事前に落とせない

`ProjectVerifier` は Python では主に `py_compile` と一部の missing global binding を見る。

これにより、構文として正しいが pytest 的には壊れている修復が通り、full verifier で初めて失敗する。失敗 run ではこの高コスト cycle を何度も回している。

ただし、ここで full pytest を cheap check に入れるのは過剰。必要なのは、LLM patch の前後で「同じ failure set に対して何を直す予定か」を構造化し、明らかに別方向の patch を早めに reject すること。

### 6. invalid repair の扱いが「学習」ではなく「消費」になっている

無効 proposal は validator で reject されるが、その理由が次の repair plan に十分に効いていない。

現状は「invalid を記録して retry / target exhaustion」まではできる。しかし「なぜ invalid だったか」を次の LLM 指示に反映し、別 target / 別方針 / safe stop に切り替える力が弱い。

結果として、同じ target に対する無効 proposal が続き、時間を消費して `no safe repair target remains` で止まる。

## 具体的なバグ候補

### Bug 1: `python_test_source_has_state_isolation()` が fixture の存在だけで state isolation 済みとみなす

run 07 の失敗が典型。

test には fixture があるが、SUT の実状態 `items_db` / `next_id` を reset していない。にもかかわらず deterministic candidate 側は「fixture がある」と見て provider state reset 候補を出さない可能性がある。

これは「isolation fixture が存在する」ではなく「SUT provider state に接続された isolation が存在する」を見ないといけない。

### Bug 2: test repair が存在しない内部 helper を仮定する

run 08 では test が `get_db` を前提にするが、`app.main` は `get_db` を提供していない。

validator には missing local import symbol を見る処理があるが、最終的にこの状態を防げていない。test repair の public API 優先ルールがまだ徹底されていない。

### Bug 3: failure detail の標準出力が不十分な失敗がある

`verifier repair exhausted: no safe repair target remains` で止まる run では、最終 pytest failure が標準ログに出ない。後から各評価ディレクトリで pytest を再実行しないと原因が読めない。

これはユーザー体験・デバッグ性の問題。safe stop 時にも最後の verifier failure summary を必ず添付すべき。

### Bug 4: v0.4.13 の構造化 packet が実 repair に十分接続されていない

`FailurePacket` の `affected_cases` / `observed_expected_pairs` が空のまま渡る経路がある。RepairBrief / PatchProposal に構造化情報を渡す設計があるのに、実データが薄い。

これは今回の「LLM に構造化して考えさせる」方針と実装のギャップ。

## コード上の確認ポイント

- `src/agent/loop_run/failure_packet.rs:84`
  - `FailurePacket::from_repair_job()` が `affected_cases` / `observed_expected_pairs` に `Vec::new()` を渡している。
  - 構造化 packet の器はあるが、production repair で使う中身が薄い。
- `src/agent/loop_run/turn.rs:23456`
  - `controller_repair_candidate_for_job()` が Python/pytest 近傍の deterministic candidate を選ぶ。
  - 汎用 repair の主経路としては特化が強い。
- `src/agent/loop_run/turn.rs:23589`
  - state leak 候補生成が `python_test_source_has_state_isolation(contents)` で早期 return する。
  - fixture が存在しても SUT に接続されていない run 07 のようなケースを取り逃がす。
- `src/agent/loop_run/turn.rs:13719`
  - repair pass は最大 3 attempt の patch proposal loop。
  - invalid proposal の理由は retry message に入るが、job 全体の修復方針変更に十分昇格していない。
- `src/agent/loop_run/project_verifier.rs:92`
  - `ProjectVerifier` は `.py` / `.pyw` に対して Python syntax cheap check を行う。
  - semantic pytest failure は cheap check では検出できない。

## 対策方針

### 方針 A: RepairJob を「収束単位」にする

repair を単発 edit の連鎖ではなく、以下を持つ一つの job として扱う。

- failure set
- accepted spec decision
- repair plan
- attempted patches
- invalid reasons
- rerun progress
- stop reason

重要なのは、LLM に「次どうする？」を自由回答させないこと。Anvil は job state から次の小さな依頼を決め、LLM には限定された仕事だけ渡す。

### 方針 B: LLM を「診断 + 方針決定」に使い、deterministic は安全境界に戻す

deterministic ルールで FastAPI / pytest 固有の修復を増やすのではなく、LLM に以下を JSON で出させる。

- failure clusters
- observed vs expected
- ambiguity
- chosen source of truth
- repair target
- patch intent
- why this patch should reduce the failure set

Anvil 側は以下だけを deterministic に担う。

- path scope
- owned artifact
- exact patch application
- secret / shell injection / tool markup reject
- assertion deletionや明らかな弱体化の reject
- verifier rerunと progress 判定

### 方針 C: ambiguous spec を明示的な状態にする

ユーザー要求・README・実装・test のどれも優位でない場合は、`SpecAmbiguous` として扱う。

この場合の選択肢は 2 つ。

- newly generated artifacts の内部整合性を優先して、実装・test・README のいずれかに統一する
- 安全に決められない場合は safe stop して、採用可能な仕様候補を提示する

少なくとも、曖昧なまま実装修正と test 修正を往復しない。

### 方針 D: test isolation は public behavior と provider state の接続で判定する

fixture の有無ではなく、実際に SUT の状態を reset しているかで判定する。

汎用化するなら、Python 固有の `items_db` などを増やすのではなく、LLM に「この test isolation は SUT に接続されているか」を診断させ、Anvil は patch の安全性と verifier 結果で判定する。

### 方針 E: safe stop を失敗ではなく有用な診断成果物にする

repair exhausted の場合でも、以下を必ず残す。

- 最後の verifier failure summary
- 試した target
- reject 理由
- 採用した source of truth
- 残っている failure cluster
- 次の推奨 action

これがあれば、ユーザーや別エージェントが次に何を直すべきか判断できる。

## 次の修正候補

1. `FailurePacket` に pytest parser 由来の `affected_cases` / `observed_expected_pairs` を実際に詰める。
2. verifier repair prompt を `FailurePacket + invalid reasons + current spec decision` ベースに一本化する。
3. `python_test_source_has_state_isolation()` を「fixture 存在」ではなく「SUT provider state reset が存在」判定へ修正する。
4. `repair exhausted` の terminal output に最後の verifier summary を必ず出す。
5. deterministic Python repair candidate は production path の主役から外し、LLM patch の fallback / validator 補助へ降格する。
6. ambiguous spec のときに、実装・test・README の整合性を採るか safe stop するかを明示状態で扱う。

## 結論

現時点の品質は、生成フェーズは改善しているが、実用水準にはまだ届いていない。

10 回中 10 回が verifier まで進んだ点は前進。ただし、10 回中 1 回しか完了していないため、未解決の中心は verifier repair の構造。今回の結果は、v0.4.13 の新 pipeline が「観測と一部の制御」まではできているが、「複合 failure を一つの修復ジョブとして収束させる」段階にはまだ達していないことを示している。

## qwen3.6 実診断検証の追記

実 verifier failure 3 件を `qwen3.6:27b-coding-nvfp4` に渡し、`RepairPlan JSON` を出せるか検証した。

結果として、`/no_think`、`think:false`、`format:"json"`、`temperature:0`、`num_predict` 上限を併用した場合、3 件すべてで valid JSON が返り、failure cluster と repair target も実用的な粒度で分離できた。同じ複合 failure の再実行では出力 JSON も一致した。

一方で、`format:"json"` だけでは thinking に token を使い切り、response が空になることを確認した。また、`201 vs 200` / `204 vs 200` のような曖昧仕様を `spec_ambiguity` とせず test を正として扱う傾向も見えた。

したがって、qwen3.6 は repair diagnosis の候補生成器として有望だが、最終判断者にはできない。Anvil 側で schema validation、authority policy、ambiguity detection、patch safety validation、verifier progress 管理を行う前提が必要。

## 2026-05-24 追加評価

今回の修正後に、同一タスクで PAM なし 10 回、PAM あり 5 回を追加評価した。

タスク:

```text
FastAPIでcrudのAPIを開発してください。使用方法をREADME.mdに記述してください。テストコードも実装してください。
```

実行条件:

- command: `anvildev -m qwen3.6:27b-coding-nvfp4 --sidecar-model qwen3.5:9b -y --fresh-session --no-footer --deterministic-fallback full --max-iterations 50`
- PAM なし: `ANVIL_PHOTON_ENABLED=false`
- PAM あり: `ANVIL_PHOTON_ENABLED=true`, `ANVIL_PHOTON_SHADOW_MODE=false`, `ANVIL_PHOTON_CANARY=100`, `ANVIL_PHOTON_TIMEOUT_MS=500`
- PAM health: `{"status":"ok","schema_version":"action-memory.v1"}`

### 結果サマリ

| mode | success | failed | success rate |
| --- | ---: | ---: | ---: |
| PAM なし | 0 | 10 | 0% |
| PAM あり | 2 | 3 | 40% |
| 合計 | 2 | 13 | 13.3% |

### PAM なし

| run | result | terminal reason |
| --- | --- | --- |
| 01 | failed | `verifier_repair_pass_invalid: repair intent string contained markdown or tool-call markup` |
| 02 | failed | `No module named pytest` を verifier failure として残したまま終了 |
| 03 | failed | `verifier_repair_pass_invalid: repair intent string contained markdown or tool-call markup` |
| 04 | failed | `No module named pytest` を verifier failure として残したまま終了 |
| 05 | invalid | 評価ログを作業ディレクトリ内に置いたため測定汚染。集計対象外 |
| 05r | failed | `verifier_repair_pass_invalid: repair reply must contain a JSON object` |
| 06 | failed | `test edit requires SemanticRepairPlan (spec_authority + repair_hypothesis)` |
| 07 | failed | `verifier repair exhausted: no safe repair target remains` |
| 08 | failed | `verifier repair exhausted: no safe repair target remains` |
| 09 | failed | `verifier repair exhausted: no safe repair target remains` |
| 10 | failed | `verifier repair exhausted: no safe repair target remains` |

### PAM あり

| run | result | terminal reason |
| --- | --- | --- |
| 01 | failed | `verifier repair exhausted: no safe repair target remains` |
| 02 | success | `tests/test_main.py` 修復後に verifier pass |
| 03 | failed | `verifier repair exhausted: no safe repair target remains` |
| 04 | failed | `verifier repair exhausted: no safe repair target remains` |
| 05 | success | `app/main.py` 修復後に verifier pass |

### 観測した改善

- 成果物生成はおおむね verifier 到達まで進む。
- PAM ありでは 5 回中 2 回成功し、過去の 0 成功状態よりは改善が見えた。
- 危険な test weakening や authority 不足の test edit は validator が止めている。
- safe stop として `no safe repair target remains` を出せるため、無制限に壊し続ける状態は避けられている。

### 残る構造課題

現時点の主課題は、生成フェーズではなく verifier repair の収束性。

- diagnostic が malformed になりやすい。
- accepted diagnostic でも、repair target が README に偏ることがある。
- missing dependency/config failure を setup 側へ直せず、README や test 修復へ逸れることがある。
- target が正しくても、repair proposal が JSON object / intent / exact edit の検証で落ちる。
- invalid repair 後の次アクションが弱く、同じ target で invalid proposal を繰り返しやすい。
- `SemanticRepairPlan` が必要な場面で構築できず、安全側に止まるが完了には至らない。

### 評価から見た次の優先課題

1. verifier repair を「diagnose -> plan -> patch -> verify -> progress判定」の単一 job として閉じる。
2. diagnostic 出力を最初から RepairPlan schema に固定し、malformed 時は同じ文脈で即リトライする。
3. missing dependency/config、import/runtime error、assertion mismatch、test isolation failure を failure category として RepairPlan に保持する。
4. invalid repair 後は、同じ target への再試行だけでなく、reject reason を入れた re-plan を必須にする。
5. `SemanticRepairPlan` を後段 validator の要求ではなく、repair plan の必須フィールドへ上げる。
6. PAM は成功率改善の兆候があるが、repair pipeline の構造欠陥を単独では解消しない。PAM は過去成功例と reject reason の retrieval に限定し、主制御は RepairJob 側に置く。
