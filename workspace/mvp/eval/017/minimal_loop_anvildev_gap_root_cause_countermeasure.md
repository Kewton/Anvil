# minimal-loop 単体が anvildev に劣る問題の深掘り

## 0. 対象と前提

対象は直近の eval 結果における以下の論点である。

- 「3. minimal-loop 単体でも anvildev に劣る」
- MVP: `/private/tmp/anvilminimal-eval-015-2-mvp-live`
- anvildev: `/private/tmp/anvilminimal-eval-015-2-anvildev-live`
- 対象 mode: `minimal-loop`
- 条件: スピード重視、ローカル LLM 未使用

本調査は、評価値だけではなく、生成された成果物、postcheck ログ、MVP 実装、移植元実装を照合した。

本レビューでの補正方針:

- 事実と推論を分ける。
- eval 指標を runtime ロジックへ逆流させない。
- MVP の設計思想である「小さい loop + deterministic verify」を保つ。
- `TaskContract` や通常 loop 全体を丸ごと移植せず、必要な completion authority の考え方だけを小さい contract として再設計する。
- package/version など変動し得る外部情報は「最新」へ追随するのではなく、deterministic に検証済みの known-good profile set として扱う。
- 受け入れ条件は単発 run の絶対値だけで判定せず、provider error を除外した 3-run trend、false positive、失敗分類、anvildev 比較を組み合わせて判定する。
- runtime は eval の score / acceptance 判定を見ない。runtime に渡すのは required capability / evidence / verify policy のような実行契約だけに限定する。

## 1. 事象

minimal-loop 単体の acceptance 成功率で MVP が anvildev に劣っている。

| binary | legacy success | acceptance success | false positive | artifact success |
| --- | ---: | ---: | ---: | ---: |
| MVP | 10/12 | 7/12 | 3 | 12/12 |
| anvildev | 12/12 | 11/12 | 1 | 12/12 |

重要なのは、MVP も `artifact_success` は 12/12 である点である。つまり、主問題は「ファイルを作れない」ことではない。作った成果物が、要求された意味的機能、決定的テスト、ビルド可能性、実行可能性まで満たす前に完了扱いになっている。

MVP の minimal-loop 失敗内訳は以下。

| failure kind | 件数 | 内容 |
| --- | ---: | --- |
| `missing_required_capabilities` | 3 | 実装ファイルはあるが、要求された deterministic test/check がない |
| `postcheck_failure` | 2 | Next.js の install/build/dev server readiness のどこかで失敗 |

anvildev の失敗は `missing_required_capabilities` 1 件のみで、Next.js 2 件は build と dev server readiness まで成功している。

## 2. 実例

### 2.1 MVP: date helper は実装のみで smoke check がない

対象:

- `/private/tmp/anvilminimal-eval-015-2-mvp-live/runs/mvp-acceptance__fix-js-date-helper-small__minimal-loop__openai-gpt-5.4-mini__gemini-gemini-3.5-flash__r1/workdir/date-helper.js`
- `/private/tmp/anvilminimal-eval-015-2-mvp-live/runs/mvp-acceptance__fix-js-date-helper-small__minimal-loop__gemini-gemini-3.1-flash-lite__openai-gpt-5.4-mini__r1/workdir/date-helper.js`

観察:

- `formatDate`, `addDays`, `normalizeDate` はある。
- しかし、prompt / `functional_contract` が要求した deterministic node smoke check は存在しない。
- `node date-helper.js` のような弱い postcheck では、exports だけのファイルでも rc=0 になり得る。

直接的な評価結果:

- `source_semantic_score = 50.0`
- missing capability: `deterministic_test`
- `legacy_success = true`
- `acceptance_success = false`
- `acceptance_false_positive = true`

### 2.2 MVP: Rust CLI は `Hello, world!` で完了

対象:

- `/private/tmp/anvilminimal-eval-015-2-mvp-live/runs/mvp-acceptance__rust-cli-medium__minimal-loop__gemini-gemini-3.1-flash-lite__openai-gpt-5.4-mini__r1/workdir/src/main.rs`

生成物:

```rust
fn main() {
    println!("Hello, world!");
}
```

観察:

- `Cargo.toml` と `src/main.rs` は存在する。
- ただし、CLI としての要求や deterministic check/unit test は満たしていない。
- `cargo test` はテストが 0 件でも通り得るため、postcheck だけでは不十分。

直接的な評価結果:

- `source_semantic_score = 50.0`
- missing capability: `deterministic_check`
- `legacy_success = true`
- `acceptance_success = false`
- `acceptance_false_positive = true`

