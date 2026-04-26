# Anvil UAT Scenarios v5

目的: WorkMode / ModePolicy により、LLM 会話履歴を汚さずに用途別の実行方針を切り替えられることを検証する。TypeScript UI 以外の用途で UI fallback が誤発火しないこと、answer-only が repo edit recovery に巻き込まれないこと、Python / Docs の狭い deterministic fallback が安定することを確認する。

## 品質層

- L1 Mode Routing: 入力から `typescript-ui`, `python`, `docs`, `answer-only` が妥当に推定される。
- L2 Context Hygiene: mode policy はターンごとの runtime system message と構造化状態で扱い、会話履歴への永続的な分類プロンプトを増やさない。
- L3 Fallback Isolation: TypeScript UI fallback は UI 系の要求だけで発火し、Python / Docs / AnswerOnly では発火しない。
- L4 Deterministic Narrowness: Python / Docs fallback は高信頼の空 workspace ケースだけで発火し、既存ファイルを不用意に置換しない。
- L5 Runtime Quality: 対象成果物の最小実行確認が通り、50 iter 到達や timeout loop がない。

## 実行条件

- モデル: qwen3.5 系と qwen3.6 系の代表モデル。
- 目標: 各シナリオが合格するまで原因調査と修正を繰り返す。
- yes確認が出た場合は `yes` を返して進める。
- 生成物の代表ケースでは該当言語の実行確認を行う。

## Scenario 01: AnswerOnly Does Not Force Repo Edits

既存 README があるディレクトリで実行:

```text
READMEを要約して、現時点の設計上の課題を整理してください。ファイルは変更しないでください。
```

合格条件:

- `work_mode=answer-only` 相当の方針で動く。
- `Write` / `Edit` が発生しない。
- repo edit recovery による「編集を強制する再試行」へ進まない。
- 50 iter 到達がない。
- README の内容は変更されない。

## Scenario 02: Python CSV CLI Mode

空ディレクトリで実行:

```text
PythonでCSVを読み込んでカテゴリ別合計を出すCLIを作って下さい。サンプルCSVと実行手順も含めて下さい。
```

合格条件:

- TypeScript / React / Next.js / Nuxt のファイルが生成されない。
- `analyze_csv.py`, `sample.csv`, `README.md` が生成される。
- `python3 analyze_csv.py sample.csv` が通り、`Grand Total` を出力する。
- 50 iter 到達がない。

## Scenario 03: Docs Mode

空ディレクトリで実行:

```text
READMEを作成してください。目的、使い方、検証方法、注意点を含めて下さい。
```

合格条件:

- TypeScript UI fallback が発火しない。
- `README.md` が生成され、`Purpose`, `Usage`, `Verification`, `Notes` を含む。
- コード生成や package scaffold が発生しない。
- 50 iter 到達がない。

## Scenario 04: TypeScript UI Still Uses Existing Quality Path

空ディレクトリで実行:

```text
Next.jsで売上シミュレーターアプリを作って下さい。単価、数量、割引率を入力し、合計、目標達成判定、グラフ風の進捗表示、保存、エラー表示、起動ポート3011を入れて下さい。
```

合格条件:

- `package.json`, `src/app/layout.tsx`, `src/app/page.tsx`, `scripts/smoke-test.mjs` が生成される。
- `package.json` に `next dev -p 3011`, `build`, `test`, `audit` がある。
- entry file に `Calculated total`, `Target`, `targetMet`, `Math.max`, `Math.min`, `localStorage`, `role="alert"` がある。
- `npm run build` と `npm test` が通る。

## Scenario 05: Existing Code Safe Polish Remains Scoped

Scenario 04 の出力を残し、同じディレクトリで実行:

```text
売上シミュレーターを、入力値の異常値チェックと目標達成判定付きに改善して下さい。既存の画面構成は大きく変えないで下さい。
```

合格条件:

- safe polish fallback が発火する。
- `data-anvil-polish="v1"` が追加される。
- `Calculated total`, `Target`, `Score history`, `localStorage` が残る。
- `npm test` が通る。

## Scenario 06: Non-TypeScript Request Does Not Claim UI Template

空ディレクトリで実行:

```text
Rustの小さなCLIにする方針を検討し、必要なファイル構成案だけ説明してください。ファイルは変更しないでください。
```

合格条件:

- TypeScript UI fallback が発火しない。
- `package.json`, `src/app/page.tsx`, `app.vue` などの UI scaffold が生成されない。
- answer-only/read-only として完了する。

## Scenario 07: Unit Regression

ローカルで実行:

```bash
cargo fmt
cargo test
cargo build --release
```

合格条件:

- 全て成功する。
- 新規 `WorkMode` は旧 session JSON で default 復元できる。
