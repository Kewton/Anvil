## 概要
親 Issue #127 の Wave 2 として、small/local model 向けの prompt 最適化とモデル適応を導入する。

## 背景・動機
現状の system prompt は情報量が多く、小規模ローカルモデルでは推論品質と速度の両面で負荷が大きい。
また、プロトコル選択が主にモデル名ヒューリスティックに依存しており、実際の成功率や失敗傾向に基づく最適化がない。

## 提案する解決策

- model classifier を能力ベースに拡張する
- system prompt に `full / compact / tiny` tier を導入する
- モデルごとの成功率や失敗傾向に応じて protocol, prompt verbosity, retry 方針を調整する

## 受け入れ基準

- [ ] system prompt の tier が複数用意され、モデルに応じて切り替えられる
- [ ] model classifier が単純なモデル名判定より広い適応判断を持つ
- [ ] 少なくとも 1 つ以上のモデル適応設定が runtime 内で自動適用される
- [ ] 既存フローを壊さず後方互換性を維持できる
- [ ] テストで prompt tiering と適応ロジックを検証できる

## 設計メモ（任意）

- 主な対象: `src/agent/model_classifier.rs`, `src/agent/mod.rs`, `src/app/mod.rs`
- Wave 1 の多層 tool protocol 導入後に進める前提

## 追加情報（任意）

- Parent: #127
