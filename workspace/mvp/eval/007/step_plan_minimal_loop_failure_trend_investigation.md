# step-plan / minimal-loop Failure Trend Investigation

作成日: 2026-06-26

対象:

- run root: `/tmp/anvilminimal-eval-006-step-minimal-repeat2`
- suite: `mvp-smoke`
- modes: `step-plan,minimal-loop`
- runs: 2
- local LLM: 未使用

## 1. 結論

直近の失敗はランダムではなく、次の 2 系統に集中している。

| mode | 成功率 | 失敗傾向 |
|---|---:|---|
| `step-plan` | 21/24 | planner 出力は JSON shape としては成立しているが、MVP lint / verify policy に落ちる |
| `minimal-loop` | 21/24 | completion contract が成果物の存在だけで完了扱いする、または verify failure 後の修復誘導が弱い |

重要なのは、今回の `step-plan` 失敗は provider schema / parser の問題ではないこと。`planner_raw_output_shape` は全て `json_extract_status=ok` で、`goal` / `steps` も含まれている。問題は、その後の deterministic lint と補正 retry の収束性にある。

また、`minimal-loop` の Next.js 失敗は「生成ファイルがない」ではなく、`npm run build` を loop 内で見ないまま `required_artifacts_satisfied_after_tool` で終了したことが本質である。

## 2. 現象と傾向

### 2.1 step-plan

失敗 3 件。

| scenario | planner | failure kind | 直接の失敗 |
|---|---|---|---|
| `docs-heading-update-small` | OpenAI `gpt-5.4-mini` | `verify_command_policy_error` | verify command に shell control syntax、または `README.md` ownership 重複 |
| `python-markdown-linter-medium` | OpenAI `gpt-5.4-mini` | `planner_lint_error` | `python3 -m unittest ...` が dependency setup / package manifest 前の verify と判定 |
| `repair-exhausted-report-large` | Gemini `gemini-3.5-flash` | `planner_lint_error` | 同じく Python unittest verify が setup 前判定。補正途中で expected path ownership 重複も発生 |

傾向:

- plan の schema は成立している。
- 3 回の補正 retry は実行されている。
- ただし retry ごとに別の lint 違反へ横滑りしており、同じ不変条件を安定して満たす方向へ収束していない。
- Python 標準ライブラリだけで成立する `unittest` タスクを、Node framework と同じ「依存セットアップが必要な verify」として扱っている。

### 2.2 minimal-loop

失敗 3 件。

| scenario | main model | failure kind | stop reason | 直接の失敗 |
|---|---|---|---|---|
| `python-markdown-linter-medium` | Gemini `gemini-3.1-flash-lite` | `verify_repair_no_change` | `verify_repair_no_change` | unittest failure 後、`Read` 2 回のみで編集せず停止 |
| `nextjs-space-invaders-large` r1 | Gemini `gemini-3.1-flash-lite` | `postcheck_failure` | `required_artifacts_satisfied_after_tool` | artifact 存在で終了後、postcheck `npm run build` が TypeScript/依存不整合で失敗 |
| `nextjs-space-invaders-large` r2 | Gemini `gemini-3.1-flash-lite` | `postcheck_failure` | `required_artifacts_satisfied_after_tool` | 同上 |

Next.js postcheck の実体:

- `npm install --ignore-scripts`: 成功
- `npm run build`: 失敗
- dev server readiness: 200 応答
- build failure: `Type error: Option 'moduleResolution=node10' is deprecated ...`

傾向:

- artifact は全て存在するため、MVP loop は成功扱いで止まる。
- しかし framework app としての completion は満たしていない。
- completion contract には `verify_commands: []` が入っており、`npm run build` が loop 内検証から除外されている。
- Python linter は verifier failure の診断は取れているが、その後の修復 feedback が「どのファイルをどう直すべきか」まで十分に収束させられていない。

## 3. 問題箇所

### 3.1 step-plan lint / retry

MVP:

- `mvp/anvilminimal/src/planner/lint.rs`
  - `is_build_verify()` が `python -m unittest` / `python3 -m unittest` / `pytest` まで dependency-order 対象に含めている。
  - `dependency_order` の判定が「manifest/setup が必要な verify」と「標準ライブラリだけで完結する verify」を分けていない。
- `mvp/anvilminimal/src/planner/runner.rs`
  - `build_lint_retry_prompt()` が primary category に応じた一般指示を出すだけで、複数エラーの組み合わせを安定して解消する制約セットになっていない。
  - `verify_policy` / `path_ownership` / `dependency_order` の違反が retry ごとに入れ替わる。

移植元:

- `src/agent/minimal_step_runner/plan_lint.rs`
  - dependency setup before verify の対象は `npm run build` / `npm test` / `npm run test` に限定。
  - Python unittest を dependency setup 必須とは扱っていない。