anvildev の同ケースでは `greet` 関数と `#[test]` が生成されており、少なくとも deterministic check の形は満たしている。

### 2.3 MVP: Next.js は機能実装があっても build repair に届かない

対象:

- `/private/tmp/anvilminimal-eval-015-2-mvp-live/runs/mvp-acceptance__nextjs-space-invaders-large__minimal-loop__openai-gpt-5.4-mini__gemini-gemini-3.5-flash__r1`

観察:

- Canvas / enemies / bullets / score など、ゲームらしい実装は一部存在する。
- しかし `src/app/page.tsx` 内で `const s` が二重定義され、`npm run build` が失敗。
- loop は `completion_contract_satisfied` で停止し、build failure を修復していない。

代表エラー:

```text
the name `s` is defined multiple times
Build failed because of webpack errors
```

対象:

- `/private/tmp/anvilminimal-eval-015-2-mvp-live/runs/mvp-acceptance__nextjs-space-invaders-large__minimal-loop__gemini-gemini-3.1-flash-lite__openai-gpt-5.4-mini__r1`

観察:

- `npm install --ignore-scripts` が `typescript@5.0.0` の解決失敗で落ちている。
- dependency/version coherence を runtime が十分に拘束できていない。

代表エラー:

```text
npm error notarget No matching version found for typescript@5.0.0.
```

anvildev の同 Next.js ケースは 2 件とも以下を満たした。

- expected artifacts あり
- `npm install --ignore-scripts` rc=0
- `npm run build` rc=0
- dev server port 3011 readiness true / HTTP 200

## 3. 問題点

### P1. 完了判定が artifact existence に寄りすぎている

MVP minimal-loop は required paths を満たすと早期に完了しやすい。結果として「ファイルはあるが機能・テスト・ビルドが足りない」状態を成功扱いしやすい。

評価上は `artifact_success = 12/12` だが、`acceptance_success = 7/12` であり、artifact existence と実用上の成功に乖離がある。

### P2. acceptance contract が runtime completion contract に渡っていない

eval の `functional_contract` には `implementation`, `deterministic_test`, `deterministic_check`, `browser_interaction` などの要求がある。しかし、MVP の minimal-loop completion contract は主に以下で構成されている。

- `expected_artifacts`
- deterministic と判定できた `postcheck.commands`
- dependency setup が必要な deferred verify
- profile

required capabilities は completion contract の停止条件に入っていない。

### P3. postcheck が弱いケースで false positive を止められない

`node date-helper.js` や `cargo test` のような postcheck は、テストが存在しない場合にも rc=0 になることがある。現状は「postcheck が存在する」ことと「要求機能を検証している」ことを runtime 側が十分に区別できていない。

### P4. Next.js の deferred build を静的 profile check だけで代替しすぎている

MVP には Next.js profile verification があるが、実際の `npm install` / `npm run build` / dev server readiness と完全一致するものではない。静的 profile が通っても、以下は残る。

- TypeScript/React/Next の実 dependency resolution
- TSX compile error
- React server/client boundary error
- dev server runtime error

### P5. 移植元の周辺安全装置が minimal-loop MVP に入っていない

移植元には、単体の `src/agent/minimal_loop` だけでなく、通常 loop 側に以下の大きな安全装置がある。

- `TaskContract` による required artifacts / verification required / required behavior の推定
- post-loop success verifier
- missing evidence recovery
- deterministic scaffold は「bootstrap only, not completion」と明示する継続 note
- Python test fallback
- Node test runner manifest fallback
- Next.js / playable UI polish fallback
- verifier repair / evidence extraction / case record

MVP では、これらを直接持たず、薄い completion contract と profile verify に寄せている。

## 4. 問題箇所

### 4.1 eval から MVP minimal-loop に渡す contract

`mvp/anvilminimal/scripts/eval-run.py`

- `completion_contract_for_spec`
- `required_paths = scenario.expected_artifacts`
- `verify_commands = deterministic postcheck commands`
- `deferred_verify_requirements = dependency setup が必要な postcheck`

問題:

- `functional_contract.required_capabilities` を completion contract に渡していない。
- `oracle_contract.deterministic_oracles` を runtime の停止条件に渡していない。
- `postcheck` が弱い場合の「追加で self-test を要求する」契約がない。

### 4.2 MVP completion contract

`mvp/anvilminimal/src/minimal_loop/completion.rs`

現在の主要フィールド:

- `required_paths`
- `verify_commands`
- `profile`
- `goal`
- `deferred_verify_requirements`
- `verify_repair_cap`

問題:

- required capabilities を表現するフィールドがない。
- deterministic test/check の存在要件を表現できない。
- `postcheck` と `source_semantic` の両方が必要なケースを runtime で扱えない。
- build/dev-server の外部 postcheck failure を loop 内 repair feedback へ戻す経路がない。

### 4.3 MVP minimal-loop の停止箇所

`mvp/anvilminimal/src/minimal_loop/loop_run.rs`

該当:

- assistant final 受信時に `verify_completion_contract` が `Ok(None)` なら `completion_contract_satisfied`
- tool 実行後に required paths が満たされ、contract verify が通れば `completion_contract_satisfied`
- verify なし、または profile 静的 check が通った場合、semantic acceptance なしで終了可能

問題:

- stop reason は正しく記録されるが、停止の根拠が acceptance contract と一致していない。
- `completion_contract_satisfied` が「必要ファイルと限定的 verify が通った」以上の意味を持ってしまっている。

### 4.4 MVP Next.js profile verification

`mvp/anvilminimal/src/planner/profiles/nextjs.rs`

現在見ている主な観点:

- package.json 存在
- next/react/react-dom dependencies
- scripts.build = `next build`
- port 3011 dev script
- entrypoint/layout
- tsconfig alias / moduleResolution
- Tailwind toolchain consistency

問題:

- 実 build を通していない場合の compile/runtime failure は捕捉できない。
- dependency version の実解決可能性までは保証しない。
- Canvas/gameplay/input などのドメイン機能の有無は profile check の責務外。

## 5. 移植元 anvil との比較

### 5.1 数値比較

| 観点 | MVP | anvildev | 差分 |
| --- | ---: | ---: | --- |
| minimal-loop legacy success | 10/12 | 12/12 | anvildev +2 |
| minimal-loop acceptance success | 7/12 | 11/12 | anvildev +4 |
| false positive | 3 | 1 | MVP +2 |
| Next.js build success | 0/2 | 2/2 | anvildev +2 |
| artifact success | 12/12 | 12/12 | 差なし |

### 5.2 コード比較

移植元の `src/agent/minimal_loop` は、それ自体は completion contract を持たない素朴な loop である。ただし次の差分がある。

- `src/agent/minimal_loop/prompt.rs`
  - 「作成・編集・検証すると言ったなら同じ応答で tool call する」
  - 「観測していないファイル・テスト・コマンド結果を捏造しない」
  - final answer は完了済み作業だけを述べる
- `src/agent/minimal_loop/feedback.rs`
  - completion without write
  - requested artifact missing
  - missing relative imports
  - planned action without tool
- `src/agent/minimal_step_runner.rs`
  - step prompt に overall goal, required artifacts, expected paths, verify, expected result を入れる
  - verify failure 後に repair prompt を構築する
- `src/agent/minimal_step_runner/profile.rs`
  - ultra phase prompt に profile runtime contract を入れる
- `src/agent/minimal_step_runner/profiles/nextjs.rs`
  - Next.js create/fix 向けに build verification phase、dependency setup、Tailwind consistency、fake success 禁止を明示
- `src/agent/loop_run/task_contract.rs`
  - TaskContract で required artifacts, verification required, required behavior, completion policy を構築
- `src/agent/loop_run/scaffold_pipeline.rs`
  - deterministic scaffold を bootstrap only と明示し、完了扱いしない
  - Python test fallback / Node test runner manifest fallback / playable UI repair を持つ
- `src/agent/loop_run/actor_loop_flow.rs`
  - post-loop success verifier と case record extraction に verify commands を渡す

### 5.3 重要な比較結果

確認済みの事実:

- anvildev は eval harness から MVP 用 completion contract を注入されていない。
- それでも minimal-loop で 11/12 acceptance を達成している。
- anvildev の Next.js 2 件は `npm install --ignore-scripts`, `npm run build`, dev server readiness が成功している。
- anvildev の Rust CLI Gemini ケースでは `#[test]` が生成され、MVP の同ケースより deterministic check の evidence が強い。

推論:

- anvildev 側の prompt/feedback/周辺機構が、モデルに「テスト・検証・実 build までやる」行動を引き出している可能性が高い。
- ただし、この推論は「anvildev minimal-loop 単体のコード差分」だけでは説明しきれない。通常 loop 側の TaskContract / verifier / deterministic fallback の思想が、移植元全体の設計文化として minimal 系にも波及していると見るのが妥当である。

一方、MVP は completion contract を導入したことで artifact 作成の安定性は上がったが、その contract が artifact/profile 中心であるため、浅い完了を固定化している。

## 6. 原因

### 直接原因 C1

MVP の completion contract が `functional_contract.required_capabilities` を持たないため、`deterministic_test` や `deterministic_check` が欠落しても loop 内で検出できない。

