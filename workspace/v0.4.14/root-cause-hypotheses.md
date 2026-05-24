# v0.4.14 構造的問題・根本原因・解決仮説

## 目的

この文書は、これまでの評価・コード確認・qwen3.6 実診断検証を踏まえ、Anvil が FastAPI CRUD + README + test タスクを安定完了できていない構造的原因と、解決に向けた仮説を整理する。

対象は特定の FastAPI テンプレート対策ではなく、ローカル LLM 特化 coding agent として汎用的に使える repair architecture の整理である。

## 現時点の観測事実

### 10 回評価の結果

- PAM なし 10 回評価で成功は 1 回。
- 10 回すべてで `app/main.py`, `tests/test_main.py`, `README.md` は生成された。
- 10 回すべてで verifier に到達した。
- 失敗 9 回はすべて `verifier_failed`。
- 主な失敗は以下に集中した。
  - `POST` の期待 status が 200 だが実装は 201。
  - `DELETE` の期待 status が 200 だが実装は 204。
  - in-memory state が test 間で漏れる。
  - test が実装に存在しない `get_db` や `/health` を仮定する。
  - repair proposal が validator に reject され続ける。

このため、現時点の主問題は「成果物を作れない」ことではなく、「verifier failure から完了まで修復を収束させられない」こと。

### qwen3.6 実診断検証の結果

実際の verifier failure 3 ケースを `qwen3.6:27b-coding-nvfp4` に渡し、`RepairPlan JSON` を出せるか検証した。

検証条件:

- Ollama API `/api/generate`
- `format: "json"`
- `think: false`
- `temperature: 0`
- `num_predict: 1600`
- prompt 先頭に `/no_think`

結果:

| ケース | JSON 妥当性 | 診断品質 | 主な出力 |
| --- | --- | --- | --- |
| state leak | pass | 状態漏れ原因を特定 | `app/main.py` 修正を提案 |
| `get_db` 未定義 | pass | test が存在しない内部依存を仮定していると診断 | `tests/test_main.py` 修正を提案 |
| 複合 failure | pass | status mismatch、`/health` 欠落、state leak を分離 | `app/main.py` と `tests/test_main.py` 修正を提案 |

同じ複合 failure を再実行した結果、cluster と repair target は同一で、出力 JSON も一致した。

一方で、`format: "json"` だけの実行では qwen3.6 が `thinking` に token を使い切り、`response` が空になった。したがって、実運用では `/no_think` / `think:false` / token 上限 / schema validation が必須。

### 追加 LLM 仮説検証の結果

詳細は [ローカル LLM 仮説検証結果](llm-hypothesis-verification.md) を参照。

追加で確認できたこと:

- Rust Counter と Python data analysis の小規模 failure でも qwen3.6 は valid な `RepairPlan` を出せた。
- Rust Counter では `RepairPlan -> patch JSON -> cargo test pass` まで成功した。
- 実 FastAPI 複合 failure では patch JSON は valid だったが、生成された Python に `IndentationError` が入り、patch 実行は失敗した。
- 曖昧仕様は hard rule 付き schema なら `spec_ambiguity` + `repair_steps=[]` にできた。
- PAM は task_signature / candidate_summary_ids が合えば relevant memory を返すが、通常 request では 0 item の場合がある。
- PAM context を入れると ambiguity 認識は改善したが、patch step 抑止まではできなかった。
- Ollama API では `think:false` が必須で、`/no_think` だけでは response 空 / length stop になり得る。

## ブラッシュアップした中心仮説

### 仮説 1: qwen3.6 は repair diagnosis の「生成器」として使える

確度: 高い。ただし条件付き。

今回の検証では、qwen3.6 は実 verifier failure から構造化 JSON を返せた。複数 failure の分離、repair target の選択、test bug と implementation bug の区別も一定程度できていた。

ただし、これは次の条件を満たす場合に限る。

- thinking を抑止する。
- JSON schema を明示する。
- verifier output と関連ファイル抜粋を十分に渡す。
- 出力を Anvil 側で schema validation する。

修正後の仮説:

> verifier failure の意味解釈と repair plan 作成は、deterministic pattern matching より qwen3.6 の構造化診断に寄せる。ただし qwen3.6 は最終判断者ではなく、bounded `RepairPlan` の候補生成器として扱う。

