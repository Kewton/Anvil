# Anvil UAT Scenarios v4

目的: v3で安定した runnable UI fallback を、キーワード固定ではなく抽象的な機能プリミティブと品質層で検証する。LLM が詰まるケースは deterministic fallback で救うが、既存コードの意味変更を奪いすぎないことも確認する。

## 品質層

- L1 Structure: 対象フレームワークの entry file と package scripts が生成される。
- L2 Runnable: `npm install`, `npm run build`, `npm test` が通る。
- L3 Interaction: 入力、ボタン、状態、エラー表示、履歴が存在する。
- L4 Functional primitives: validation, calculation, target decision, visualization, persistence, accessibility のうち要求されたものが実装される。
- L5 Existing-code fit: 既存コード改善では、低品質 placeholder 以外を丸ごと置換しない。fast polish fallback は安全な品質改善語彙だけで発火する。

## 実行条件

- モデル: qwen3.5 系と qwen3.6 系。
- 目標: 各モデルで全シナリオ合格を2連続以上。失敗時は原因を分類し、実装修正後に再実行する。
- yes確認が出た場合は `yes` を返して進める。
- 生成物の代表ケースでは `npm install`, `npm run build`, `npm test` を実行する。

## Scenario 01: Semantic Business App

空ディレクトリで実行:

```text
Next.jsで売上シミュレーターアプリを作って下さい。単価、数量、割引率を入力し、合計、目標達成判定、グラフ風の進捗表示、保存、エラー表示、キーボード操作、起動ポート3011を入れて下さい。
```

合格条件:

- `package.json`, `src/app/layout.tsx`, `src/app/page.tsx`, `scripts/smoke-test.mjs` が存在する。
- `package.json` に `build`, `test`, `audit`, `dev -p 3011` がある。
- entry file に `Calculated total`, `Target`, `targetMet`, `Math.max`, `Math.min`, `localStorage`, `aria-live`, `role="alert"` がある。
- `npm run build` と `npm test` が通る。

## Scenario 02: Cross Framework Primitive Coverage

空ディレクトリで実行:

```text
Nuxt.jsで予約フォームアプリを作って下さい。一覧、入力チェック、保存、目標件数の進捗表示、復元、スクリーンリーダー向け状態通知、起動ポート3012を入れて下さい。
```

合格条件:

- `app.vue`, `nuxt.config.ts`, `scripts/smoke-test.mjs`, `package.json` が存在する。
- `package.json` に `nuxt dev -p 3012`, `test`, `audit` がある。
- `app.vue` に `Calculated total`, `Target`, `targetMet`, `Math.max`, `Math.min`, `localStorage`, `aria-live`, `role="alert"` がある。
- `npm run build` と `npm test` が通る。

## Scenario 03: Existing Code Safe Polish

まず Scenario 01 の出力を残し、同じディレクトリで実行:

```text
1つ目の売上シミュレーターを、入力値の異常値チェックと目標達成判定付きに改善して下さい。既存の画面構成は大きく変えないで下さい。
```

合格条件:

- fast polish fallback が iter 1 近辺で完了する。
- entry file に `data-anvil-polish="v1"` が追加される。
- 既存の `Calculated total`, `Target`, `Score history`, `localStorage` が残る。
- `npm test` が通る。

## Scenario 04: Existing Code Non-safe Request

既存の React/Vue/Next アプリがあるディレクトリで実行:

```text
既存の認証フローをOAuth連携に置き換えて下さい。
```

合格条件:

- deterministic polish auto-plan bypass が発火しない。
- generic practice template に丸ごと置換されない。
- LLM/通常編集フローへ進むか、実装不能なら理由を返す。

## Scenario 05: Game Smoke Consistency

空ディレクトリで実行:

```text
React.jsでブロック崩しゲームを作って下さい。キーボード操作、リアルタイム描画、Restart、起動ポート3013を入れて下さい。
```

合格条件:

- `scripts/smoke-test.mjs` と `npm test` が生成される。
- game smoke が `canvas`, `requestAnimationFrame`, `addEventListener`, `Restart` を確認する。
- `npm run build` と `npm test` が通る。

## Scenario 06: Dependency Visibility

Scenario 01, 02, 05 の生成 `package.json` を確認する。

合格条件:

- dependency は固定 version で、`latest` や range-only にしない。
- `audit` script が存在し、critical threshold で実行可能。
- audit warning があっても、CI相当の `npm test` と `npm run build` は品質判定の主軸として維持する。

## Scenario 07: Regression Matrix

既存 v3 の代表ケースを各1回ずつ再実行する。

合格条件:

- app create, app improve, game create の代表ケースが qwen3.5 / qwen3.6 の両方で合格する。
- panic, timeout loop, 50 iter到達がない。
- 代表生成物の `npm test` が通る。