- `src/agent/minimal_step_runner.rs`
  - retry prompt は `setup` / `verify` / `report` の責務分離、expected_paths の意味、verify 禁止事項を同時に再提示する。

判断:

- Python unittest の dependency-order 失敗は、移植元からの単純な取りこぼしというより、MVP 側で安全側に広げた lint が現実の task class と噛み合っていないことが直接原因。
- ただし、移植元が持っていた「npm に限定した setup-before-verify」と「補正時の複合制約再提示」は MVP に十分反映されていない。

### 3.2 minimal-loop completion contract

MVP:

- `mvp/anvilminimal/scripts/eval-run.py`
  - `completion_contract_for_spec()` が `postcheck.commands` から deterministic verify を抽出する。
  - dependency setup が含まれる場合、`npm run build` など npm/pnpm/yarn 系 verify を completion contract から除外する。
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
  - `required_paths_satisfied_after_tool()` が成立し、completion contract に verify がない場合、`required_artifacts_satisfied_after_tool` で終了する。

移植元 / 現行 source 側:

- `src/agent/loop_run/task_contract_completion_policy.rs`
  - completion は local evidence に基づき、artifact role と verifier class を分けて扱う。
  - `RepoEdit` だけで足りる場合と、`VerifierExitZero` が必要な場合を policy で分離。
- `src/agent/loop_run/completion_evidence.rs`
  - `RepoEdit`, `VerifierExitZero`, docs/data/report checker などの deterministic evidence を分ける。
- `src/agent/minimal_loop/loop_run.rs`
  - minimal loop 移植元そのものにも `early_success_paths` による artifact existence completion は存在する。
  - ただしこれは step runner から渡される「その step の expected path」に対する早期終了であり、eval の postcheck/build completion authority を代替するものではない。
- `src/agent/minimal_step_runner/profiles/nextjs.rs`
  - Next.js create では `scripts.build = next build`、dependency setup、build verification phase を明示。
- `src/agent/minimal_step_runner/verify.rs`
  - `npm run build` 実行前に `node_modules/.bin/next` がなければ `dependency_missing` として扱い、build 成功を偽装させない。

判断:

- MVP の completion contract は、source の task contract / evidence policy をかなり単純化している。
- その単純化自体は MVP 方針として妥当だが、framework profile でも artifact existence だけで完了できるのは source 設計思想とずれている。
- `npm run build` を dependency setup があるから除外する実装は、speed comparison では便利だが、completion authority としては弱すぎる。
- ここでいう「移植不備」は、移植元 minimal loop の `early_success_paths` が欠落したという意味ではない。MVP eval の完了条件に対して、source 側にある task contract / profile verification の completion authority を MVP completion contract へ接続できていない、という分類が正確である。

### 3.3 minimal-loop verify repair feedback

MVP:

- `mvp/anvilminimal/src/minimal_loop/completion.rs`
  - verifier failure を `primary_reason` として feedback 化。
- `mvp/anvilminimal/src/minimal_loop/feedback.rs`
  - `verify_repair_edit_required()` は「編集せよ」と伝えるが、失敗 command、失敗 assertion、候補ファイル、実装/テストどちらを優先すべきかまでは十分に構造化しない。
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
  - verify failure 後に 2 ターン連続で編集がなければ `verify_repair_no_change` で停止する。

移植元 / 現行 source 側:

- `src/agent/minimal_step_runner/repair.rs`
  - repair prompt が「compiler/import/test/assertion errors を fixable feedback として扱い、関連する小さな file range を inspect して concrete Read/Edit/Write fix を行う」と明示。
- `src/agent/minimal_step_runner/verify.rs`
  - verifier failure excerpt に head/tail、diagnostic line、source location 周辺抜粋を含める。
- `src/agent/loop_run/repair_assertion_analysis.rs`
  - assertion mismatch の observed actual/expected を扱う補助ロジックがある。
- `src/agent/loop_run/repair_python_test_analysis.rs`
  - Python test failure の弱い修復や fixture state 問題を検出する補助ロジックがある。

判断:

- `verify_repair_no_change` ガード自体は必要。
- 問題は、ガードに至る前の feedback が source 側ほど具体的でなく、小さい Gemini モデルが `Read` で停滞しやすいこと。
- ここは eval 固有ではなく、実運用でも verifier failure 後の修復成功率に影響する。

## 4. 直接原因

### DC-01: Python unittest を dependency setup 必須 verify と誤分類

`python3 -m unittest test_*.py` は標準ライブラリだけで走ることが多い。今回の suite もその前提である。

しかし MVP の `planner/lint.rs` では Python unittest が `is_build_verify()` に含まれ、setup/manifest がないと lint error になる。

これにより、planner は不要な setup step を足す、または verify step の責務を曖昧にする方向へ retry し、別の lint 違反を誘発している。

### DC-02: lint retry が複数制約を同時に固定しきれていない