追加検証により、FastAPI 以外の小規模 Rust / Python data analysis failure でも成立する可能性が上がった。ただし大規模 repo や複数依存の failure は未検証。

### 仮説 2: LLM に spec の最終決定まで任せるのは危険

確度: 高い。

qwen3.6 は `201 vs 200` や `204 vs 200` を `spec_ambiguity` とせず、test を source of truth として実装修正を提案した。

しかし CRUD API では、以下はいずれも成立する。

- `POST /items` が 201 Created を返す。
- `POST /items` が 200 OK を返す。
- `DELETE /items/{id}` が 204 No Content を返す。
- `DELETE /items/{id}` が 200 OK + body を返す。

ユーザー要求が詳細 status code を指定していない場合、LLM の判断だけで一方を正とするのは危険。

修正後の仮説:

> LLM は spec conflict を検出し、候補を出す。Anvil は `user_request > explicit README/spec > generated tests/implementation consistency` のような authority policy で採用可否を検証し、決められない場合は `SpecAmbiguous` にする。

追加検証では、authority schema だけでは LLM が `ambiguous` と判定しつつ repair step を出すことがあった。したがって `SpecAmbiguous` 時の repair step 抑止は prompt ではなく Anvil 側 validator の責務にする必要がある。

### 仮説 3: RepairJob は単発 edit ではなく、完了まで管理する状態機械であるべき

確度: 高い。

現状は以下がバラバラに動いている。

- verifier parser
- diagnostic LLM
- patch proposal
- deterministic repair candidate
- validator
- progress classifier
- retry / exhausted 判定

このため、複合 failure では「今どの failure cluster を直しているのか」「この patch は何を減らす予定か」「reject された理由から次にどう切り替えるか」が薄い。

修正後の仮説:

> verifier failure が起きたら `RepairJob` を作り、その中で `FailurePacket -> RepairPlan -> RepairStep -> PatchProposal -> Validation -> VerifierProgress` を管理する。通常ループの retry ではなく、repair job 専用の小さな状態機械で収束させる。

追加検証により、`RepairPlan` が正しくても patch 出力自体が壊れることが確認された。よって状態機械には patch-local cheap check / syntax check / formatter / verifier delta を含める必要がある。

### 仮説 4: FailurePacket は repair の唯一の入力に近づけるべき

確度: 中から高。

現状コードには `FailurePacket` があるが、production 経路で `affected_cases` や `observed_expected_pairs` が空になる箇所がある。

これでは、LLM に「構造化して考えさせる」設計にしても、入力が薄いため excerpt 依存になる。

修正後の仮説:

> verifier output はまず `FailurePacket` に正規化し、diagnostic LLM / validator / safe stop / telemetry は同じ packet を参照する。LLM prompt は raw log ではなく、packet + 必要最小限の file excerpt を主入力にする。

### 仮説 5: deterministic repair rule は主役から降ろすべき

確度: 中。

Python / pytest / FastAPI 近傍の deterministic candidate は、今回のタスクでは有効な場面もある。ただし、外れた場合に別ルールを足したくなり、長期的にはルールベース化しやすい。

一方で、deterministic 処理は不要ではない。必要なのは役割の変更。

修正後の仮説:

> deterministic 側は「意味判断」ではなく「安全境界」に寄せる。具体的には path scope、secret 混入、tool markup、assertion deletion、owned artifact、patch size、schema validation、verifier progress 判定を担当する。repair の意味解釈は LLM の structured plan に寄せる。

実評価ログでは diagnostic 受理後も旧 controller repair が主導し、局所 patch / reject / exhausted に流れている。干渉は可能性ではなく実際に起きているため、新 pipeline を main path にしたうえで旧 deterministic repair を fallback / validator assist に降格する必要がある。

### 仮説 6: invalid repair は retry 消費ではなく plan update の入力にすべき

確度: 高い。

今回の失敗では、invalid proposal が reject され続け、最後に `no safe repair target remains` になるケースがあった。

reject 自体は安全性として正しい。しかし reject reason が次の方針変更に十分使われていない。

修正後の仮説:

> validator reject は `RepairJob` の観測事実として記録し、次の LLM diagnostic / plan revision に渡す。同じ target の同じ invalid intent が続く場合は target switch、plan re-evaluation、または safe stop に遷移する。

