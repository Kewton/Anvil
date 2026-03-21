## 概要
親 Issue #127 の Wave 3 として、長セッション耐性を上げるための構造化 working memory と状態ベース compaction を導入する。

## 背景・動機
現状のコンテキスト構築は末尾のメッセージ優先で、長いセッションになるほど目的、制約、保留課題、最近の変更が落ちやすい。
compaction も自然文の会話要約寄りで、実行状態の保持には最適化されていない。

## 提案する解決策

- `active task / constraints / touched files / unresolved errors / recent diffs` を保持する構造化 working memory を導入する
- compaction を会話要約から状態要約へ寄せる
- turn request 構築時に recency だけでなく状態メモリを優先注入する

## 受け入れ基準

- [ ] session に構造化 working memory を保持できる
- [ ] turn request 構築時に working memory をプロンプトへ反映できる
- [ ] compaction が状態情報を落としにくい設計へ更新される
- [ ] 長セッション系テストが追加または拡張される
- [ ] 既存の resume フローと整合する

## 設計メモ（任意）

- 主な対象: `src/session/mod.rs`, `src/agent/mod.rs`, `src/app/mod.rs`
- working memory は会話履歴の代替ではなく補助レイヤーとして設計する

## 追加情報（任意）

- Parent: #127
