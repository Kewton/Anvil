# v0.4.4 汎用・非ユースケース特化に向けた課題整理

## 目的

Anvil の task completion / fallback / verifier repair を、FastAPI、ToDo、特定モデル、特定テスト失敗に寄せた個別対応ではなく、汎用的な制御構造として整理する。

直近の Issue 632 では、artifact completion と verifier repair の状態管理は前進した。一方でコード確認の結果、production code にはまだ特定ユースケース向けの scaffold、モデル固有 fallback、文字列パターンによる判定が残っている。

本ドキュメントは、次の改善対象を明確化するための課題整理である。

## レビュー反映サマリー

本版では、以下のレビュー指摘を反映する。

- `ProjectVerifier` / cheap check が任意コマンド実行に近づくリスクを明示し、安全境界を追加する。
- `RequiredBehaviorContract` の抽出者、検証者、信頼境界、失敗時の扱いを明確化する。
- 特化 fallback の扱いを「即削除」ではなく、production completion path からの切り離し、feature flag 隔離、削除判断の順に整理する。
- `RepairJob`、`RequiredBehaviorContract`、`ProjectVerifier` を一度に導入しないよう、複数 Issue に分割する。
- diagnostic LLM が失敗した場合の failure report と停止条件を定義する。
- grep / unit test / UAT で確認できる受け入れ条件を追加する。

## GitHub Issue 構成

この Issue は親 tracking issue として扱う。

子 Issue:

- #634: 特化 fallback の production path からの切り離し
- #635: `RequiredBehaviorContract` の最小 schema と抽出フロー導入
- #636: behavior-aware artifact completion の導入
- #637: verifier repair を `RepairJob` 状態機械に統合
- #638: verifier parser の責務縮小と failure report 化
- #639: `ProjectVerifier` / cheap check の安全な一般化

本来の目的である「FastAPI CRUD + README + test のような要求を、特定テンプレートやユースケース専用 patch に依存せず安定完了させる」には、少なくとも #634、#637、#638 が必要である。#635、#636 は completion quality を上げるために必要で、#639 は verifier / repair の安全性を維持するために必要である。

#633 単体、またはいずれか 1 件の子 Issue 単体では本来目的は達成しない。#634-#639 を依存順に実施することで達成可能になる。

## Issue 634-639 の進め方

### 基本方針

- #633 は tracking issue とし、実装は #634-#639 の子 Issue で進める。
- 1 Issue ごとに「レビュー、設計確認、実装、単体テスト、代表 UAT」を完了させる。
- 複数 Issue を同時に大きく実装しない。特に `RequiredBehaviorContract`、`RepairJob`、`ProjectVerifier` は状態が増えるため、混ぜると失敗時の原因切り分けが困難になる。
- 各 Issue の完了後に、FastAPI / ToDo など特定ユースケースの成功だけでなく、特化 fallback や固定パターンが増えていないことを確認する。

### 推奨順序

1. #634: 特化 fallback を production path から切り離す。
2. #637: verifier repair を `RepairJob` 状態機械として管理する。
3. #638: verifier parser の責務を failure report 生成と安全境界に縮小する。
4. #635: `RequiredBehaviorContract` の最小 schema と抽出フローを導入する。
5. #636: behavior-aware artifact completion を導入する。
6. #639: `ProjectVerifier` / cheap check を安全に一般化する。

この順序にする理由:

- #634 を先に行わないと、以降の検証が「汎用制御で成功した」のか「特化 fallback で成功した」のか判断できない。
- #637 と #638 は verifier failure 後の未達に直結しており、成果物が verifier に到達した後の安定性を先に作る。
- #635 と #636 は completion quality を高める変更であり、repair loop が整理された後に入れる方が切り分けやすい。
- #639 は外部コマンド実行や package script の安全境界を含むため、failure report と repair job が整理された後に扱う。

### 依存関係