### 仮説 7: safe stop は失敗終了ではなく診断成果物にすべき

確度: 高い。

現状の `verifier repair exhausted` は、ユーザーが次に何をすべきか分かりにくい場合がある。今回も後から pytest を再実行しないと最終原因が見えない run があった。

修正後の仮説:

> repair が完了できない場合も、最後の verifier summary、failure clusters、採用した source of truth、試した targets、reject reasons、残課題、次の推奨 action を出す。これにより human / 別 agent に引き継げる。

## まだ不確実な仮説

### 不確実性 1: qwen3.6 の RepairPlan は広いタスクでも安定するか

FastAPI 以外として小規模 Rust / Python data analysis は追加検証済み。ただし、CLI、フロントエンド、設定ファイル不備、依存関係問題、大規模 repo では未検証。

追加検証が必要。

### 不確実性 2: qwen3.6 の診断は常に semantic に正しいか

JSON は返せるが、`spec_ambiguity` を見落とす可能性がある。今回も status code conflict を ambiguity とせず、test を正としていた。

したがって、LLM 診断を採用する前に Anvil 側の authority policy と ambiguity detector が必要。

### 不確実性 3: RepairPlan が正しくても patch 実行が成功するか

小規模 Rust では成功したが、実 FastAPI 複合 failure では patch JSON が valid でも Python indentation が壊れて失敗した。

今後は `RepairPlan` から小さな `RepairStep` を作り、1 step ごとに patch + syntax validation + formatting + verifier progress を確認する必要がある。

### 不確実性 4: `/no_think` と `think:false` がすべての環境で効くか

追加検証では、Ollama API の `think:false` が必須だった。`/no_think` だけでは response 空 / length stop になった。

Anvil 側では response が空、thinking が長い、JSON parse 不能、length stop などを明示的に扱う必要がある。

### 不確実性 5: 旧 repair 経路との干渉

新しい `RepairPlan` pipeline を入れても、既存の deterministic candidate や focused recovery が先に発火すると、同じ混乱が残る可能性がある。

どのフェーズを production main path にし、どれを fallback に降格するかを明確にする必要がある。

### 不確実性 6: PAM の寄与

PAM は task_signature / candidate_summary_ids が合えば relevant seed を返し、prompt 注入により ambiguity 認識を改善した。ただし patch step 抑止まではできなかった。

したがって PAM は補助にはなり得るが、RepairJob の構造化と validator なしに根本解決する可能性は低い。

### 不確実性 7: 実行時間とコスト

qwen3.6 の diagnostic は 1 回 20 秒から 40 秒程度かかった。成功率が上がるなら許容できる可能性はあるが、無駄な re-diagnostic を抑える job state が必要。

## 棄却または修正された仮説

### 棄却: `format:"json"` だけで安定する

棄却。

`format:"json"` だけでは `thinking` に token を使い切り、`response` が空になった。実運用では `/no_think`、`think:false`、token 上限、parse failure handling が必要。

### 修正: LLM に完全自由回答させればよい

修正。

自然言語診断ではなく、schema 付きの `RepairPlan JSON` に限定すべき。自由回答は main session の文脈を汚しやすく、制御側が使いにくい。

### 修正: deterministic rule をすべて削除すればよい

修正。

削除すべきなのは意味判断を担う特化 repair rule。安全性、検証、境界管理、progress 判定の deterministic 処理はむしろ必要。

### 修正: verifier failure をそのまま LLM に渡せばよい

修正。

raw log だけでは token と品質の面で不安定。`FailurePacket` に正規化し、必要な file excerpt と reject history を添えて渡す方がよい。

## 解決に向けた設計仮説

### 1. `RepairPlan` pipeline を verifier repair の主経路にする

想定 flow:

1. verifier が失敗する。
2. Anvil が `FailurePacket` を作る。
3. Diagnostic LLM が `RepairPlan JSON` を返す。
4. Anvil が schema / path / security / authority / ambiguity を検証する。
5. 有効な `RepairStep` を 1 つ選ぶ。
6. Patch LLM または既存 edit pass が、その step だけを実装する。
7. Anvil が patch を validation する。
8. verifier を再実行する。
9. progress があれば job を更新し、残り step へ進む。
10. progress がなければ reject reason / verifier delta を加えて plan revision する。
11. 収束不能なら diagnostic safe stop を出す。

