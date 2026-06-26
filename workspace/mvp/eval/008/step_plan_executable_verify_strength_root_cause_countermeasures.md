# step-plan executable_plan / verify_strength 根本原因分析と対策案

## 目的

`workspace/mvp/eval/008/step_plan_executable_verify_strength_current_state.md` で整理した通り、MVP 版 step-plan は成功率と artifact ownership は高い一方で、`executable_plan avg` と `verify_strength avg` が伸び切っていない。

この文書では、現行アーキテクチャと設計思想に照らして根本原因を掘り下げ、評価専用の場当たり対策ではなく、通常利用の計画品質を上げる対策方針を整理する。

## 前提にする設計思想

このリポジトリの現在の方針は以下。

- local-first coding agent として、小さい loop / prompt / planner を優先する
- provider abstraction を増やさない
- 旧アーキテクチャへ戻す方向の継ぎ足しはしない
- plan は LLM に作らせるが、実行可能性と安全性は deterministic な parser / lint / verify policy で支える
- 新機能や品質改善は E2E 寄りの検証を伴わせる

したがって、対策は「planner を巨大化する」「provider 別の特別処理を増やす」「eval scenario 名に依存する」方向ではなく、次の層で小さく行うべき。

- 作成済み plan を deterministic に自己チェックする
- 自己チェック結果のうち高信頼なものだけ corrective retry に戻す
- planner system/user prompt は、自己チェックで期待する契約を短く補助する
- profile guidance は plan 全体の制約として明示する
- lint は安全性と最低限の実行可能性を守り、品質チェックは段階的に warning / retry へ載せる
- eval は改善の検出と回帰検知に使い、runtime logic を eval に合わせて歪めない

## 現行アーキテクチャ上の流れ

step-plan は概ね次の流れで作られる。

1. `planner::runner` が system prompt / user prompt を組み立てる
2. planner provider が StepPlan JSON を返す
3. parser / repair が YAML 化可能な StepPlan に整える
4. `planner::lint` が step kind / expected_paths / verify / dependency order を検査する
5. 成功した plan を YAML として保存する
6. plan-run / ultra-plan-run では各 step を minimal loop に渡して実行する

現在の設計では、lint は「危険な verify」「順序違反」「所有権重複」「step kind 違反」を止める役割が中心であり、「強い verify を選べているか」は主に prompt と eval scoring に委ねられている。

## A/B/C で見た現在の重心

対策対象を次の3つに分ける。

- A. プランを作る
- B. 作成したプランをチェックする
- C. チェック結果を反映する

現在の対策案で最も重心を置くべきなのは B と C。

理由は、今回の失敗傾向が「壊れた plan が出る」ではなく、「valid だが弱い plan が通る」ことだから。planner prompt を改善して A を良くするだけでは、LLM の出力品質に依存し続ける。現行設計に合う根本対策は、valid plan を deterministic に自己チェックし、品質不足が明確な場合だけ bounded retry に戻すこと。

A は不要ではないが、B/C を支える補助策として扱う。

## 根本原因

### RC-01 valid だが弱い plan を止める自己チェックが不足している

現行 lint は、schema / step kind / path ownership / verify safety / dependency order を検査する。一方で、次のような plan は valid として通りやすい。

- verify が存在確認だけ
- code task なのに test / build / smoke がない
- framework task なのに build がない
- verify が expected artifact と結びついていない
- implement instruction が抽象的で、plan-run で実行指示として弱い

これは「危険ではないが実行力が弱い plan」を見逃している状態。`verify_strength` や `executable_plan` の低さは、その見逃しが eval 上に表れたもの。

### RC-02 quality warning が修正ループに十分つながっていない

現行には `step_plan_quality_warnings` があるが、主に event 出力であり、合格済み plan の作り直しには強く効いていない。

schema error / lint error は corrective retry に戻る。一方で、quality warning は「記録されるが、その plan は採用される」挙動に近い。

