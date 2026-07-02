# CommandAgent Local Recovery Plan

作成日: 2026-06-17

## 目的

latest large eval root `20260617T110312` の失敗を、移植時に発生した
共通契約の問題と profile 固有の問題に再分類し、今すぐ直すべき局所範囲を
確定する。

本計画は、既存の広い `workspace/CommandAgent/recovery-plan.md` の続きでは
あるが、対象をさらに絞る。目的は成功率チューニングではなく、Anvil minimal
から CommandAgent へ移植した契約の意味を正しく戻すことである。

## 入力

- CommandAgent latest triage:
  - `/Users/maenokota/share/work/github_kewton/CommandAgent/docs/eval/triage/large-root-20260617T110312.md`
- CommandAgent latest large root:
  - `/Users/maenokota/share/work/github_kewton/CommandAgent/eval/runs/contract-recovery-large/20260617T110312`
- Anvil reference:
  - `src/agent/minimal_step_runner.rs`
  - `src/agent/minimal_step_runner/profile.rs`
  - `src/agent/minimal_step_runner/profiles/nextjs.rs`
  - `docs/eval/minimal-loop-large-ultra-plan-run-20260615.md`

## 判定サマリ

今回の事象は単純な typo ではない。根本原因は、Anvil で分かれていた
契約境界を CommandAgent 移植時に一部まとめ直し、その際に意味が変わった
ことである。

特に大きい問題は以下。

1. `required_artifacts` を最終成果物契約ではなく、phase step plan の
   hard gate として扱っている
2. `required_artifacts` が generated step plan に重複注入される
3. verifier command の lint / Bash policy が shell quoting を見ず、
   local check を compound shell と誤判定する
4. planner が生成する verifier が brittle な exact grep に寄っている
5. dependency setup の許可/不許可が profile、verifier、eval policy の間で
   まだ一貫していない
6. Python の top-level `app` import が workspace 外の package に解決される
   import isolation 問題が残っている

ただし、1-4 は共通契約の問題であり、profile を太らせる前に直すべきである。
5-6 は profile/eval 固有に近いが、1-4 の修正後に再評価してから扱う。

## 問題箇所と根本原因

### P0: `required_artifacts` の enforcement タイミングが早すぎる

#### 現象

`large-rust-app-modify` は phase 1 `analyze-structure` の step plan を実行後、
`src/lib.rs` が存在しないため失敗した。

しかし ultra plan では `src/lib.rs` を後続 phase で作る流れになっている。
分析 phase の時点で `src/lib.rs` が無いのは正常である。

#### CommandAgent 側の構造

`StepPlan` が `required_artifacts` を持つ。

```text
src/agent/step_runner/mod.rs
StepPlan {
  goal,
  profile,
  style,
  intent,
  required_artifacts,
  steps,
}
```

`execute_ultra_plan()` は ultra plan の `required_artifacts` を各 phase の
generated step plan に渡す。

`execute_step_plan()` は step plan 終了時に `plan.required_artifacts` を
missing check する。

```text
src/agent/step_runner/runtime.rs
execute_step_plan()
  ...
  missing_paths(self.cwd, &plan.required_artifacts)
```

#### Anvil 側の構造

Anvil の `StepPlan` は `required_artifacts` を持たない。

```text
src/agent/minimal_step_runner.rs
StepPlan {
  goal,
  steps,
}
```

Anvil は required final artifacts を phase/step prompt に pressure として
出すが、各 phase step plan の終了条件にはしない。step-local な hard gate は
`expected_paths` で行う。

#### 根本原因

移植時に「最終成果物契約」と「step-local 成果物契約」を同じ型に寄せた。

- `required_artifacts`: タスク全体の最終成果物
- `expected_paths`: その step が完了した時点で存在すべき成果物

この 2 つは意味が違う。CommandAgent では `required_artifacts` を step plan
に持たせたうえで hard gate にしたため、final contract が phase-local
contract に変質した。

#### 対策

1. `execute_step_plan()` から `required_artifacts` の missing check を除去する。
2. step plan 実行中の hard gate は `step.expected_paths` のみとする。
3. ultra plan 全体の最後でのみ `UltraPlan.required_artifacts` を確認する。
4. `/run-plan` のような単独 step plan 実行では、`required_artifacts` は
   prompt/reporting の文脈として残してもよいが、hard gate にはしない。
5. tests:
   - inspect-only phase が final artifact 未作成でも次 phase に進む
   - final phase 後に artifact が無ければ ultra plan は失敗する
   - step.expected_paths が欠けている場合は従来通り step 失敗する

### P0.1: `required_artifacts` の重複注入

#### 現象

`large-rust-app-modify` の generated step plan では `required_artifacts` が
重複していた。