OpenAI/Gemini とも、補正 attempt ごとに次の違反へ移っている。

- shell control syntax
- duplicate expected path ownership
- verify step instruction が file change を要求
- dependency setup before verify

retry prompt は primary category に応じた guidance を選ぶが、前回までに出た違反を「全て維持して修正する」ような hard constraints として十分に提示していない。

### DC-03: Next.js minimal-loop completion contract から build verify が消える

`postcheck.commands` に `npm install --ignore-scripts` があるため、`eval-run.py` は npm 系 verify を completion contract から除外する。

結果:

```json
{
  "required_paths": [
    "package.json",
    "src/app/page.tsx",
    "src/app/layout.tsx",
    "src/app/global.d.ts"
  ],
  "verify_commands": []
}
```

この contract では artifact が存在した時点で loop 内成功になる。

### DC-04: verifier failure 後の修復 feedback が target を絞り込まない

Python linter failure では `AssertionError: 2 != 3` まで取れている。

しかし feedback は「同じ signature なので編集せよ」に近く、次ターンで読むべきファイル、修正対象の優先順位、再実行すべき command を十分に拘束しない。

## 5. 根本原因

### RC-01: MVP completion contract が source の completion authority を過度に単純化している

source 側は completion を「ファイルがある」ではなく「役割別 artifact と deterministic evidence が completion policy を満たす」ものとして扱う。

MVP ではこれを `required_paths + verify_commands` に縮約した。この縮約は MVP として理解できるが、framework app / test-required task / generated project のように「ファイル存在だけでは完成でない」タスクでは不足する。

補足:

- 移植元 minimal loop 自体には artifact existence による early success があるため、artifact early success の存在そのものは移植漏れではない。
- 不足しているのは、eval/runtime が要求する build/profile/postcheck 相当の completion authority を、MVP の `CompletionContract` に失わずに伝える設計である。

### RC-02: lint policy が task class 非依存で広すぎる

MVP は安全性を高めるため build/test verify の setup-order を広げたが、Python stdlib unittest のように setup 不要な verifier まで巻き込んだ。

安全側のつもりのルールが、planner に不要な構造を生成させ、結果として成功率を下げている。

### RC-03: source の profile contract が minimal-loop completion に接続されていない

MVP は `nextjs` profile guidance と expected paths は持っているが、minimal-loop の completion 判定には profile verify が入っていない。

source 側の Next.js contract は、少なくとも次を守ろうとしている。

- real Next.js app contract
- `scripts.build = next build`
- dependency setup before build verify
- build verification phase
- dependency missing を成功扱いしない
- Tailwind / alias / tsconfig などの framework 整合性

MVP minimal-loop は guidance に頼っており、local deterministic gate としては弱い。

### RC-04: verifier failure を修復行動へ変換する中間表現が薄い

source 側には failure excerpt、repair prompt、assertion/test analysis、repair progress など、失敗を具体的な修復対象へ寄せる仕組みがある。

MVP 側は「小さい loop」を優先しているため、この層を薄くしている。そのため、小さいモデルが verifier output を読んでも編集へ進まず、`verify_repair_no_change` に落ちる。

## 6. 移植元ロジック確認

過度な eval 適応を避けるため、次の観点で source と MVP を比較した。

| 観点 | 確認内容 | 結果 |
|---|---|---|
| A. completion authority | source は artifact existence だけで完了するか | 通常 loop は `CompletionEvidence` / `CompletionPolicy` を持ち、deterministic evidence を分離。MVP minimal-loop は `required_paths` のみで完了可能 |
| B. framework profile | Next.js の build/dev/profile 整合性をどこで検証するか | source step/ultra は profile verify を phase 後に走らせる。MVP minimal-loop は guidance のみで completion gate には未接続 |
| C. dependency setup order | Python unittest を setup 必須扱いするか | source lint は npm verify に限定。MVP は Python unittest / pytest も対象に含める |
| D. verify command safety | shell control / setup / dev server を verify から除外するか | source/MVP とも安全制約あり。MVP の方向性は妥当 |
| E. repair prompt | verifier failure 後に具体的な file-range inspect / edit を促すか | source repair prompt は具体的。MVP feedback は汎用的で弱い |
| F. failure diagnostics | verifier output から source excerpt / assertion mismatch を抽出するか | source には関連 source excerpt や assertion/test analysis がある。MVP completion feedback は primary reason 中心 |
| G. test coverage | 今回の失敗を単体テストで捕捉できるか | 既存テストは `npm run build` を completion contract から除外する挙動を固定しており、Next.js completion authority 不足を検出しない |
| H. eval overfitting risk | suite 固有の path / command に過適応していないか | 対策は scenario id ではなく task class / profile / verifier kind で設計すべき |
| I. minimal source parity | 移植元 minimal loop の artifact early success と比較したか | early success 自体は移植元にもある。問題は eval completion contract が build/profile evidence を落としている点 |