- #634 は全体の前提。production path から特化 fallback を外すことで、以降の Issue の検証が汎用制御の検証になる。
- #637 は #634 後に実施する。#635/#636 とは並行可能だが、同一 PR に混ぜない。
- #638 は #637 の一部または直後に実施する。parser の責務縮小と `RepairJob` の failure report は整合している必要がある。
- #635 は #634 後に実施する。contract は advisory data とし、completion の最終判定は Anvil 側の evidence で行う。
- #636 は #635 後に実施する。behavior-aware 判定は `RequiredBehaviorContract` を前提にする。
- #639 は #637/#638 後に実施する。safe verifier がない場合は `Unavailable` とし、任意コマンド実行で補わない。

### 各 Issue の検証ゲート

各 Issue 完了時に最低限確認すること:

- `cargo fmt --check`
- `cargo clippy --all-targets --all-features`
- `cargo test`
- `cargo build --release`
- 代表 UAT: fresh session で「実装、README、テスト」を要求し、特化テンプレートなしで artifact completion と verifier repair が進むこと
- `rg` による特化 fallback / 固定 patch の残存確認

UAT は成功可否だけでなく、ログ上の状態遷移も確認する。

- scaffold が completion evidence になっていないこと
- contract 不足中に generic retry へ流れないこと
- verifier failure 後に repair edit なしで verifier retry しないこと
- diagnostic 失敗時に通常 retry へ戻らず bounded failure report で停止すること
- repair / diagnostic / verifier output が main session の長期 instruction として混入しないこと

### 完了判定

#634-#639 の全体完了は、以下を満たした時点とする。

- production path に FastAPI / ToDo / FizzBuzz / qwen 固有 / fixed arithmetic patch のような特化補助が残っていない。
- artifact completion が file exists ではなく、artifact role、scaffold delta、required behavior、verifier result の evidence で判定される。
- verifier failure が `RepairJob` として、diagnostic、target selection、edit、rerun、re-diagnostic まで管理される。
- parser は root cause を決めず、path safety、secret masking、candidate extraction、failure report 生成に責務が限定される。
- cheap check / verifier は安全 gate、cwd confinement、timeout、approval policy を通り、unsafe な package script を自動実行しない。
- FastAPI CRUD + README + test の要求が、特定テンプレートやユースケース専用 patch なしに完了へ進む。

## 現状評価

### 改善済みの点

- scaffold は completion ではなく bootstrap として扱う方向に寄っている。
- artifact role、file classification、scaffold hash delta によって、単なる file exists では完了扱いしない構造になり始めている。
- verifier failure 後に、main session とは別の diagnostic pass を使う構造が入った。
- controller repair は、LLM に自由な tool call をさせず、JSON edit intent を検証してから適用する方向に進んだ。
- verifier repair 後は stale plan を続けず、verifier を再実行するようになった。

### 残っている構造的問題

- fallback と recovery がまだ「汎用状態機械」ではなく、一部の特定 scaffold / model / language 補助に依存している。
- task contract は artifact role を扱うが、ユーザー要求から必要 behavior を抽出して成果物と照合する `RequiredBehaviorContract` までは実装されていない。
- verifier failure の一次分類は diagnostic LLM に寄り始めたが、補助的な target 抽出や failure type 判定に文字列パターンが多く残っている。
- deterministic fallback が「最後の補助」ではなく、特定スタックの scaffold 生成機構として残っている。

## 課題一覧

### 1. FastAPI 専用 scaffold が production path に残っている

該当:

- `deterministic_fastapi_scaffold_files()`
- `maybe_materialize_mode_deterministic_fallback()`
- `maybe_materialize_task_contract_fallback()`

問題:

- `app/main.py`、`tests/test_health.py`、`README.md`、`/health` を固定生成している。
- FastAPI 以外のバックエンドや同じ FastAPI でも異なる構成に自然に拡張できない。
- ユーザー要件ではなく、Anvil 側の既知 scaffold が初期成果物を決めてしまう。

あるべき姿:

- framework-specific scaffold は completion path から外す。
- scaffold を使う場合は、言語や framework ではなく「artifact role を満たす最小 placeholder」を生成するだけに留める。
- 生成後の completion は必ず `RequiredBehaviorContract` と verifier に委ねる。