### 直接原因 C2

`postcheck` が弱いケースを runtime が補強しないため、0 test / no self-test / exports only の成果物を成功扱いし得る。

### 直接原因 C3

Next.js の deferred verify が静的 profile coverage で満了扱いになる経路があり、実 install/build/dev server の失敗を loop 内修復に戻せない。

### 直接原因 C4

MVP の minimal-loop は `completion_contract_satisfied` を runtime success として返すが、その success は acceptance success と同義ではない。

## 7. 根本原因

### R1. MVP 切り出し時の境界設定が artifact/runtime shape に寄りすぎた

MVP の目的は「小さい loop + YAML plan + deterministic verify」の切り出しだった。しかし、移植元で実品質を支えていた安全装置は、単体の minimal loop ファイルだけではなく、通常 loop の TaskContract / verifier / fallback / evidence system に分散していた。

そのため、移植時に `src/agent/minimal_loop` と `src/agent/minimal_step_runner` の見える部分を中心に拾っただけでは、「完了の意味」を支える周辺機構が抜ける。

### R2. eval の成功基準と runtime の停止基準が別々に進化した

eval は 015-1 / 015-2 で false positive を抑える方向に改善され、`acceptance_success` は実成果物の意味的充足を見るようになった。一方で runtime の completion contract は `expected_artifacts` と postcheck/profile 中心のままだった。

その結果、評価は正しく厳しくなったが、runtime はその厳しさを停止条件として受け取っていない。

### R3. `postcheck` を外部評価専用と見なし、loop 内 feedback source として十分に扱っていない

現在の eval では postcheck はプロセス終了後に実行される。そのため、postcheck failure は acceptance では検出できるが、agent は修復機会を得ない。

これは Next.js の build failure と dependency failure に直結している。

### R4. 「移植漏れ」の定義が狭かった

過去の SG 系棚卸しでは、明示的な minimal-loop safeguard を中心に確認した。しかし今回の差分は、以下のような横断機構に由来する。

- TaskContract 由来の required behavior
- verifier/evidence 由来の completion authority
- deterministic fallback 由来の test/scaffold 補完
- profile runtime contract 由来の build/dependency integrity

つまり、`minimal_loop` ディレクトリ単位ではなく、「成功判定に関与する全経路」を移植対象として棚卸しする必要があった。

## 8. 移植漏れか

結論として、広義には移植不備である。ただし、単一の source ファイルを丸ごと移し忘れたというより、移植境界の取り方を誤った。

移植漏れの性質:

- `src/agent/minimal_loop` 単体の差分では説明できない。
- anvildev の実品質は `src/agent/loop_run` 配下の TaskContract / verifier / deterministic fallback の思想にも支えられている。
- MVP は artifact recovery と profile check を追加したが、required behavior / semantic capability / evidence authority までは runtime contract に移していない。

漏れた原因:

1. MVP API を新規に切り直す方針の中で、既存の大きな TaskContract system を複雑すぎるものとして対象外にした。
2. その判断自体は MVP の複雑性を抑える意味では妥当だったが、代替となる小さい semantic completion contract を設計していなかった。
3. eval で false positive を検出するまでは、artifact success と legacy success が高く見え、欠落が表面化しにくかった。
4. 移植元比較を「同じファイル構造の比較」に寄せすぎ、実際の成功に寄与する cross-cutting mechanism の比較が不足していた。

## 9. 横展開すべき観点

### H1. minimal-loop

required capabilities を completion contract に渡し、以下を停止条件に含める。

- implementation
- deterministic_test
- deterministic_check
- buildable
- browser_interaction / playable UI などの profile-specific capability

### H2. plan-run

plan-run は各 step を minimal-loop に乗せるため、同じ浅い完了判定が波及する。step prompt に capability contract を渡し、step verify が弱い場合は plan lint または runtime repair へ戻す。

### H3. ultra-plan-run

ultra phase は `/plan-run` の連鎖なので、phase 完了時に profile postcheck / final acceptance が弱いと「フェーズは完了したがアプリは未完成」になる。phase finalization で final required capabilities を確認する必要がある。

### H4. eval

minimal-loop に限らず、legacy success は主指標にしない。主指標は acceptance success とし、false positive を mode 別に継続監視する。

### H5. provider 差分

OpenAI と Gemini で失敗傾向が異なる。

- OpenAI: 実装量は多いが compile error を残しやすい
- Gemini: dependency/version coherence や最小実装化の失敗が出やすい

provider 別に runtime feedback が効いているかを eval events で確認する。