追加で見つかった不備候補:

1. `python3 -m unittest` を dependency setup 必須扱いする lint は source parity として過剰。
2. Next.js profile verify が minimal-loop completion gate に接続されていない。
3. completion contract 生成が dependency setup を理由に npm build verify を完全除外するため、framework app の deterministic completion authority が抜ける。
4. repair feedback が source の repair prompt より抽象的で、verifier failure から編集対象へ収束しにくい。
5. テストが「build verify を外す」挙動を正として固定しており、eval 設計の完了条件と runtime completion contract の整合性を保証できていない。

今回の範囲で、上記以外に直近失敗へ直接つながる移植漏れは確認できなかった。ただし、plan-run / ultra-plan-run に同じ minimal-loop completion と step-plan lint が波及するため、対策は mode 横断で行う必要がある。

### 6.1 影響範囲

| 対象 | 想定影響 | 注意点 |
|---|---|---|
| `minimal-loop --prompt` | completion contract / profile-aware completion の変更が直接影響 | docs-only / artifact-only task を過剰 verifier 要求にしない |
| TUI 通常入力 | `anvilminimal` TUI からの通常依頼も同じ minimal-loop を使う | profile が未指定の通常入力では generic 挙動を維持する |
| `step-plan` | lint policy / retry prompt の変更が直接影響 | plan-only 評価では実行しないため、過剰に実行都合へ寄せない |
| `plan-run` | step-plan 生成と各 step の minimal-loop 実行の両方が影響 | lint 修正と repair feedback 修正が二重に効く |
| `ultra-plan-run` | phase step-plan と profile verify に波及 | Next.js profile completion と ultra phase profile verify の責務重複に注意 |
| `ultra-step-run` | phase replay の verifier/expected path 判定に影響 | phase snapshot 前提を壊さない |
| eval harness | completion contract 生成と scoring に影響 | dependency elapsed を速度評価から除外する設計は維持する |
| providers | retry prompt 変更が OpenAI/Gemini/Ollama の planner 出力へ影響 | provider 別 special case は避け、fixture/live check で確認する |

## 7. 対策の方向性

### P0: lint policy を source parity 寄りに戻す

目的:

- Python stdlib verifier を不要な dependency setup 要求から外す。

方針:

- `dependency_order` の対象を「依存インストールが必要な可能性が高い verifier」に限定する。
- 最低限、`python -m unittest` / `python3 -m unittest` は setup 必須から外す。
- 初期対応では source parity を優先し、dependency-order lint の対象は npm/pnpm/yarn など package install が明確に必要な verify に絞る。
- `pytest` / `cargo test` は plan lint で一律 setup 必須にせず、verify 実行時の precondition / missing manifest / missing dependency 診断で扱う。

過度な eval 適応を避ける条件:

- scenario id や `test_markdown_lint.py` 固有名では分岐しない。
- verifier command class と workspace state で判定する。
- suite/profile 固有 metadata を lint 判定に直接持ち込まない。必要な場合は completion contract 側の requirement として扱う。

必要なテスト:

- `python3 -m unittest test_app.py` は setup step なしでも lint pass。
- `npm run build` は package/app entrypoint/setup がない場合 lint fail。
- `pytest` / `cargo test` は plan lint では setup-order fail にならず、verify/precondition 側の診断 fixture で扱われる。

### P1: retry prompt を「現在の primary error」ではなく「累積 hard constraints」にする

目的:

- retry の横滑りを減らす。

方針:

- attempt ごとに発生した lint categories を累積し、次 retry に全カテゴリの禁止事項を列挙する。
- `verify step must not own expected_paths unless validating already-owned paths` のように、verify / implement / setup の責務をより明確にする。
- shell control syntax を使った verify は、複数 verify list item へ分割する例を出す。ただし具体 scenario 名は出さない。
- duplicate ownership は「verify-only step は expected_paths を空にするか、既存/先行 step の path を検証対象として説明する」方針にする。
- 具体プロンプト案を作った後、OpenAI/Gemini/Ollama の planner fixture で regression を確認する。必要なら `.env` の API key を使った小さい live check を行い、同一 failure 3 種が改善するかを実装前に確認する。

不確実性:

- retry prompt 改善は LLM 挙動に依存するため、現時点では仮説である。
- 本文書作成時点では具体プロンプト案が未定義のため、LLM API の追加実行はまだ検証対象を持たない。実装計画化時に prompt diff を作ってから検証する。

必要なテスト:

- shell control syntax -> retry 後に verify list item 分割。
- duplicate expected path ownership -> retry 後に verify step の expected_paths が空、または owner が一意。
- dependency_order + ownership の複合エラー fixture が 3 attempt 以内に pass する。
- fixture で改善しない場合は live API で効果を確認し、それでも不安定なら prompt 強化ではなく deterministic normalizer / plan repair を検討する。