### 2. Python CSV / FizzBuzz / help test fallback が特化している

該当:

- `deterministic_empty_python_cli_files_with_names()`
- `maybe_materialize_python_test_fallback()`
- FizzBuzz 専用 test 生成分岐

問題:

- CSV 集計や FizzBuzz は明確なユースケース特化である。
- 「テストが無い」問題を、特定サンプルテストの生成で埋めている。
- 汎用的な test artifact completion ではなく、既知パターンの穴埋めになっている。

あるべき姿:

- test fallback は具体テストを生成しない。
- 必要なら `test artifact missing` という job state を作り、LLM に対象実装を読ませたうえでテストを書かせる。
- controller が許可するのは target selection と edit protocol までで、テスト内容は LLM が現在の実装とユーザー要求から生成する。

### 3. qwen3.5 固有 fallback が残っている

該当:

- `maybe_finish_after_qwen35_edit_format_error()`
- `maybe_apply_qwen35_obvious_edit_fallback_after_format_error()`
- `pub fn multiply` / `left + right` などの deterministic patch

問題:

- モデル名と既知の失敗形に直接依存している。
- 特定の Rust サンプル修正を deterministic に適用しており、汎用性が低い。
- EffectiveToolPolicy に寄せる方針と一部重複する。

あるべき姿:

- モデル名ではなく capability によって制御する。
- malformed tool call 後の対応は、全モデル共通の `ToolProtocolRecovery` として扱う。
- deterministic patch は削除し、許可するなら「直前に読んだ target に対する controller edit protocol」だけにする。

### 4. TaskContract が artifact role 止まりで behavior を見ていない

現状:

- implementation / test / usage_docs / setup の有無を主に見る。
- scaffold hash delta により「scaffold から変わったか」は見られる。

不足:

- ユーザーが要求した domain / operation / public surface が成果物に反映されたかを確認していない。
- README や test が「何か書かれた」だけでも role evidence になり得る。

あるべき姿:

`RequiredBehaviorContract` を導入する。

責務:

- 抽出: Anvil が request text を control data として扱い、短命 classifier または deterministic fallback で JSON schema に抽出する。
- 検証: Anvil が成果物の role evidence、scaffold hash delta、verifier result、behavior coverage を突き合わせる。
- 実装: Main LLM が通常実装や artifact edit を行う。Main LLM には `RequiredBehaviorContract` を instruction ではなく data として渡す。
- 診断: Diagnostic LLM は verifier failure の原因分類だけを行い、behavior contract の最終判定者にはしない。

信頼境界:

- LLM が返した behavior contract は advisory data として扱う。
- JSON schema、enum、文字数、配列長、confidence を検証する。
- request に明示されていない behavior を LLM が追加しても required 扱いしない。
- 抽出不能な項目は `unknown` とし、unknown だけを理由に completion を失敗させない。
- explicit artifacts、scaffold unchanged、verifier failure は deterministic に判定し、LLM 判定で上書きしない。

抽出対象:

- operations: create / read / update / delete / run / validate など
- artifact expectations: implementation / tests / usage docs / setup
- domain terms: ユーザー要求に明示された名詞や対象
- interface hints: CLI / API / UI / library など
- verification expectation: test / build / run / smoke など

completion 判定:

- implementation は required behavior の主要語や interface hint と矛盾しない。
- test は implementation-derived surface だけでなく required behavior を検証している。
- usage docs は setup / run / verification / usage surface を説明している。
- scaffold unchanged は常に未完成扱い。

最小 schema:

```json
{
  "operations": ["create", "read"],
  "domain_terms": ["task"],
  "interface_hints": ["api"],
  "required_artifacts": ["implementation", "test", "usage_docs"],
  "verification": ["test"],
  "confidence": 0.0
}
```

この schema は completion 判定の補助であり、任意の自然言語評価を許可するものではない。

### 5. verifier failure 分類が文字列パターンに依存している

該当:

- `classify_verifier_failure_type()`
- `verifier_output_line_is_non_fatal_warning()`
- `verifier_line_names_import_provider()`
- `verifier_failure_error_kind()`
- path-like token extraction と scoring

問題:

- `ImportError`、`AssertionError`、`warning` など既知ログ表現への依存がある。
- 言語や test runner が増えるほど分岐が増える。
- パターンマッチの追加が「モグラ叩き」になりやすい。

あるべき姿:

- verifier output はまず構造化された `VerifierFailureReport` に変換する。
- 変換は deterministic parser だけで完結させず、diagnostic LLM を protocol 化して使う。
- deterministic parser は security boundary と candidate extraction に限定する。
- root cause classification は bounded diagnostic pass の JSON schema に集約する。

diagnostic unavailable 時の扱い:

- diagnostic pass が timeout、malformed JSON、schema validation failure で上限に達した場合、通常 retry に戻さない。
- `verifier_failed` として停止し、`VerifierFailureReport` を出力する。
- failure report には command、masked output excerpt、changed candidates、safe target candidates、diagnostic error を含める。
- failure report は次回の修復や Issue 化に使える control data とし、main session に長期注入しない。

### 6. target selection に固定スコアが残っている

該当:

- implementation / setup / test / docs の role score
- verifier output path と changed files の重み付け
- import provider 優先
- assertion が続いたら test target に切り替える補正

問題:

- 個別の経験則が増えやすい。
- verifier failure の意味ではなく、ログに出た path の重みで target が決まりやすい。
- target switching の条件が局所的で、修復ジョブ全体の状態として扱えていない。

あるべき姿:

- target selection は `RepairJob` の状態遷移として扱う。
- diagnostic result に `repair_plan` を持たせ、controller はその plan を検証して順に実行する。
- 同一 failure が残る、悪化する、別 failure に変わる、改善する、という rerun outcome から plan を更新する。
- deterministic scoring は fallback candidate のみに使い、最終判断は diagnostic schema に寄せる。

### 7. cheap check が Python 偏重

該当:

- `.py` / `.pyw` だけ `py_compile`
- 他言語は cheap check disabled

問題:

- Python では syntax regression を事前に防げるが、Rust / TS / JS / Go などでは同等の保護がない。
- かといって各言語の compiler を直接増やすと、また rule-based になる。

あるべき姿:

- cheap check は `ProjectVerifier` の一種として抽象化する。
- 言語別 command を増やすのではなく、既存 workspace から検出済み verifier / formatter / parser を利用する。
- cheap check が無い場合は、編集範囲制約と verifier rerun で安全性を担保する。

安全境界:

- `ProjectVerifier` は「検出したコマンドをそのまま実行する」仕組みにしない。
- 実行可能な command は既存の shell safety gate、approval policy、cwd confinement、timeout、secret masking を通す。
- shell control operator、redirect、subshell、command substitution、network install、destructive command は cheap check evidence として扱わない。
- package script や formatter script は任意コードになり得るため、非対話 approval なしに追加実行しない。
- safe verifier が無い場合は cheap check を `Unavailable` とし、controller edit validation と full verifier rerun に委ねる。

### 8. deterministic fallback が「template completion」に寄りすぎている

問題:

- fallback が成果物生成に近づくほど、特定ユースケース化する。
- 「LLM が止まった時の補助」と「Anvil が代わりに実装する」が混ざる。

あるべき姿:

- fallback の責務は job state を進めるための最小 scaffold / target note / protocol retry に限定する。
- domain-specific implementation は LLM が行う。
- Anvil は completion criteria と verifier を管理する。

### 9. request intent 判定が keyword list に依存している

該当:

- FastAPI / Flask / Django / Python / Rust
- React / Next / Vue / Vite
- game / UI / CSV / test など

問題:

- keyword list が増えるほどルールベース化する。
- 多言語・多フレームワークに拡張すると保守不能になる。

あるべき姿:

- keyword は初期候補抽出のみに限定する。
- task intent / required behavior は short-lived classifier に JSON schema で抽出させる。
- deterministic keyword 判定は safety fallback として残すが、completion の根拠にしない。