今回必要なのは、すべての warning を hard error にすることではない。高信頼な warning だけを retry 可能な quality issue として扱い、低信頼なものは event 記録に留める二段階化。

### RC-03 profile guidance が plan 全体のチェック条件として効き切っていない

Next.js profile には `package.json`, `src/app/page.tsx`, `src/app/layout.tsx`, `src/app/global.d.ts`, `next/react/react-dom`, `next build`, port 3011 などの重要条件がある。

しかし現状では、profile expected paths は user prompt に入る一方、profile guidance の一部は step instruction の後段補強に寄っている。つまり、planner から見ると「plan 全体の verification contract」ではなく「追加の参考情報」に近い。

そのため、artifact は揃っても verify が `npm run build` まで到達しない、または build を置く順序が不安定になる。

### RC-04 safety / ordering と strong verify の間に摩擦がある

`npm run build` のような強い verify は望ましいが、現行 lint は dependency manifest と entrypoint の順序を守らない build を拒否する。

これは正しい安全装置だが、planner に安全な強 verify の型が明示されていないと、planner は次のどちらかに倒れやすい。

- build を早く置いて lint に落ちる
- lint 回避のために弱い verify へ逃げる

根本的には「強い verify を要求する」だけでは不足で、「安全に通る強 verify の配置パターン」を planner に与える必要がある。

### RC-05 executable_plan は path ownership だけでは上がらない

MVP は expected_paths の所有権をかなり安定して扱えている。一方で、`executable_plan_score` は以下にも左右される。

- implement step の instruction が具体的か
- setup / implement / verify の責任境界が自然か
- verify が expected artifact と結びついているか
- framework の実行前提が plan 内で満たされているか

現状は artifact ownership が高い一方で、instruction と verify の結合が弱いケースが残っている。このため、所有権は高くても executable と verify が伸びない。

### RC-06 eval 指標は改善されたが、plan-run 予測力の検証がまだ不足している

追加された `constraint_coverage_score`, `verify_strength_score`, `artifact_ownership_score`, `lint_repair_score` により、従来より問題の見え方は改善した。

ただし、step-plan の点数が実際の plan-run 成功率をどれだけ予測できるかは、まだ十分に校正されていない。

特に問題になるのは以下。

- 高スコアなのに plan-run が失敗する false positive
- 低スコアだが plan-run が成功する false negative
- verify_strength だけ上げた結果、lint/retry が増えて実行安定性が下がるケース

### RC-07 anvildev 比較では policy mismatch を分離する必要がある

anvildev は `node smoke-check.js`, `python -m unittest ...`, `npm run build` などで safe allowlist / ordering に落ちるケースがある。

これは MVP の planner 品質だけでなく、移植元バイナリ側の verify policy と eval suite の期待が噛み合っていない問題を含む。

したがって比較では、次を分けて見る必要がある。

- planner が生成した YAML の品質
- runtime policy により YAML が保存されなかった失敗
- verify command policy の差分
- plan-run へ進めた場合の実行成功率

## 対策方針

### 方針 A: plan quality self-check を第一級化する

主対策は、作成済み StepPlan に対して deterministic な quality self-check を追加すること。

チェックは schema/lint の延長だが、すべてを hard error にしない。以下のように分類する。

- fatal: 既存 lint と同じく採用不可
- retryable_quality: 高信頼で修正可能な品質不足。bounded retry に戻す
- advisory: 判断が揺れる品質懸念。event と eval に残すが採用は妨げない

最初に retryable_quality にしてよい候補:

- framework profile が build/test expectation を持ち、setup と entrypoint の順序が満たせるのに deterministic verify がない
- code task で test/build/smoke/compile のいずれもなく、成果物だけで挙動確認できない
- docs task で要求された heading / phrase / section の content assertion がない
- verify が expected_paths と明らかに無関係
- implement instruction が expected_paths に何を書くかを説明していない

これにより「valid だが弱い plan」を A の prompt 運に任せず、B で検出できる。

### 方針 B: 高信頼な quality issue だけ corrective retry に反映する