### P2: minimal-loop completion を profile-aware にする

目的:

- framework app を artifact existence だけで完了させない。

方針:

- completion contract に `profile` または `profile_checks` を追加し、`nextjs` profile の場合は artifact existence 後に lightweight deterministic profile verify を実行する。
- 最小実装では、既存 `mvp/anvilminimal/src/planner/profiles/nextjs.rs::verify()` を minimal-loop completion に再利用する。
- `profile verify` は `npm install` や dev server 起動を行わず、package/script/entrypoint/tsconfig/css/toolchain の静的整合性を見る。
- `npm run build` は dependency setup が必要なら loop 内で実行しない判断もあり得るが、その場合でも profile verify failure または dependency_missing として loop に feedback する。
- ただし、現行の Next.js profile verify だけでは `typescript` / `@types/*` / Next.js version compatibility や、`tsconfig.json` 未作成時の build-time default までは捕捉できない。今回の build failure はこの領域に該当するため、P2 は P3 と一体で実装する。
- static profile verify を拡張する場合は、package dependency の major version compatibility、`tsconfig.compilerOptions.moduleResolution`、`ignoreDeprecations` などの一般的な framework compatibility を見る。静的に判断できない場合は `deferred_verify_requirements` として build requirement を残す。

過度な eval 適応を避ける条件:

- `nextjs-space-invaders-large` という scenario 名ではなく `--profile nextjs` と generated project shape で有効化する。
- port 3011 は goal/profile guidance の一部として扱うが、completion gate は「指定 port を dev script が保持しているか」を見る。
- `Space Invaders` や `3011` 固有ではなく、Next.js project contract と build requirement の一般条件として実装する。

必要なテスト:

- `nextjs` profile + required paths only + invalid `package.json` は early success しない。
- deprecated / invalid `tsconfig` または broken package/script は profile verify feedback になる。
- `typescript` / `@types/*` / Next.js の明らかな不整合、または `moduleResolution=node10` 系 build risk が static profile verify か deferred build requirement で捕捉される。
- profile が `generic` の docs-only task は従来通り artifact-only completion を許す。

### P3: completion contract generator の npm build 除外方針を見直す

目的:

- dependency setup を理由に build authority が完全に消える状態をなくす。

方針:

- `postcheck.commands` から loop 内に入れられない command を単純に捨てず、`deferred_verify_requirements` または `profile_verify_required` として contract に残す。
- npm build 自体を loop 内実行しない場合でも、次のいずれかを completion gate にする。
  - static profile verify
  - dependency precondition failure feedback
  - postcheck-aligned deferred requirement metadata
- eval harness は `dependency_elapsed_sec` を速度評価から除外しつつ、completion authority から build requirement を消さない。
- `deferred_verify_requirements` は「実行しないが忘れない」ための metadata とし、成功判定へどう使うかを明確にする。たとえば profile verify が pass し、かつ deferred build risk がない場合のみ artifact-only completion を許す。
- build requirement を runtime 内で必ず実行する案は避ける。network/dependency 条件で不安定化しやすく、speed-cloud eval の目的とも衝突する。

必要なテスト:

- `npm install --ignore-scripts` + `npm run build` を持つ scenario で、contract が `verify_commands: []` だけにならない。
- dependency setup command は verify command に混入しない。
- speed metric では dependency time を除外し続ける。
- `deferred_verify_requirements` が存在する場合の success/failure 判定が unit test で固定される。

### P4: verifier failure feedback を source repair prompt に寄せる

目的:

- `verify_repair_no_change` に至る前に編集へ収束させる。

方針:

- `format_verify_feedback()` に以下を追加する。
  - failing command
  - likely target files from command and stack/output
  - observed assertion mismatch excerpt
  - "次のターンは Read だけでなく、必要なら小さい Edit/Write を行う" という bounded instruction
- source の `minimal_step_runner/repair.rs` の文言に近づける。
- ただし自動でテストを書き換えるような operator は入れない。まずは feedback 強化に留める。
- repair target は source-of-truth を区別する。原則として、ユーザー要求・既存仕様・postcheck が実装挙動を要求している場合は implementation/setup を優先し、テスト期待値変更は明示的な test artifact 生成ミス、test framework mismatch、または要求と矛盾する生成テストが確認できる場合に限定する。

過度な eval 適応を避ける条件:

- `AssertionError: 2 != 3` 固有の変換はしない。
- unittest/pytest/node/cargo 共通の failure target extraction として扱う。
- feedback は「テストを書き換えろ」ではなく「失敗診断を根拠に、実装・テスト・セットアップのどれが authority を持つか確認して最小修正せよ」とする。

必要なテスト:

- unittest assertion failure feedback に command、target test file、implementation candidate が含まれる。
- 2 回連続 Read の場合は現行通り `verify_repair_no_change` で止まる。
- feedback 強化後も verifier failure を成功扱いしない。
- 明らかな test weakening、例: assertion を `True` にする、期待値だけを observed actual に合わせる、を促す文言が feedback に含まれない。