## 目標構造

### 中心概念

- `TaskContract`
  - ユーザー要求から必要 artifact と verification requirement を表す。
- `RequiredBehaviorContract`
  - ユーザー要求から必要 behavior / domain terms / interface hints を表す。
- `ArtifactState`
  - file exists / scaffold unchanged / changed / verified を表す。
- `CompletionEvidence`
  - role に対応する edit、verifier success、behavior coverage を表す。
- `RepairJob`
  - verifier failure から diagnostic / repair / rerun / re-diagnostic を管理する。
- `EffectiveToolPolicy`
  - 次に許可される tool と target を制御する。

### 役割分担

Anvil:

- 状態を持つ。
- 次の job を決める。
- tool policy を絞る。
- LLM 出力を検証する。
- verifier を実行する。
- scaffold / edit / verifier の証跡を管理する。

Main LLM:

- 通常実装。
- artifact target に対する編集。
- controller から指定された target の修復。

Diagnostic LLM:

- verifier output と bounded excerpts から failure kind / cause role / repair plan を JSON で返す。
- tool call や patch は出さない。

Repair editor LLM:

- selected target に対する JSON edit intent だけを返す。
- controller が検証し、適用する。

## 対応方針

### Phase 1: 特化 fallback の棚卸しと封じ込め

- FastAPI scaffold を direct completion path から外す。
- Python CSV / FizzBuzz fallback を production completion path から外し、必要なら明示 feature flag 配下の experimental 扱いにする。
- qwen3.5 関数名を capability-based recovery に置き換える。
- deterministic patch を削除する。

順序:

1. production completion path から切り離す。
2. 既存挙動が必要な場合は明示 feature flag 配下へ隔離する。
3. UAT と代替 recovery が安定した後に削除判断する。

受け入れ条件:

- production path に ToDo / FizzBuzz / fixed arithmetic patch が存在しない。
- model name ではなく `ModelCapabilities` だけで recovery が分岐する。
- deterministic scaffold は completion evidence にならない。
- FastAPI scaffold は明示 feature flag なしに task completion fallback として発火しない。

### Phase 2: RequiredBehaviorContract を導入

- request から behavior contract を抽出する。
- artifact role completion と behavior coverage を分離する。
- README / test completion は file exists ではなく behavior coverage を見る。

受け入れ条件:

- README が scaffold から変わっただけでは usage_docs complete にならない。
- test が health check だけなら、CRUD 要求の test complete にならない。
- FastAPI / ToDo 専用語ではなく、request-derived behavior terms で判定する。

### Phase 3: RepairJob 状態機械に統合

- verifier repair を `RepairJob` として管理する。
- diagnostic result、selected target、applied edits、rerun outcome、retry budget を job に持たせる。
- generic retry / focused edit recovery / verifier repair retry を分離する。

受け入れ条件:

- verifier failure 後、修復 edit なしに verifier retry しない。
- repair edit 後は必ず verifier rerun する。
- 同じ failure が残る場合は re-diagnostic する。
- diagnostic unavailable は明示的な verifier_failed で止める。

### Phase 4: verifier parsing を安全境界に限定

- deterministic parser は path safety、secret masking、candidate extraction に限定する。
- failure meaning は diagnostic schema に寄せる。
- warning / assertion / import などの個別判定を最小化する。

受け入れ条件:

- parser は target candidate を出すだけで root cause を決めない。
- root cause は diagnostic JSON と validation policy で扱う。
- verifier output に含まれる命令文を instruction として扱わない。

### Phase 5: ProjectVerifier / cheap check の一般化

- cheap check を言語別 hardcode ではなく project-local verifier capability として扱う。
- 既存 test/build command があれば、それを bounded check として使う。
- cheap check が無い場合でも edit validation と full verifier rerun で補う。

受け入れ条件:

- Python だけ特別に成功率が高い構造を避ける。
- compiler / formatter / test runner の直接増殖を避ける。
- shell command は既存の安全 gate を通す。
- unsafe な package script は cheap check として自動実行されない。

## セキュリティ方針