## 10. 対策案

### A1. CompletionContract v2 を導入する

MVP の `CompletionContract` に runtime 用の evidence contract を追加する。

```json
{
  "required_capabilities": ["implementation", "deterministic_test"],
  "deterministic_oracles": ["source_semantic", "postcheck"],
  "required_evidence": ["test_artifact", "bound_verify_command"]
}
```

設計方針:

- eval 専用の文字列マッチにしない。
- generic capability 名で表現する。
- runtime では `source_semantic_score` や `acceptance_success` のような eval 指標を参照しない。
- runtime では完全な semantic oracle を再実装せず、まずは「必要な検証成果物/テスト/チェックが存在するか」を deterministic に見る。
- eval は runtime events と成果物から acceptance score を算出する。runtime は score ではなく、完了に必要な evidence が不足しているかだけを判断する。

### A2. functional_contract から completion contract へ capability を投影する

`mvp/anvilminimal/scripts/eval-run.py` の `completion_contract_for_spec` で、以下を contract に含める。

- `functional_contract.required_capabilities`
- `oracle_contract.deterministic_oracles`
- runtime に渡せる `required_evidence`
- `postcheck.commands`

ただし、runtime に渡すのは汎用化済み capability のみとし、scenario 固有語に依存しない。

### A3. deterministic test/check capability verifier を追加する

runtime 側に軽量 verifier を追加する。

例:

- `deterministic_test`
  - `test_*.py`, `*.test.js`, `*.spec.js`, `#[test]`, `node:test`, `assert` などを検出
  - implementation file 内の `if (require.main === module)` + `assert` も許容
- `deterministic_check`
  - `cargo test` が 0 tests でないこと
  - `npm test` が script と実 test artifact に接続していること
  - `node <file>` が no-op でないこと

重要:

- 評価ケース固有のファイル名を hardcode しない。
- source semantic oracle と同じ完全判定を runtime に持ち込まず、完了前の最低限ガードに留める。

### A4. weak postcheck を検出して補強 feedback を出す

以下を weak verification として扱う。

- `cargo test` で 0 tests
- `node file.js` で assertion / smoke output / non-trivial main がない
- `npm test` script が存在しない、または no-op
- `cat`, `test -f` のみで functional capability を検証していない

weak の場合は final に進ませず、以下の feedback を返す。

- requested capability
- 現在不足している evidence
- 追加すべき最小テスト/チェック

### A5. postcheck failure を bounded repair に戻す

eval 実行時だけでなく通常実行でも使える形で、completion contract の `verify_commands` に安全な build/test を含めるか、deferred postcheck を満了扱いにする条件を厳しくする。

非目標:

- dev server の長時間起動を runtime verify に入れない。
- network install を無条件に runtime verify として実行しない。
- eval の postcheck runner をそのまま agent loop に埋め込まない。

特に Next.js:

- `npm run build` が postcheck にある場合、静的 profile だけで完全満了にしない。
- dependency setup が必要なら、setup 許可/禁止を明示して repair feedback へ渡す。
- 実 build が実行できない場合は `dependency_missing` として完了ではなく明示停止する。

### A6. Next.js dependency/version coherence check を強化する

Next.js profile check に以下を追加する。

- deterministic profile が採用する known-good dependency set を定義する。
- Next major と React major の coherence を検証する。
- `@types/react` と React major の不一致を warning/failure にする
- package manager lockfile が存在する場合、package.json と矛盾しないこと

注意:

- ここでは「最新バージョン」を推測して追いかけない。外部 package の最新状況は変動するため、runtime profile は eval / CI で検証済みの known-good set を使う。
- 実装時に最新 compatibility を採用する必要がある場合は、公式 documentation / registry で別途検証し、profile fixture を更新する。

### A7. source/anvil の周辺思想を MVP に小さく再設計して取り込む

そのまま TaskContract 全体を移植しない。代わりに小さい `RuntimeAcceptanceContract` を作る。

責務:

- prompt / eval scenario / plan から required capabilities を受ける
- required artifacts と capabilities を統合する
- weak verify を検出する
- finalization を cap する
- repair feedback を生成する

これにより、MVP の小ささを保ちながら、移植元の「完了には evidence が必要」という設計思想を回収する。

### A8. 不確実性を検証してから採用する

現時点で不確実な対策:

- Next.js dependency major coherence の具体ルール
- `deterministic_test` / `deterministic_check` の言語別 evidence 判定の境界
- postcheck を runtime verify に戻す範囲

検証方法:

- まず fake provider / fixture で deterministic に検証する。
- package compatibility は最新推測ではなく、known-good fixture を install/build して確認する。
- LLM API を使う場合は「モデルがこの feedback で修復行動に移るか」の仮説検証に限定する。仕様・互換性の根拠にはしない。
- LLM API 検証は network/API cost を伴うため、実装前の必須条件にはせず、fake provider で再現できない prompt 感度の検証時だけ実施する。

## 10.1 レビュー観点別の見直し結果

### 設計思想に則っているか

方向性は概ね妥当。ただし初版の `minimum_acceptance.source_semantic_score` を runtime contract に入れる案は不適切だった。eval score を runtime が参照すると、評価指標へ過適応し、通常利用時の意味が曖昧になる。

修正後は、runtime は `required_capabilities` と `required_evidence` の不足だけを見る。score 化は eval 側に閉じる。

### 不安定な挙動をもたらさないか

初版の「postcheck failure を loop 内に戻す」は、そのままだと network install / dev server 起動 / 長時間 process を runtime に混入させるリスクがあった。

修正後は、安全な build/test verify と dependency_missing の明示停止に限定する。dev server readiness は eval oracle 側に残す。

### 影響調査は十分か

初版は minimal-loop 中心だったため、plan-run / ultra-plan-run への波及は書いていたものの、通常 TUI 実行への影響が弱かった。

補足すべき影響:

- TUI で `/ultra-plan-run` を実行した場合、phase / step 経由で同じ completion contract が使われる。
- `anvilminimal --yes ...` の通常 minimal-loop でも、required evidence が増えると完了までの turn 数が増える可能性がある。
- verify command を増やすと実行時間が伸びるため、dependency setup と dev server は runtime から分離する必要がある。

### 他に影響を与えないか

主な影響先:

- `mvp/anvilminimal/src/minimal_loop/completion.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/planner/runner.rs`
- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `mvp/anvilminimal/scripts/eval-run.py`
- `mvp/anvilminimal/scripts/eval_lib/*`

互換性確保:

- CompletionContract v1 は後方互換で読み込む。
- required capabilities が空なら現行挙動を維持する。
- plan-run step では step-local expected paths を優先し、final capabilities は最終 step / phase completion で確認する。

### ソフトウェア複雑性が増大しないか

複雑性リスクはある。TaskContract 全体の移植は避けるべきである。

許容する追加:

- `RuntimeAcceptanceContract` の小さい構造体
- capability/evidence verifier の小さい pure function 群
- eval-run からの contract projection

避ける追加:

- 通常 loop の TaskContract 全体移植
- browser oracle の runtime 埋め込み
- provider 別 special case
- scenario 名に依存した条件分岐

### 原因の深掘りは十分か

初版では「移植境界が狭かった」と整理したが、なぜ見落としたかの分解が不足していた。追加分析は以下。

- artifact success が 12/12 だったため、従来の完了条件では問題が隠れた。
- anvildev の品質を `src/agent/minimal_loop` の差分だけで説明しようとしたため、通常 loop 側の completion authority を過小評価した。
- eval の acceptance oracle 強化が後追いだったため、runtime 停止条件との乖離が後から顕在化した。
- SG 棚卸しが「安全装置の存在確認」に寄り、「成功判定に寄与する cross-cutting mechanism」の依存関係確認になっていなかった。

### 他に移植漏れがないか

追加で確認すべき候補:

- requested artifact extraction と explicit artifact obligation
- required behavior extraction
- test evidence binding
- verifier command collection
- missing evidence recovery
- deterministic scaffold continuation note
- Node test runner manifest fallback
- Python test fallback
- playable UI repair / polish fallback
- post-loop success verifier

これらをすべて移植するのではなく、MVP の completion contract に必要な input/output だけを抽出する。

### 移植不備が無いか

現時点で明確な移植不備:

- required behavior / capability が runtime 完了条件に入っていない。
- deterministic test/check の evidence gate がない。
- profile static check と実 build/postcheck の境界が曖昧。
- scaffold / shell / placeholder を「完了ではない」と扱う completion authority が弱い。

### 不確実な対策方針はないか

不確実な点はあるが、現段階で LLM API を実行しないと判断する。

理由:

- 今回の主問題は LLM 応答品質の仮説ではなく、runtime completion contract の欠落として、既存ログと成果物で再現できている。
- 対策の第一段階は fake provider / fixture で検証可能である。
- LLM API は prompt feedback の効き方を確認する段階で使うのが妥当であり、設計判断の根拠として使うべきではない。

## 11. 実装フェーズ案

### Phase 0: baseline fixture 固定

目的:

- 今回の失敗を regression fixture として固定する。

作業:

- MVP 失敗 5 ケースの summary / output / events を fixture 化
- anvildev 成功例 4 ケースを比較 fixture 化
- `minimal_loop_acceptance_gap_fixture` を追加

受け入れ条件:

- fixture から failure kind と missing capability を再現できる。

### Phase 1: CompletionContract v2 schema

目的:

- required capabilities を runtime に渡せるようにする。

作業:

- `CompletionContract` に `required_capabilities`, `deterministic_oracles`, `required_evidence` を追加
- JSON validate / dedupe / unknown capability handling
- eval-run から functional_contract を投影

受け入れ条件:

- 既存 contract JSON は後方互換で通る。
- new contract JSON が validate される。

### Phase 2: capability verifier

目的:

- deterministic test/check 不足を loop 内で止める。

作業:

- `deterministic_test` verifier
- `deterministic_check` verifier
- zero-test / no-op smoke の検出
- feedback 文言追加

受け入れ条件:

- `date-helper.js` exports only は completion しない。
- `date-helper.js` + assert/self-test は completion する。
- Rust `Hello, world!` + 0 tests は completion しない。
- `#[test]` ありは completion する。

### Phase 3: weak postcheck guard

目的:

- postcheck が意味的要求を検証していない場合の false positive を防ぐ。

作業:

- weak command classifier
- required capability と verify command の接続判定
- feedback 生成

受け入れ条件:

- `node date-helper.js` だけでは deterministic_test を満たさない。
- `cargo test` 0 tests は deterministic_check を満たさない。
- `npm run build` は buildable には効くが gameplay/browser_interaction までは満たさない。

### Phase 4: Next.js build/dependency repair gate

目的:

- Next.js の postcheck failure を loop 内で修復または明示停止する。

作業:

- package coherence check
- TypeScript/React/Next major coherence
- build command failure feedback
- deferred verify の covered 判定見直し

受け入れ条件:

- `typescript@5.0.0` のような解決不能 version を completion 前に止める。
- TSX compile error を feedback に含める。
- build 未実行/未成功の状態を `completion_contract_satisfied` にしない。

### Phase 5: finalization cap

目的:

- `completion_contract_satisfied` の意味を acceptance contract に近づける。

作業:

- required capabilities 未充足なら finalization cap
- postcheck-required なのに未実行/未成功なら finalization cap
- eval events に `runtime_acceptance_contract_summary` を出す

受け入れ条件:

- false positive 3 件が runtime 上も成功扱いにならない。
- failure kind が `missing_required_capabilities` または `postcheck_required_not_satisfied` として分類される。

### Phase 6: 横展開

目的:

- plan-run / ultra-plan-run に同じ穴を残さない。

作業:

- step prompt に required capabilities を渡す。
- step verify が weak な場合は plan lint / runtime repair へ戻す。
- ultra final phase で final acceptance contract を確認する。

受け入れ条件:

- plan-run で高スコア YAML なのに実行が浅いケースが減る。
- ultra-plan-run で「アプリは存在するがゲームがない」ケースを acceptance で検出し、runtime events でも原因を追える。

### Phase 7: regression eval

目的:

- MVP minimal-loop が anvildev に劣る状態を是正できたか確認する。

作業:

- MVP minimal-loop cloud trend 3 回
- anvildev minimal-loop cloud trend 3 回
- full eval smoke 1 回
- false positive と acceptance success を比較

受け入れ条件:

- provider error を除外した 3-run trend で、MVP minimal-loop acceptance success の median が 11/12 以上、または anvildev median を下回らない。
- false positive の median が 1 件以下、かつ anvildev より悪化しない。
- artifact success 12/12 を維持。
- Next.js は dependency setup が成立するケースで build/dev readiness が anvildev と同等。dependency resolution が成立しない場合は、classified failure として扱い false positive にしない。
- `completion_contract_satisfied` で停止した run は required capabilities/evidence を満たしている。
- unclassified failure を増やさない。
- 速度劣化は 3-run median で +20% 以内を目安とする。ただし acceptance / false positive 改善を優先し、超過時は自動不合格ではなく原因分析を必須にする。

## 12. テスト計画

### Unit tests

追加対象:

- `mvp/anvilminimal/src/minimal_loop/completion.rs`
- `mvp/anvilminimal/src/minimal_loop/loop_run.rs`
- `mvp/anvilminimal/src/planner/profiles/nextjs.rs`
- `mvp/anvilminimal/scripts/eval_lib/*`

ケース:

- CompletionContract v1 後方互換
- CompletionContract v2 required capability parse/dedupe
- deterministic_test missing
- deterministic_test satisfied by self-test
- deterministic_check missing when cargo test has 0 tests
- deterministic_check satisfied when Rust `#[test]` exists
- weak postcheck classification
- Next.js invalid dependency version
- Next.js React/@types major mismatch
- deferred build not covered by weak static profile alone

### Integration tests

ケース:

- fake provider で `date-helper.js` exports only を返すと completion しない。
- fake provider が次 turn で assert self-test を追加すると completion する。
- fake provider で Rust `Hello, world!` だけを返すと completion しない。
- fake provider が `#[test]` を追加すると completion する。
- fake provider で Next.js compile error を残すと repair feedback が出る。

### Eval tests

ケース:

- `fix-js-date-helper-small minimal-loop`
- `rust-cli-medium minimal-loop`
- `nextjs-space-invaders-large minimal-loop`
- plan-run / ultra-plan-run の代表 1 ケースずつ

確認値:

- `acceptance_success`
- `acceptance_false_positive`
- `source_semantic_score`
- `build_success`
- `launch_success`
- `finalization_score`
- `runtime_acceptance_contract_summary`
- `stop_reason`

## 13. 是正確認方法

### 13.1 事前確認

```bash
cargo test --manifest-path mvp/anvilminimal/Cargo.toml
cargo build --release --manifest-path mvp/anvilminimal/Cargo.toml
```

### 13.2 minimal-loop focused eval

MVP:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp-acceptance \
  --binary-kind anvilminimal \
  --modes minimal-loop \
  --no-local-llm \
  --out /private/tmp/anvilminimal-eval-017-mvp-minimal
```

anvildev:

```bash
python3 mvp/anvilminimal/scripts/eval-run.py \
  --suite mvp-acceptance \
  --binary-kind anvildev \
  --modes minimal-loop \
  --no-local-llm \
  --out /private/tmp/anvilminimal-eval-017-anvildev-minimal
```

### 13.3 比較基準

必須:

- provider error を除外した 3-run trend で MVP minimal-loop `acceptance_success` median が 11/12 以上、または anvildev median を下回らない。
- provider error を除外した 3-run trend で MVP minimal-loop `acceptance_false_positive` median が 1 以下、かつ anvildev 以下。
- MVP minimal-loop `artifact_success = 12/12`
- MVP Next.js minimal-loop は dependency setup が成立するケースで `build_success = true` / `launch_success = true`。dependency resolution が成立しない場合は classified failure になり、false positive にならない。
- remaining failure は `missing_required_capabilities`, `postcheck_failure`, `dependency_missing`, `provider_http_status` などの既知分類に落ち、`unclassified_process_failure` を増やさない。

望ましい:

- MVP minimal-loop acceptance success が anvildev と同等以上
- false positive が anvildev 以下
- `completion_contract_satisfied` の場合、required capabilities も満たしている
- speed regression は 3-run median で +20% 以内

### 13.4 成果物の手動確認ではなく機械確認する観点

人間レビューに依存しないため、以下を自動化する。

- JS/TS/Python/Rust test artifact existence
- assertion / test framework / `#[test]` の存在
- 0 tests rejection
- package dependency coherence
- build/dev readiness
- UI/browser interaction oracle の adapter point
- source semantic oracle の required capability coverage

## 14. 過適応リスクと抑制策

### リスク

- `date-helper.js`, `space invaders`, `rust-cli` など個別ケースに寄った hardcode になる。
- eval の required capability 名だけを満たす表層対策になる。
- runtime が重くなり、MVP の小ささを失う。

### 抑制策

- capability 名は汎用語に限定する。
- verifier はファイル名ではなく言語/テスト/ビルド構造を見る。
- source semantic oracle を runtime に丸ごと移さない。
- runtime は「完了前の最低限 evidence gate」に限定する。
- 重い browser oracle は eval 側 adapter とし、runtime は要求がある場合の evidence presence までに留める。
- anvildev との比較だけでなく blind eval でも確認する。

## 15. 最終判断

MVP minimal-loop が anvildev に劣る問題は、LLM provider の偶然ではなく、runtime の完了契約不足として再現性がある。

特に以下が根本である。

- artifact success を completion authority に近く扱いすぎている。
- functional capability が runtime contract に渡っていない。
- weak postcheck を補強できない。
- postcheck failure を修復 feedback に戻せない。
- 移植元の TaskContract / verifier / deterministic fallback の思想を、小さい MVP 版 completion contract として再設計できていない。

次の修正は、MVP を大きくすることではなく、`CompletionContract v2` と `RuntimeAcceptanceContract` を小さく追加し、「完了には evidence が必要」という移植元の設計思想を MVP の停止条件に反映することを目的にする。