quality issue をすべて hard lint にすると不安定になる。そこで、次の制約で C を実装する。

- retry は既存の schema/lint corrective retry と同じ最大回数枠に収める
- retry prompt は「何が弱いか」と「安全制約」を短く渡す
- 修正で safety lint を弱めない
- retryable_quality の発火回数と reason を eval event に残す
- retry 後に fatal lint が増えた場合は、既に得られている valid plan を保持する
- retry 後も改善しない場合は、最後の valid plan を採用するか失敗にするかを issue 種別で分ける

初期実装では、quality issue exhausted を新たな失敗扱いにしない。既存 fatal lint を通過した valid plan がある場合は、その plan を保持して event に残す。失敗扱いへの昇格は、blind eval と plan-run predictiveness で false positive が減ることを確認した後の別判断にする。

### 方針 C: planner prompt は self-check と同じ契約を短く補助する

planner prompt に、タスク種別ごとの verify 強度の優先順位を追加する。

追加する内容は provider 依存ではなく、汎用の contract として扱う。

- docs task: 存在確認だけでなく、要求された heading / phrase / section を `grep -q` などで確認する
- Python task: 可能なら `python3 -m unittest ...` を使う
- JavaScript task: 実行可能 smoke script または構文確認を使う
- Rust task: `cargo test` を使う
- Next.js task: setup と entrypoint 作成後に `npm run build` を置く
- file existence check は最後の fallback として扱う

ここで重要なのは、特定 eval scenario の名前や expected artifact 文字列に寄せないこと。prompt / profile / artifact の性質から自然に導く。

prompt 強化は必要だが、主対策ではない。B/C の self-check で期待する品質を、planner が最初から満たしやすくするための補助策として扱う。

### 方針 D: profile verification expectations を user prompt と self-check の両方に出す

Next.js profile の guidance を、最後の step instruction 補強だけに閉じ込めず、user prompt の profile section として明示する。

例:

- required artifacts
- dependency manifest expectations
- app entrypoint expectations
- deterministic verify expectation
- forbidden verify pattern

Next.js なら以下のような順序を planner に明示する。

1. setup step で `package.json` と必要な設定を所有する
2. implement step で `src/app/page.tsx`, `layout.tsx`, `global.d.ts` を所有する
3. verify step で `npm run build` を実行する
4. dev server readiness は必要なら postcheck 側で扱い、step-plan verify では長時間起動しない

加えて、self-check 側でも profile expectation を見る。つまり、profile は「planner への参考情報」ではなく、「生成後チェックの根拠」にもする。

### 方針 E: weak verify は warning / retry から始め、hard lint 化しない

いきなり hard lint にすると成功率や lint_repair が悪化する可能性が高い。

初期段階では `step_plan_quality_warnings` を拡張するか、別の `step_plan_quality_issues` を追加し、retryable / advisory を明示する。

候補:

- framework task で `test -f` / `cat` しかない
- code task で build/test/unit/smoke がない
- docs task で content assertion がない
- verify command が expected_paths と無関係

hard error 化は、blind eval と plan-run predictiveness で安定性を確認した後に限定する。

### 方針 F: executable_plan は instruction/path/verify の結合で改善する

planner prompt に以下を追加する。

- implement instruction は expected_paths ごとに何を書くかを明記する
- verify は直前までに作成された artifact を検証する
- report step は成果物作成を持たない
- inspect step は未知状態確認だけに使い、既知の required artifact 作成を先延ばししない

これは現行の step kind contract と整合する。新しい abstraction は不要。

### 方針 G: eval は予測力と安定性を見る

step-plan 単体の平均点だけではなく、以下を併用する。

- `plan_run_predictiveness`
- false positive / false negative
- `stability_score`
- verify command category breakdown
- lint repair reason breakdown

改善判定では、verify_strength の平均だけを追わない。強い verify を入れたことで plan-run 成功率が下がるなら失敗とみなす。

### 方針 H: anvildev 比較は policy mismatch を明示分類する