- verifier output、file excerpt、LLM diagnostic result はすべて untrusted data として扱う。
- diagnostic LLM には tool call を許可しない。
- repair editor LLM には selected target 以外の edit を許可しない。
- path は workspace-relative のみ許可し、absolute path / `..` / symlink escape を拒否する。
- secret masking 後の excerpt だけを LLM に渡す。
- shell command は completion evidence gate と approval policy を通す。
- deterministic fallback は外部ネットワークや任意コマンドを増やさない。
- `RequiredBehaviorContract` と diagnostic JSON は schema validation 後の control data として保存し、会話 instruction として扱わない。
- verifier / cheap check command は、既存の command safety gate、timeout、cwd confinement、secret redaction を満たさない限り自動実行しない。

## 非目標

- FastAPI 専用 README 生成を増やすこと。
- ToDo 専用 test 生成を増やすこと。
- pytest assertion 専用 repair を増やすこと。
- qwen 固有関数を増やすこと。
- 特定 endpoint 名に依存した completion 判定を追加すること。
- framework template を増やして成功率を上げること。

## 完了判定

以下を満たす状態を v0.4.4 のゴールとする。

- production path から明確なユースケース専用 fallback が削除または無効化されている。
- task completion が artifact role と required behavior の両方で判定される。
- scaffold unchanged は completion evidence にならない。
- verifier repair は RepairJob として完了まで管理される。
- diagnostic / repair は main session のコンテキストを汚さない。
- LLM は「次どうするか」を決めず、Anvil が state / evidence / verifier result から次 job を決める。
- 新しいユースケースを追加しても、個別 template や専用 patch を追加しなくてよい。

テスト可能な受け入れ条件:

- `rg "deterministic_fastapi_scaffold_files|FizzBuzz|pub fn multiply|left \\+ right" src/agent/loop_run src/model_capabilities.rs` で production completion fallback に残っていないことを確認できる。
- qwen 固有の関数名ではなく、capability 名で tool protocol recovery が制御されている。
- scaffold README / scaffold health test は、hash が未変化なら completion evidence にならない。
- test / docs completion は、file exists だけでなく behavior coverage を要求する unit test がある。
- diagnostic malformed / timeout は generic retry に戻らず、`verifier_failed` と bounded failure report で停止する。
- unsafe な verifier / package script は cheap check として自動実行されない。

## 優先度

1. qwen / FizzBuzz / arithmetic deterministic patch の除去
2. FastAPI scaffold の completion path からの切り離し
3. RequiredBehaviorContract の最小導入
4. RepairJob 状態機械の明文化と retry budget 統合
5. verifier parser の責務縮小
6. cheap check の ProjectVerifier 化

## リスク

- 特化 fallback を外すと短期的な UAT 成功率は下がる可能性がある。
- RequiredBehaviorContract が弱いと、README/test の完了判定が曖昧になる。
- diagnostic LLM に寄せすぎると、診断不安定性が repair 成功率に影響する。
- 汎用化しすぎると、local small model に渡す指示が抽象的になりすぎる。

対策:

- controller state は明確に保つ。
- LLM には小さい target と bounded schema だけを渡す。
- behavior contract は最初は小さく導入し、過剰な semantic 判定を避ける。
- fallback は成果物生成ではなく、job progression の補助に限定する。

## 複数 Issue への分割案

### Issue A: 特化 fallback の production path からの切り離し

目的:

- FastAPI、Python CSV、FizzBuzz、fixed arithmetic patch、qwen 関数名などを production completion path から切り離す。

対象:

- `deterministic_fastapi_scaffold_files()`
- `deterministic_empty_python_cli_files_with_names()`
- `maybe_materialize_python_test_fallback()`
- `maybe_apply_qwen35_obvious_edit_fallback_after_format_error()`
- qwen 固有関数名

受け入れ条件:

- 特化 fallback は明示 feature flag なしに task completion fallback として発火しない。
- model name 直接分岐ではなく `ModelCapabilities` で recovery が決まる。
- fixed arithmetic patch と FizzBuzz 専用 test 生成が production path から消える。
- 既存 UAT は「特化 fallback なしでも、LLM edit + artifact recovery + verifier repair で進む」ことを確認する。