### P5: eval / runtime contract 整合性テストを追加する

目的:

- eval postcheck で必須の条件が runtime completion contract から消える不備を早期検出する。

方針:

- suite scenario から生成される completion contract を snapshot 化する。
- `expected_artifacts` と `postcheck` の関係を検証し、framework/task-class に対して required verify/profile check が空にならないことを確認する。
- 現在の `test_completion_contract_for_minimal_loop_splits_verify_from_setup` は修正し、「setup command は除外するが build requirement は metadata として残る」ことを期待にする。

必要なテスト:

- Next.js scenario contract に `profile=nextjs` または equivalent check が残る。
- Python unittest scenario contract に `python3 -m unittest ...` が残る。
- docs-only scenario は verify なしでも artifact completion を許す。

## 8. 優先順位

1. P0: Python unittest の dependency-order 誤判定を修正。
2. P5: eval / runtime contract 整合性テストを先に追加。
3. P2/P3: Next.js profile-aware completion と npm build requirement metadata を一体で追加。
4. P4: verifier failure feedback を強化。
5. P1: lint retry を累積 hard constraints 化。

理由:

- P0 は step-plan 3 件中 2 件へ直接効く。
- P2/P3 は minimal-loop Next.js 2 件へ直接効く。
- P4 は minimal-loop Python 1 件へ効くが、全 verifier repair に横展開できる。
- P1 は provider 出力の揺れを減らす横断対策。

## 9. 受け入れ条件の方向性

修正時は、単に今回の 6 failure を通すだけでは不十分。最低限、次を受け入れ条件にする。

- `step-plan,minimal-loop` の speed-cloud smoke 2 runs で、同一 failure kind が再発しない。
- `python3 -m unittest` の setup-order 誤判定が unit test で防止されている。
- `nextjs` profile minimal-loop が artifact existence だけで完了しないことを unit test で保証する。
- Next.js profile verify だけでは捕捉できない build-risk が `deferred_verify_requirements` または compatibility check として残る。
- docs-only / artifact-only task は過剰に verifier を要求されない。
- dependency install command は verify command として実行されない。
- `npm run build` requirement は speed metric から dependency time を除外しつつ、completion authority から消えない。
- failure classification は `postcheck_failure` / `verify_repair_no_change` / `planner_lint_error` の診断を維持する。
- `plan-run` / `ultra-plan-run` / TUI 通常入力への影響が fixture または integration test で確認されている。

### 9.1 完了条件レビュー結果

レビュー観点:

- P0-P5 の各対策が「実装した」だけでなく「どの失敗をどの gate で防ぐか」まで判定可能か。
- eval smoke 成功だけに依存していないか。
- provider / model の揺れを deterministic fixture と live check のどちらで扱うかが明確か。
- Next.js / Python / docs-only の negative regression を拾えるか。
- plan-run / ultra-plan-run / TUI への波及を確認できるか。

不足していた点:

1. P0-P5 ごとの done 条件が分散しており、実装完了の判定が曖昧だった。
2. `speed-cloud smoke 2 runs` は必要だが、unit / fixture / integration test の前提条件が不足していた。
3. Next.js の build-risk を `profile verify` と `deferred_verify_requirements` のどちらで成功/失敗判定へ使うかが曖昧だった。
4. P1 の retry prompt は LLM 挙動仮説なのに、fixture/live validation の成功条件が不足していた。
5. repair feedback 強化が test weakening を誘発しないことを確認する negative test が不足していた。
6. TUI 通常入力、plan-run、ultra-plan-run への波及確認が「影響あり」に留まり、完了条件へ入っていなかった。
7. `step-plan` は lint pass だけでは不十分で、plan quality score、責務分界、verify coverage の退行検知が不足していた。
8. `ultra-step-run` は影響範囲には入っていたが、横断完了条件と smoke に入っていなかった。
9. 移植元 `anvildev` との dry-run / targeted comparison が完了条件に入っておらず、MVP eval harness 側だけの退行を見落とす余地があった。
10. eval harness 自体の `--scenario` 単一指定、report schema、redaction、contract shape summary のテストが不足していた。
11. 実装修正後の静的品質 gate として `fmt` / `clippy` が明示されていなかった。

反映方針:

- 完了条件を P0-P5 と横断 gate に分ける。
- テスト計画を unit / eval script / runtime integration / provider fixture / live API / eval trend の層に分ける。
- eval 成功率だけで完了にしない。unit と fixture で不変条件を先に固定する。
- plan品質、source比較、eval harness 自体の正しさを横断 gate に追加する。
- `ultra-step-run` と TUI slash command を、少なくとも dry-run / targeted smoke のどちらかで確認する。
- secret/home path redaction と report schema を observability の完了条件に入れる。

