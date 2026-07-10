【共通制約】
対象クレート: mvp/anvilminimal（Rust）。本指示内のパスはすべてリポジトリルートからの
相対パスで記載する。作業前に mvp/anvilminimal/docs/dev-guardrails.md を読むこと。

- mvp/anvilminimal/src/planner/runner.rs と
  mvp/anvilminimal/src/minimal_loop/loop_run.rs は成長トリップワイヤ対象
  （mvp/anvilminimal/tests/generality_guardrails.rs が baseline+2% で fail する）。
  新ロジックは新規モジュールまたは既存のリーフモジュールに置き、これら2ファイルへの
  変更は呼び出し追加の最小限にとどめる。ベースライン定数の引き上げは禁止。
- 「正直な失敗」セマンティクスを壊さない: 検証を緩めて合格させる方向の変更は禁止。
  検証コマンドの書き換えは意味的に等価以上に厳密であること（本当にポート指定が
  無いpackage.jsonは引き続きfailすること）。
- 変更ごとにユニットテストを追加し、mvp/anvilminimal/tests/corpus/apps/ の既存規約
  （mvp/anvilminimal/tests/corpus/apps/test0710_stage0_pivot をテンプレートに
  expectations.toml + fixtures/*.jsonl）で回帰fixtureを追加する。
- 完了条件: cargo test --manifest-path mvp/anvilminimal/Cargo.toml が全パス。
- コミットは既存流儀（短い命令形英語、1タスク1コミット）。