anvildev の失敗は、MVP の対策判断に直接混ぜない。

eval 側では以下の分類を分ける。

- `planner_lint_error`
- `verify_command_policy_error`
- `dependency_order_error`
- `no_plan_artifact_due_to_generation_failure`

これにより、planner の質と runtime policy の違いを分離して比較できる。

## 影響範囲

想定する変更範囲:

- `mvp/anvilminimal/src/planner/lint.rs`
  - 既存 fatal lint は維持する
  - quality self-check を追加または分離する
- `mvp/anvilminimal/src/planner/runner.rs`
  - quality issue event
  - retryable quality issue の bounded retry
  - retry 悪化時の last valid plan 保持
- `mvp/anvilminimal/src/planner/profile.rs`
  - profile expectation を self-check に渡す薄い入口
- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
  - Next.js profile expectation の再利用
- `mvp/anvilminimal/scripts/eval_lib/*`
  - quality retry reason、predictiveness、reporting の可視化
- `mvp/anvilminimal/tests/*`
  - unit / eval script / live eval 受け入れテスト

変更しない範囲:

- provider client / provider abstraction
- minimal loop の tool execution 本体
- bash safety policy の根本変更
- dev server 起動を step-plan verify に入れる挙動
- eval scenario 名に依存した runtime logic

この範囲に留めることで、plan quality 改善が minimal loop や provider 挙動へ波及しすぎることを避ける。

## 移植漏れ・移植不備の観点

現時点の整理では、今回の主問題は「移植元に存在した特定 safeguard の取りこぼし」とは断定しない。

理由:

- anvildev 側は safe allowlist / dependency ordering の policy mismatch で YAML 保存前に落ちるケースがある
- MVP 側は YAML 生成成功率と artifact ownership は高い
- 問題は、valid plan の品質不足を B/C で扱う閉ループが弱い点にある

ただし、実装前に次の観点で移植元との差分確認を行う。

- plan 生成後の quality warning / retry 相当機構が移植元に存在するか
- verify command allowlist / policy が MVP と意図的に違うのか、取りこぼしなのか
- profile guidance / profile verify / postcheck の責務分担が一致しているか
- plan-run へ渡す step instruction の補強が移植元に存在するか
- eval failure classification が移植元 failure を過度に不利に扱っていないか

この確認で移植漏れが見つかった場合は、quality self-check 実装計画に取り込み、単なる prompt tuning として処理しない。

## 不確実性と LLM API 検証方針

現在の対策は、provider API の挙動仮説に依存していない。主な変更は local の deterministic self-check / bounded retry / event 記録であり、OpenAI/Gemini/Ollama の API 仕様確認を必要とする不確実性は今のところない。

LLM API を使って仮説検証すべき条件は以下に限定する。

- prompt contract 変更後、特定 provider だけ schema-valid plan を返さなくなる
- quality retry prompt が特定 provider で tool/schema 破綻を誘発する
- Gemini/OpenAI の tool/function call 仕様差が planner output shape に影響している疑いが出る

それ以外は、live eval と saved YAML の定性確認で検証する。

## 実施フェーズ案

### Phase 0: baseline 固定

- 現在の MVP / anvildev step-plan 結果を baseline として保存する
- `mvp-smoke.yaml` と `mvp-blind.yaml` の両方で現状を確認する
- low verify / low executable の YAML を代表例として保存する
- 移植元との差分確認観点を issue 化せずに済む範囲で確認する

### Phase 1: quality self-check の分類設計

- `fatal`, `retryable_quality`, `advisory` の分類を定義する
- weak verify / artifact coupling / instruction specificity / profile expectation を判定対象にする
- 判定理由を stable な category と message で event 出力する
- 既存 lint の fatal 判定とは混ぜず、段階的に扱う

### Phase 2: quality issue corrective retry

