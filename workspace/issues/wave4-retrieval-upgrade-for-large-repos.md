## 概要
親 Issue #127 の Wave 4 として、大きめの repo でも探索初速を落としにくい retrieval 改善を行う。

## 背景・動機
現状の retrieval は path/name/content の単純一致中心で、repo が大きくなるほど探索効率が落ちやすい。
local model 自身に探索方針を任せる割合が高く、無駄な `file.read` や `file.search` が増えやすい。

## 提案する解決策

- retrieval に symbol/path/keyword の混合スコアリングを導入する
- 変更ファイル、関連テスト、近傍ファイルに対する boost を導入する
- 大規模 repo でも探索の初速を確保できるように index / ranking を改善する

## 受け入れ基準

- [ ] retrieval のスコアリングが単純一致より改善される
- [ ] 変更ファイルまたは関連テストへの boost が導入される
- [ ] 既存の repo-find 系フローを壊さない
- [ ] retrieval の品質を確認するテストが追加される
- [ ] 大規模 repo を意識した設計メモまたは制約が整理される

## 設計メモ（任意）

- 主な対象: `src/retrieval/mod.rs`
- embeddings 導入は必須条件ではなく、まずは lightweight な ranking 改善を優先する

## 追加情報（任意）

- Parent: #127