```yaml
required_artifacts:
  - Cargo.toml
  - src/main.rs
  - src/lib.rs
  - Cargo.toml
  - src/main.rs
  - src/lib.rs
```

このため error も `src/lib.rs, src/lib.rs` のように重複する。

#### 根本原因

`ensure_generated_plan_header()` は requested header を先頭に注入し、モデルが
出した `required_artifacts` を除去しようとしている。しかし YAML 断片の
indent / 位置 / list 形式への耐性が弱く、モデル出力側の list が残るケースが
ある。

また、parse 後に dedupe していないため、同じ artifact が複数回残る。

#### 対策

1. `required_artifacts` を parse 後に repository-relative path として
   normalize + stable dedupe する。
2. `ensure_generated_plan_header()` の strip 処理を強めるより、parse 後の
   canonicalization を必ず通す。
3. tests:
   - model output に duplicate required_artifacts がある
   - indented / unindented の両方で重複しない
   - error message に duplicate path が出ない

### P1: verifier command 契約が syntax-naive

#### 現象

`large-fastapi-app-new` では以下が plan lint で拒否された。

```text
python -c "import ast; ast.parse(open('app/main.py').read())"
```

この semicolon は Python code の中にあり、shell chaining ではない。
しかし CommandAgent の lint は `command.contains(';')` で拒否している。

`large-nextjs-app-new` では以下が Bash policy で `Unknown` block になった。

```text
grep '"next"' package.json
```

policy は `grep -q` だけを read-only として許可しており、plain `grep` は
planner が自然に出す local check だが弾かれる。

#### 根本原因

「verifier は単純な local check」という設計は正しいが、実装が文字列包含に
寄りすぎている。

問題は 2 つある。

- lint は shell quoting を理解しない
- policy と planner prompt の verifier 語彙が揃っていない

#### 対策

過度な shell whitelist ではなく、canonical verifier 契約を作る。

1. planner prompt に canonical verifier を明示する。
   - file existence: `test -f path`
   - Python syntax: `python -m py_compile path.py`
   - Python tests: `python -m pytest ...`
   - Rust: `cargo check` / `cargo test`
   - Next.js: `npm run build`
   - text contains: `grep -q pattern path`
2. lint は `;`, `&&`, `||` を quote-aware に判定する。
3. Bash policy は `grep -q` を推奨のままにする。plain `grep` を許可するかは
   P1 修正後の再評価で決める。最初から広げない。
4. tests:
   - quoted semicolon in `python -c` is accepted or canonicalized away
   - unquoted semicolon remains rejected
   - generated plans prefer `python -m py_compile` over `python -c`
   - `grep -q` verifier passes policy

### P1.1: generated verifier が brittle exact grep に寄る

#### 現象

`large-rust-app-new` は `#[derive(Parser, Debug)]` を生成したが、verifier は
以下を期待して失敗した。

```text
grep -q "#\[derive(Parser)\]" src/main.rs
```

Rust としては `Parser` derive を含んでいるため、検証のほうが brittle。

#### 根本原因

planner prompt が「source code に対する verifier は semantic command を優先」
という契約を十分に持っていない。

`expected_paths` と `verify` の責務は分かれているが、`verify` の品質基準が
まだ弱く、literal grep が source semantics の代用になっている。

#### 対策

P1 と同じ PR に含めてよい。

1. source code task の verifier は build/test/check を優先する。
2. grep は literal docs/data/content requirement に限定する。
3. tests:
   - Rust new plan prompt snapshot に `cargo check`/`cargo test` 優先が出る
   - exact derive grep のような fixture を plan lint で warning/reporting するか、
     correction prompt に誘導する

### P2: dependency setup の policy が曖昧

#### 現象

Next.js modify は requested artifact を作ったが、`npm run build` が
`node_modules/.bin/next` 不在で `dependency_missing` になった。

repair は `npm install` へ進もうとし、offline policy に block された。

#### 根本原因

次の 3 つがまだ一致していない。

- profile: dependency missing の場合は install when allowed / report
- verifier: `npm run build` は dependency_missing を返す
- eval policy: dependency install を許可するのか、preseed するのか、不可として
  dependency_missing 扱いにするのか

これが決まっていないため、モデルは「直す」と「止まる」のどちらが正解かを
背負っている。

#### 対策

P0/P1 の後に扱う。今回の局所リカバリでは実装しない。

候補:

1. eval large Next.js は dependencies を fixture/preseed する
2. offline eval では dependency_missing を implementation failure と分けて
   summary する
3. setup step で install を許可する別モードを明示する

この判断は product policy なので、local runtime bug 修正とは分ける。

### P3: Python import isolation

#### 現象

FastAPI modify は workspace に `app/schemas.py` を作ったが、pytest は別 repo の
`app.schemas` を import した。