- `retryable_quality` を既存 planner retry loop に戻す
- retry prompt は品質不足と hard constraints のみを短く渡す
- retry 回数、reason、改善有無を eval event に残す
- retry 後に fatal lint が増えた場合は、既に得られている valid plan を保持する
- quality issue exhausted は初期実装では失敗扱いにせず、valid plan を保持する
- quality issue を失敗扱いへ昇格する場合は、別フェーズで predictiveness と blind eval の根拠を要求する

### Phase 3: profile expectation self-check

- Next.js profile の required artifacts / dependency / entrypoint / build expectation を quality self-check に接続する
- `npm run build` は setup と entrypoint 後にある場合だけ強 verify とみなす
- dev server 起動は verify ではなく postcheck 側に残す

### Phase 4: prompt contract の最小補強

- planner system prompt に verify strength の汎用優先順位を追加する
- `build_step_plan_user_prompt` に profile verification section を追加する
- implement instruction と expected_paths の対応を prompt で明示する
- verify が expected artifacts を検証することを prompt に追加する
- provider 別分岐は追加しない

### Phase 5: E2E 検証

- MVP step-plan speed-cloud を実行する
- blind suite step-plan を実行する
- `--modes step-plan,plan-run` で predictiveness を確認する
- local LLM 未使用と local LLM 使用ケースを分けて見る

### Phase 6: score / lint calibration

- false positive / false negative を確認して score weight を微調整する
- warning を hard lint にするかは、回帰がない場合だけ検討する
- anvildev 比較は policy mismatch を別分類で集計する

## 受け入れ条件

最低条件:

- MVP step-plan `mvp-smoke.yaml` speed-cloud が `12/12` 成功を維持する
- retryable quality issue が event と summary で追跡できる
- quality retry により fatal lint failure が増えない
- quality retry の悪化で、既に得られている valid plan が失われない
- `verify_strength avg` が baseline `62.2` から改善する
- `executable_plan avg` が baseline `78.3` から改善する
- `artifact_ownership avg` が `95.0` 以上を維持する
- `lint_repair avg` が `85.0` 未満に悪化しない
- Next.js task/profile で setup / implement / verify の順序が自然になり、build expectation が plan 上で確認できる
- `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check`、Rust unit tests、Python eval tests が通る

回帰防止条件:

- blind suite で success rate が悪化しない
- plan-run predictiveness の false positive が増えない
- quality retry が無限ループ化せず、既存の retry 上限内に収まる
- advisory issue だけでは provider call を増やさない
- warning をスコア改善目的で過剰に hard lint 化していない
- provider 別の特別処理を追加していない
- eval scenario 名に依存した runtime / planner logic を追加していない
- dev server 起動のような長時間処理を step-plan verify に入れていない
- step-plan 以外の plan-run / ultra-plan-run の基本経路を壊していない

## テスト計画

### Unit

- quality self-check の `fatal` / `retryable_quality` / `advisory` 分類
- `planner::lint` の既存 fatal lint が維持されること
- weak verify が retryable または advisory に分類されること
- Next.js build verify の ordering
- Python unittest / JS smoke / docs content assertion の verify policy
- profile verification section の prompt assembly snapshot
- quality retry prompt が hard constraints を保持すること
- retryable quality が改善された場合だけ新 plan を採用すること
- retry が fatal lint に悪化した場合に last valid plan を保持すること
- advisory のみでは retry しないこと

### Static / Full Tests

