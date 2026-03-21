## 概要
親 Issue #127 の Wave 1 として、ツール呼び出し成功率と編集成功率を上げる。

## 背景・動機
現状のツール呼び出しは主にプロンプト依存で、弱いローカルモデルでは `ANVIL_TOOL` / `ANVIL_FINAL` 出力が崩れやすい。
また、`file.edit` は strict replace 中心のため、モデルが少しでも文脈をずらすと編集に失敗しやすい。

local-first の実用性を上げるには、まず runtime 側でツール実行と編集失敗を吸収できるようにする必要がある。

## 提案する解決策

- `native tool calling > structured JSON/tag > repair fallback` の多層処理を導入する
- provider capability を拡張し、native tool calling 利用可否を runtime が判断できるようにする
- `file.edit` の代替として patch または anchor ベースの編集方式を追加する
- 失敗時の再試行や repair の扱いを runtime 側で標準化する

## 受け入れ基準

- [ ] provider capability に native tool calling の可否を表現できる
- [ ] agent loop が native / json / tag / repair fallback を選択できる
- [ ] `file.edit` 以外に patch または anchor ベースの編集方式が追加される
- [ ] 既存の strict replace 失敗ケースの一部を新しい編集方式で救済できる
- [ ] 主要フローに対するテストが追加される

## 設計メモ（任意）

- 主な対象: `src/provider/mod.rs`, `src/provider/openai.rs`, `src/agent/mod.rs`, `src/tooling/mod.rs`
- まず成功率を上げることを優先し、UI 改善は対象外とする

## 追加情報（任意）

- Parent: #127