### 9.2 P0-P5 完了条件

| 対策 | 完了条件 |
|---|---|
| P0 lint policy | `python3 -m unittest ...` が dependency-order lint で落ちない。npm/pnpm/yarn build/test は setup/order 不備を引き続き検出する。pytest/cargo は plan lint で一律 setup 必須にせず、verify/precondition 側で診断される。 |
| P1 retry prompt | 累積 lint categories が retry prompt へ反映される。shell control / duplicate ownership / dependency-order の複合 fixture が 3 attempt 以内に valid plan へ収束する。fixture で不安定な場合は live API で検証し、効果が薄い場合は deterministic normalizer / plan repair に切り替える判断を記録する。 |
| P2 profile-aware completion | `--profile nextjs` かつ Next.js project shape の場合、required paths が揃っただけでは completion しない。static profile verify で検出可能な package/script/entrypoint/tsconfig/css/toolchain 不備は feedback になる。generic/docs-only は artifact-only completion を維持する。 |
| P3 deferred build requirement | `npm install` を verify command に混ぜず、`npm run build` requirement は `verify_commands: []` によって消えない。runtime が実行しない build requirement は `deferred_verify_requirements` または equivalent metadata として残り、success 判定で無視されない。`pending` / `blocked_by_dependency_setup` のまま artifact-only success してはいけない。static profile check が代替 evidence になる場合は、どの requirement を cover したか event/report に残る。 |
| P4 repair feedback | verifier failure feedback が failing command、target候補、failure excerpt を含む。feedback は implementation/setup/test の authority を確認させ、assertion を弱める文言を含まない。2 回連続 no-edit は従来通り `verify_repair_no_change` で停止する。 |
| P5 contract consistency | suite scenario から生成される completion contract snapshot が、expected artifacts / postcheck / profile requirement の対応を保持する。既存の「npm build を外す」期待は「setup command は除外するが build requirement は metadata に残る」期待へ更新される。step-plan の plan quality score と warning も snapshot/report で追跡される。 |

### 9.3 横断完了条件

- `cargo fmt --manifest-path mvp/anvilminimal/Cargo.toml -- --check` が通る。
- `cargo clippy --manifest-path mvp/anvilminimal/Cargo.toml --all-targets -- -D warnings` が通る。
- `cargo test --manifest-path mvp/anvilminimal/Cargo.toml` が通る。
- `python3 -m unittest discover -s mvp/anvilminimal/tests/eval` が通る。
- `mvp-smoke` の `step-plan,minimal-loop` を speed-cloud / local LLM 未使用で 2 回実行し、今回の failure kinds が同一原因で再発しない。
- `plan-run`, `ultra-plan-run`, `ultra-step-run` について、少なくとも targeted smoke または dry-run / fixture integration で、step-plan lint と minimal-loop completion 変更の波及が確認されている。
- TUI 通常入力について、profile 未指定の generic task が従来通り completion でき、`nextjs` profile 指定時だけ profile-aware gate が効くことを確認する。
- failure classification が粗くならない。少なくとも `planner_lint_error`, `verify_command_policy_error`, `postcheck_failure`, `verify_repair_no_change`, `dependency_missing` 相当の分類が維持される。
- successful step-plan には `plan_quality_score` が記録され、deterministic fixture は 70 点以上、live eval は同一 scenario の直近 baseline から 5 点超低下しない。低下した場合は成功率が改善していても要調査にする。
- `anvilminimal` と `anvildev` の eval command rendering が dry-run で通り、source binary 経路を壊していないことを確認する。可能なら targeted smoke で代表 scenario を比較する。
- eval harness の `--scenario` は単一 ID 指定としてテストされ、複数 ID 一括指定を前提にした手順を残さない。
- eval report / event は contract shape、profile/deferred status、verify feedback excerpt を出すが、secret、API key、absolute home path を redaction する。
- `.env` に `OPENAI_API_KEY` / `GEMINI_API_KEY` がある環境では、P1 の prompt 変更について OpenAI/Gemini の小さい live planner check を行う。key がない環境では provider fixture を必須にし、live check 未実施を report に明記する。
- 実装後の eval 改善が scenario id 固有分岐、provider 固有分岐、test expectation weakening によるものではないことをコードレビューで確認する。

### 9.4 テスト計画