### 2. `SpecAuthority` と `SpecAmbiguous` を RepairPlan validation に組み込む

`RepairPlan` は `source_of_truth` を出すだけでは不十分。Anvil が以下を検証する。

- ユーザー要求に明示されているか。
- README に明示されているか。
- 実装と test のどちらがより public behavior に近いか。
- test が private helper や存在しない内部構造を仮定していないか。
- どちらも合理的なら `SpecAmbiguous` とするか、生成物内部整合性を選ぶか。

### 3. Patch は plan から分離する

LLM に「診断してそのまま大きな patch」をさせない。

まず `RepairPlan` を作る。その後、1 step だけ patch する。

これにより、patch が失敗しても診断全体を捨てず、どの step が失敗したかを管理できる。

### 4. validator reject を plan revision に接続する

reject reason は次の prompt に入れる。

例:

- `AssertionDeleted`
- `test edit requires SemanticRepairPlan`
- `target outside owned artifact`
- `missing import symbol`
- `no verifier progress`

これらを job state に保存し、同じ失敗を繰り返さない。

### 5. 特化 repair rule は段階的に降格する

いきなり削除せず、まず production main path から外す。

- Main path: `FailurePacket -> RepairPlan -> validated RepairStep`
- Fallback: 既存 deterministic candidate
- Validator assist: 特化知識を reject / warning の補助に限定

観測したうえで、安全に削る。

## 期待する改善挙動

### 現状

1. verifier が失敗する。
2. target 候補を選ぶ。
3. patch proposal を作る。
4. validator が reject する。
5. retry する。
6. 収束せず exhausted。

### 改善後

1. verifier が失敗する。
2. failure を clusters に分ける。
3. `201 vs 200` / `204 vs 200` を spec conflict として認識する。
4. state leak を別 cluster として認識する。
5. `/health` 欠落や `get_db` 未定義を別 cluster として認識する。
6. authority policy で実装修正 / test 修正 / ambiguity stop を決める。
7. 1 step ずつ patch する。
8. verifier delta で改善したか確認する。
9. 改善しない場合は plan を更新する。
10. 完了できない場合でも、次に何をすべきか分かる safe stop を出す。

## 次に検証すべきこと

1. `FailurePacket` に実際の affected cases / observed expected pairs を詰める。
2. qwen3.6 に `FailurePacket + file excerpt` だけで同等の `RepairPlan` を出させる。
3. `RepairPlan` から 1 step patch を実行し、verifier progress が出るか確認する。
4. ambiguity detection を status code conflict で検証する。
5. 旧 deterministic repair path を main path から外した場合の成功率を A/B 評価する。
6. FastAPI CRUD 以外のタスクで同じ pipeline を評価する。

## 結論

今回の検証で、qwen3.6 を repair diagnosis に使う仮説は補強された。ただし、LLM を最終判断者にする仮説は補強されていない。

最も妥当な方向性は、LLM を `RepairPlan` の候補生成器として使い、Anvil が authority / ambiguity / safety / progress を制御する構造である。

この方針なら、ルールベース修復の拡張ではなく、LLM の汎用推論を活かしつつ、ローカル LLM の不安定さを制御構造で抑える方向に進められる。

## 2026-05-24 追加実装・評価後の仮説評価

今回、`FailurePacket` の拡充、diagnostic LLM の JSON mode 化、`/no_think` 付与、`RepairBrief` / `RepairAction` の authority / ambiguity validation を入れたうえで、PAM なし 10 回、PAM あり 5 回を評価した。

結果:

- PAM なし: 0/10 成功
- PAM あり: 2/5 成功
- 合計: 2/15 成功

### 仮説 1: qwen3.6 は repair diagnosis の生成器として使える

評価: 一部支持。ただし production loop ではまだ不十分。

PAM ありで 2 回成功しており、qwen3.6 の診断が有効な repair target / patch に繋がるケースは確認できた。一方で、diagnostic malformed が複数回発生し、accepted diagnostic でも README 偏りや invalid repair proposal へ流れるケースが多かった。

したがって、qwen3.6 を使う方針は妥当。ただし、診断結果をそのまま repair 実行へ渡すのではなく、schema 固定、reject reason を含めた re-plan、plan quality gate が必要。

### 仮説 2: LLM に spec の最終決定まで任せるのは危険