- `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check`
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml`
- 必要に応じて `cargo clippy --manifest-path mvp/anvilminimal/Cargo.toml -- -D warnings`

### Eval script

- `verify_strength_score` の command category scoring
- `executable_plan_score` の instruction/path/verify coupling
- `constraint_coverage_score` が Next.js required artifacts と build expectation を拾うこと
- failure classification が anvildev policy mismatch を分離すること
- quality event がない既存 run でも summary/report が壊れないこと
- quality event ありの fake event から retry count / degraded count を集計できること

### Live eval

- MVP: `speed-cloud`, `--modes step-plan`, local LLM 未使用
- MVP: `speed-cloud`, `--modes step-plan,plan-run`, local LLM 未使用
- MVP: blind suite, local LLM 未使用
- anvildev: `--engine minimal`, `--modes step-plan`, local LLM 未使用
- MVP: ultra-plan-run smoke を少数ケースで確認し、StepPlan 変更が ultra 経路に副作用を出していないことを見る

### 定性確認

- low score YAML を実際に読む
- Next.js で setup -> implement -> verify の順序が自然か確認する
- verify が「存在確認」ではなく「要求された挙動」を見ているか確認する
- plan-run で各 step が minimal loop に渡されたとき、曖昧すぎる instruction になっていないか確認する
- retry 前後の YAML を比較し、品質改善が自然で eval 専用の形になっていないか確認する

## リスクと対策

### R-01 強 verify に寄せると lint failure が増える

対策:

- 最初は quality issue の warning / retry に留める
- `npm run build` は Next.js setup / entrypoint 後に置く型を明示する
- lint hardening は E2E で安定後に限定する

### R-02 eval に過剰適応する

対策:

- scenario 名や固定ファイル名に依存しない
- profile / artifact / prompt feature から verify を導く
- blind suite と plan-run predictiveness を必須にする

### R-03 planner prompt が肥大化する

対策:

- verify guidance は短い command palette と優先順位だけにする
- provider 別の分岐や長い例示を増やさない
- repair prompt は hard constraints と weak verify warning の要点だけにする

### R-04 quality retry が不安定化する

対策:

- retryable_quality は高信頼 issue に限定する
- retry 回数は既存上限を使い、専用の無制限ループを作らない
- retry 後に fatal lint が増えた場合は、その retry 結果を採用しない
- retry reason と改善有無を event に残す

### R-05 verify が長時間化する

対策:

- dev server 起動は verify に入れない
- build / unit / smoke の deterministic command を中心にする
- dev server readiness は postcheck 側で扱う

## 結論

現在の低めの `verify_strength` と `executable_plan` は、MVP の planner が壊れているというより、valid だが弱い plan を自己チェックして修正ループへ戻す仕組みが弱いことが主因。

対策は、provider 別処理や旧アーキテクチャの復元ではなく、現行の小さい planner / deterministic lint / E2E eval の設計に沿って、B/C を中心に行うべき。

優先順位は以下。

1. 作成済み plan の quality self-check を定義する
2. 高信頼な quality issue だけ bounded corrective retry に戻す
3. profile verification expectations を self-check と prompt の両方へ出す
4. planner prompt は self-check を満たしやすくする範囲で最小補強する
5. plan-run predictiveness と blind eval で過剰適応を防ぐ

## 実施結果と方針更新

実施結果の詳細は以下に記録した。

- `workspace/mvp/eval/008/implementation_results.md`

今回の実装では、上記方針 A〜E を MVP に反映した。

- fatal lint と quality self-check を分離
- `retryable_quality` / `advisory` を stable event と summary に出力
- Next.js profile expectation を self-check と prompt に接続
- retryable quality を既存 planner retry 上限内で corrective retry
- retry 悪化時に last valid plan を保持
- attempt が残っている場合は degraded lint/schema retry を継続
- shell control syntax retry prompt に代替案を追加

この結果、最新 smoke step-plan では以下まで改善した。

- success: 12/12
- executable_plan avg: `79.5`
- verify_strength avg: `68.6`
- artifact_ownership avg: `98.0`
- lint_repair avg: `95.2`

blind step-plan でも success 12/12 を維持したため、eval scenario 名への直接適応ではないと判断する。

ただし、plan-run predictiveness と ultra smoke は未達。

- plan-run predictiveness: step-plan 24/24 success に対し plan-run 1/24 success
- ultra smoke: 0/12 success

これらは「作成済み plan を強くする」今回の B/C 対策だけでは解けない。次に扱うべき根本原因は、minimal loop 実行時の tool validation / missing tool call / max_iterations と、ultra phase scaffold / schema の安定性である。
