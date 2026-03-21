## 概要
親 Issue #127 の Wave 5 として、`shell.exec` の実行ポリシー分離と offline mode の安全性強化を行う。

## 背景・動機
現状の offline mode は `web.*` と MCP を止めるが、`shell.exec` を通じたネットワークアクセスは残る。
また、`shell.exec` は汎用 `sh -c` 実行で、用途やリスクに応じた実行クラス分離が弱い。

## 提案する解決策

- `shell.exec` を read-only / build-test / general に分離する
- offline mode での shell policy を強化する
- 長時間実行、ネットワーク、危険操作に対する制御をより明示化する

## 受け入れ基準

- [ ] `shell.exec` に実行クラスまたは同等の区分が追加される
- [ ] offline mode での shell 実行制御が現状より強化される
- [ ] 主要な安全ポリシーに対するテストが追加される
- [ ] 既存の shell 実行 UX を大きく壊さず移行できる
- [ ] 設定またはポリシーの責務分離が整理される

## 設計メモ（任意）

- 主な対象: `src/tooling/mod.rs`, `src/app/policy.rs`, `src/app/mod.rs`
- OS レベル完全遮断は別テーマとしても、runtime 側で実効性を高める

## 追加情報（任意）

- Parent: #127