評価: 強く支持。

今回の実装で曖昧仕様や authority 不足の test edit を validator が止めた結果、危険な test weakening は抑止できた。一方で、`test edit requires SemanticRepairPlan` や `no safe repair target remains` に落ちる run が増えた。

これは「安全に止まる」方向としては正しいが、完了率はまだ低い。次は validator で止めるだけでなく、`SpecAmbiguous` / authority 不足を RepairPlan の再作成入力に戻す必要がある。

### 仮説 3: RepairJob は状態機械であるべき

評価: 強く支持。

失敗の大半は、単発 repair の繰り返しでは収束しないことを示している。

典型例:

- same target で invalid repair proposal を繰り返す。
- README を修復しても verifier failure が減らない。
- `No module named pytest` のような setup/config failure を setup 側へ切り替えられない。
- `no safe repair target remains` で止まるが、次の action が十分具体化されない。

よって、`diagnose -> plan -> patch -> verify -> progress判定 -> re-plan/target switch/safe stop` を 1 つの RepairJob 状態として閉じる必要がある。

### 仮説 4: FailurePacket は repair の主入力にすべき

評価: 支持。ただし、まだ十分に活用できていない。

`FailurePacket` に `affected_cases` / `observed_expected_pairs` / prior attempts を詰める修正は妥当だった。だが評価結果では、diagnostic が target を誤る、または patch proposal が崩れるケースが残った。

つまり、入力の構造化だけでは足りない。`FailurePacket` を RepairPlan schema、validator reject、progress delta と同じ job state に統合する必要がある。

### 仮説 5: deterministic repair rule は主役から降ろすべき

評価: 支持。

今回の失敗では、deterministic / controller-applied repair が安全 validation によって拒否されるケースが多かった。これは安全性としては良いが、意味判断を担う deterministic repair が主経路に残ると、失敗時に target switch や re-plan へ自然に移れない。

deterministic 処理は削除ではなく、以下へ限定する方針が妥当。

- schema validation
- path / ownership / secret / tool markup safety
- weakening detection
- exact patch application
- cheap check / verifier progress classification

意味判断と repair plan 作成は LLM structured plan に寄せる。

### 仮説 6: invalid repair は plan update の入力にすべき

評価: 強く支持。

今回もっとも再現した失敗は invalid repair proposal の連続だった。

- `repair reply must contain a JSON object`
- `repair intent string contained markdown or tool-call markup`
- `old_string was not found`
- `SemanticRepairPlan ... none was constructed`

これらは単なる retry 消費ではなく、「この target / intent / patch shape は失敗した」という観測事実として次の RepairPlan に戻すべき。

### 仮説 7: safe stop は診断成果物にすべき

評価: 支持。ただし未達。

`no safe repair target remains` で無限修復を止められている点は前進。一方で、完了時点の標準出力だけでは、最後の verifier failure、試した target、reject reason、次の推奨 action が十分にまとまっていない。

safe stop を有用な成果物にするには、RepairJob report を明示的に出力する必要がある。

### PAM に関する評価

PAM ありは 2/5 成功、PAM なしは 0/10 成功だったため、PAM には改善の兆候がある。ただし、成功率はまだ低く、repair pipeline の構造問題を PAM 単独で解消できる状態ではない。

PAM は以下に限定して使うのが妥当。

- 過去に成功した failure cluster / repair plan の retrieval
- 過去に reject された repair intent の回避
- framework / project 固有の既知注意点の補助

PAM を主制御にせず、RepairJob の state / evidence / validation / progress を主制御に置くべき。

### 総合評価

当初の大きな方向性は妥当だった。

特に、以下は評価で支持された。

- LLM を意味診断の候補生成器にする。
- LLM を最終判断者にしない。
- deterministic は安全境界に寄せる。
- verifier repair を状態機械として扱う。
- invalid repair を re-plan 入力にする。

ただし、今回の実装だけでは期待挙動には届いていない。理由は、`FailurePacket` と JSON diagnostic を導入しても、RepairPlan / PatchProposal / Progress / RejectReason がまだ 1 つの RepairJob state に閉じていないため。

次の根本対策は、個別の parser や validator を増やすことではなく、repair pipeline を `RepairJob` に一本化し、旧 controller-applied repair を fallback / validator assist に降格することである。