依存:

- なし。最優先。

### Issue B: RequiredBehaviorContract の最小 schema と抽出フロー導入

目的:

- ユーザー要求から behavior / domain terms / interface hints / required artifacts を bounded schema として抽出する。

対象:

- `TaskContract`
- 新規 `RequiredBehaviorContract`
- request classifier / deterministic fallback parser

受け入れ条件:

- JSON schema、enum、配列長、文字数、confidence validation がある。
- LLM が request に無い behavior を追加しても required 扱いしない。
- 抽出不能時は `unknown` とし、unknown だけで completion を失敗させない。
- control data は main session の instruction として永続注入されない。

依存:

- Issue A の後。

### Issue C: behavior-aware artifact completion の導入

目的:

- implementation / test / usage_docs の completion を file exists ではなく behavior coverage と結びつける。

対象:

- `ArtifactState`
- `CompletionEvidence`
- scaffold hash delta
- usage docs / test artifact 判定

受け入れ条件:

- scaffold unchanged は completion evidence にならない。
- README が scaffold から変わっただけでは usage_docs complete にならない。
- health test だけでは CRUD など explicit operation の test complete にならない。
- completion 判定は FastAPI / ToDo / endpoint 固有語に依存しない。

依存:

- Issue B。

### Issue D: verifier repair を RepairJob 状態機械に統合

目的:

- verifier repair を generic retry から分離し、diagnostic / target selection / edit / rerun / re-diagnostic を job として管理する。

対象:

- `VerifierRepairContext`
- `VerifierRepairDecision`
- verifier repair retry budget
- repair rerun outcome

受け入れ条件:

- verifier failure 後、修復 edit なしに verifier retry しない。
- repair edit 後は必ず verifier rerun する。
- 同じ failure が残る場合は stale plan を続けず re-diagnostic する。
- diagnostic unavailable は generic retry に戻らず `verifier_failed` で止まる。
- failure report が bounded / masked /再利用可能な形で出力される。

依存:

- Issue A。Issue B/C とは並行可能。

### Issue E: verifier parser の責務縮小と failure report 化

目的:

- deterministic parser を security boundary と candidate extraction に限定し、root cause 判断を diagnostic schema に寄せる。

対象:

- `classify_verifier_failure_type()`
- warning / assertion / import 系 pattern matching
- target candidate extraction
- `VerifierFailureReport`

受け入れ条件:

- parser は root cause を確定しない。
- path safety、secret masking、candidate extraction は deterministic に維持する。
- diagnostic JSON が root cause / repair plan を返す。
- malformed diagnostic でも failure report が残る。

依存:

- Issue D と密接。Issue D の前半または直後。

### Issue F: ProjectVerifier / cheap check の安全な一般化

目的:

- Python 偏重の cheap check を project-local verifier capability として整理する。ただし任意コマンド実行を増やさない。

対象:

- cheap check policy
- command safety gate
- approval / timeout / cwd confinement
- verifier command reuse

受け入れ条件:

- package script や formatter script は安全条件を満たさない限り自動実行されない。
- safe verifier が無い場合は `Unavailable` として扱う。
- cheap check 失敗時も verifier output / diagnostic data は secret masked される。
- Python だけの `py_compile` は互換 fallback として残す場合も、`ProjectVerifier` の一実装に閉じる。

依存:

- Issue D/E の後。

### 推奨実施順

1. Issue A
2. Issue D
3. Issue E
4. Issue B
5. Issue C
6. Issue F

理由:

- まず特化 fallback を production path から外さないと、以降の UAT が汎用制御の検証にならない。
- verifier repair の状態機械は現在の未達原因に直結しているため、BehaviorContract より先に安定化させる。
- BehaviorContract は completion quality を上げるが、先に導入すると状態が増えて切り分けが難しくなる。
- ProjectVerifier はセキュリティ境界が重いため、repair / failure report が整理された後に扱う。