| レイヤ | テスト対象 | 追加/更新する検証 |
|---|---|---|
| Static | `mvp/anvilminimal` crate | `cargo fmt --manifest-path ... -- --check` と `cargo clippy --manifest-path ... --all-targets -- -D warnings` |
| Rust unit | `planner/lint.rs` | Python unittest は setup-order pass、npm build/test は setup/order fail、pytest/cargo は setup-order lint 対象外、shell control syntax は引き続き fail |
| Rust unit | `planner/runner.rs` | 複数 lint category が retry prompt に累積される。provider 固有文言を入れない |
| Rust unit | `minimal_loop/completion.rs` | completion contract が profile/deferred requirement を validate できる。dependency setup command は verify に混入しない。pending deferred requirement は success を block する |
| Rust unit | `planner/profiles/nextjs.rs` | package/script/entrypoint/css/tsconfig/version compatibility/build-risk を static に検出する。検出不能な build requirement は deferred として残る |
| Rust unit | `minimal_loop/feedback.rs` | verifier failure feedback に command/target/failure excerpt が含まれ、test weakening を促さない |
| Rust integration | minimal-loop runtime | nextjs required paths only では early success しない。generic docs-only は early success できる。verify failure 後 no-edit は停止する |
| Python unit | `scripts/eval-run.py` | completion contract snapshot。`npm install` + `npm run build` で build requirement が消えない。Python unittest は verify command に残る |
| Python unit | `scripts/eval-run.py` matrix | `--scenario` 単一 ID 指定、`ultra-step-run` placeholder、`--binary-kind anvildev` rendering を dry-run で固定する |
| Python unit | `eval_lib/plan_scoring.py` | lint pass だが低品質な plan を低 score として検出する。source-derived good fixture は 70 点以上 |
| Python unit | eval classification/report | 新しい deferred/profile failure が summary/report に出る。既存 failure kinds が退化しない |
| Python unit | eval event/report redaction | contract summary / feedback excerpt / command summary から secret、API key、home path が漏れない |
| Provider fixture | planner request/response fixtures | OpenAI/Gemini/Ollama の planner output shape と retry prompt に対し、schema/lint/repair path を固定する |
| Live API optional | OpenAI/Gemini planner | P1 prompt diff 後に docs/python/repair-exhausted 相当の小さい planner check を行う。失敗した場合は deterministic normalizer 案へ戻す |
| Eval smoke | `mvp-smoke` | `step-plan,minimal-loop` 2 runs。必要に応じて `plan-run,ultra-plan-run,ultra-step-run` targeted smoke |
| Source comparison | `anvildev` | dry-run は必須。可能なら代表 scenario の targeted smoke で MVP 固有の過剰制約や緩和を比較する |
| TUI smoke | `anvilminimal` interactive | 通常入力 path で generic completion が壊れていないこと、slash `/ultra-plan-run --profile nextjs ...` で profile propagation が落ちないことを確認する |

### 9.5 未完了判定

次のいずれかが残る場合は完了扱いにしない。

- `python3 -m unittest ...` が dependency-order lint で落ちる。
- Next.js profile task が required paths の存在だけで `required_artifacts_satisfied_after_tool` になる。
- `npm run build` requirement が completion contract から完全に消える。
- docs-only / artifact-only generic task が不要な build/profile verify を要求される。
- retry prompt 変更が fixture で再現できず、live check でも改善が確認できない。
- feedback 強化により test assertion の弱体化を促す文言が入る。
- step-plan が lint pass しても plan quality score が空、または deterministic baseline を下回る。
- `ultra-step-run` の dry-run または targeted smoke が未実施。
- `anvildev` 評価経路の dry-run が未実施。
- eval report / event redaction の negative test が不足している。
- `fmt` / `clippy` / unit / Python tests のいずれかが未実施または失敗している。
- eval smoke は通るが unit/fixture の negative tests が不足している。
- plan-run / ultra-plan-run / TUI の影響確認が未実施。

## 10. 過度な eval 適応を避けるための禁止事項

- scenario id 名で分岐しない。
- `Space Invaders` / `markdown_lint.py` / `test_markdown_lint.py` 固有の補正を入れない。
- model provider 別の special case を増やさない。
- postcheck を runtime 内に丸ごと埋め込まない。
- `npm run build` を強制実行して速度評価や network 条件を不安定化させない。
- verifier failure を自動的にテスト期待値変更へ誘導しない。
- lint を緩めて危険な shell control syntax や setup-in-verify を許可しない。

対策は `task class`、`profile`、`verifier command class`、`workspace state` を軸に行う。

## 11. 次の作業案

別ファイルで作業計画化する場合は、次の phase に分ける。

- Phase 0: failing fixtures / completion contract snapshots を固定。
- Phase 1: Python verifier class と dependency-order lint を source parity 寄りに修正。
- Phase 2: lint retry prompt を累積 hard constraints 化。
- Phase 3: completion contract に profile/deferred verify requirement を追加。
- Phase 4: Next.js profile verify を minimal-loop completion gate に接続。
- Phase 5: verifier failure feedback を source repair prompt に寄せる。
- Phase 6: unit / eval script / Rust tests を追加。
- Phase 7: speed-cloud smoke を 2 回以上実行し、minimal-loop / step-plan の trend を確認。
- Phase 7 の横断確認: `plan-run` / `ultra-plan-run` / `ultra-step-run` / TUI / `anvildev` dry-run を実施する。