#### 根本原因

top-level `app` package が曖昧で、workspace 側に `app/__init__.py` が無い。
Python の import resolution で外部の regular package が拾われる余地がある。

これは CommandAgent の core loop ではなく、Python profile / eval environment の
package hygiene 問題。

#### 対策

P0/P1 の後に扱う。

候補:

1. Python/FastAPI profile で package-style app には `app/__init__.py` を要求する
2. verifier 実行時に workspace-first import を保証する
3. eval fixture に `app/__init__.py` を入れる

まず P0/P1 修正後の rerun で再現頻度を見る。

## 局所リカバリ順序

### LR-0: Local Baseline Freeze

目的:

- latest root の問題を局所リカバリの baseline として固定する。

作業:

- 本ファイルと review を commit する。
- latest triage `large-root-20260617T110312.md` を参照する。
- コード変更なし。

完了条件:

- 問題箇所、根本原因、対象外範囲が docs で追跡できる。

### LR-1: Final Artifact Enforcement Timing

目的:

- `required_artifacts` を final contract に戻す。

作業:

- `execute_step_plan()` 末尾の `plan.required_artifacts` missing check を削除する。
- `execute_ultra_plan()` 末尾の `UltraPlan.required_artifacts` missing check は残す。
- step-local hard gate は `expected_paths` のままにする。
- `StepPlan.required_artifacts` は prompt/reporting 用として残すか、将来的に削る。
  今回は互換性優先で残すが hard gate には使わない。

テスト:

- ultra plan phase 1 が inspect/report のみで final artifact 未作成でも phase 2 に進む。
- ultra plan 最後に final artifact が欠けていれば失敗する。
- `/run-plan` 単体では `required_artifacts` 欠落だけで失敗しない。
- `expected_paths` 欠落は従来通り失敗する。

受け入れ条件:

- `large-rust-app-modify` 型の早期停止が再発しない。
- final artifact missing の検出は ultra plan 完了時に残る。

### LR-2: Required Artifact Canonicalization

目的:

- 重複注入と noisy error をなくす。

作業:

- parse 後の `required_artifacts` を stable dedupe する。
- repository-relative path validation は維持する。
- `ensure_generated_plan_header()` の strip だけに依存しない。

テスト:

- duplicate artifacts fixture。
- indented/unindented artifacts fixture。
- rendered plan で重複が出ない。

受け入れ条件:

- saved plan と error message に duplicate required artifact が出ない。

### LR-3: Canonical Verifier Contract

目的:

- verifier command の共通契約を planner/lint/policy の間で揃える。

作業:

- step plan generation prompt に canonical verifier examples を追加する。
- source code verifier は semantic local checks を優先するよう明記する。
- lint の shell control detection を quote-aware にする。
- `python -c` は原則推奨しない。受け入れる場合も quoted semicolon を誤拒否しない。
- plain `grep` allow はこの段階では原則追加しない。まず planner を `grep -q` に寄せる。

テスト:

- quoted semicolon fixture。
- unquoted shell chaining fixture。
- Rust derive exact grep が推奨されないことを snapshot / correction で確認。
- `grep -q` は通る。

受け入れ条件:

- `large-fastapi-app-new` 型の false plan lint が減る。
- `large-rust-app-new` 型の brittle verifier が減る。

### LR-4: Focused Large Rerun

目的:

- P0/P1 修正だけで failure class がどう動くかを見る。

実行:

- `scripts/eval_large_tasks.sh --runs 1`
- 可能なら対象 subset:
  - `large-rust-app-modify`
  - `large-fastapi-app-new`
  - `large-rust-app-new`
  - `large-nextjs-app-new`

見る指標:

- phase 途中の `missing required final artifacts` が消えたか
- duplicate required_artifacts が消えたか
- plan lint false positive が減ったか
- verifier command block が減ったか
- dependency_missing / Python import isolation が残るか

完了条件:

- 残存 failure を `common residual` と `profile/eval specific` に再分類する。

## 今回は実装しない範囲

- Next.js dependency install policy の変更
- Python `app/__init__.py` 強制
- profile contract の拡張
- repair 回数増加
- sidecar / semantic summary
- provider/model 固有の prompt
- broad Bash whitelist

これらは LR-1 から LR-4 の後に、残存 failure として安定している場合だけ扱う。

## 成功条件

この局所リカバリの成功は `large eval 6/6 pass` ではない。

成功条件は以下。

1. final artifact contract が phase-local hard gate にならない。
2. required_artifacts duplicate が消える。
3. verifier command の false rejection が減る。
4. 残った failure が profile/eval specific として説明できる。
5. CommandAgent の設計思想、つまり「能力追加ではなく曖昧さの削減」に留まる。

