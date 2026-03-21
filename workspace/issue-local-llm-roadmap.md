## 概要
ローカルLLM特化のコーディングエージェントとしての完成度を上げるため、弱いモデル耐性、長セッション運用、探索効率、安全なローカル実行を強化する改善ロードマップを実装する。

## 背景・動機
現状の Anvil は local-first な設計基盤を持っている一方で、まだ「ローカルLLMでも動く」段階に留まっている。

特に以下がボトルネックになっている。

- ツール呼び出しが主にプロンプト依存で、弱いローカルモデルで崩れやすい
- `file.edit` が strict replace 中心で、編集失敗が起きやすい
- system prompt が大きく、小規模モデルでの推論品質と速度を圧迫する
- コンテキスト管理が末尾詰め込み中心で、長セッションで方針や制約を落としやすい
- retrieval が単純で、大きめの repo で探索効率が落ちる
- `shell.exec` と offline mode がローカル実行安全性の観点でまだ弱い

このままだと、クラウド級の強いモデルではある程度動いても、手元GPUや CPU で回す local model では粘り強さが不足する。

## 提案する解決策
以下のロードマップで段階的に改善する。

### Wave 1: ツール呼び出し成功率と編集成功率を上げる

- `native tool calling > structured JSON/tag > repair fallback` の多層処理を導入する
- provider capability を拡張し、利用可能な backend では native tool calling を優先する
- `file.edit` を strict replace だけでなく patch/anchor 系にも拡張する

### Wave 2: small/local model 向け最適化

- model classifier を能力ベースに拡張する
- system prompt に `full / compact / tiny` tier を導入する
- モデルごとの成功率や失敗傾向に応じて protocol, prompt verbosity, retry 方針を調整する

### Wave 3: 長セッション耐性の改善

- `active task / constraints / touched files / unresolved errors / recent diffs` を保持する構造化 working memory を導入する
- compaction を自然文の会話要約ではなく、状態要約へ寄せる
- turn request 構築時に recency だけでなく状態メモリを優先注入する

### Wave 4: repo 探索効率の改善

- retrieval に symbol/path/keyword の混合スコアリングを導入する
- 変更ファイルや関連テストに対する boost を導入する
- 大規模 repo でも探索の初速を落としにくい設計にする

### Wave 5: ローカル実行安全性の改善

- `shell.exec` を read-only / build-test / general に分離する
- offline mode での実効ポリシーを強化する
- ネットワークや長時間実行に対する制御を明示的に持たせる

### Wave 6: sub-agent の再設計

- sub-agent を固定反復の補助機能ではなく探索専用ワーカーとして再設計する
- 親エージェントに返す payload を構造化する
- main agent と sub-agent の役割分担をローカルモデル前提で見直す

## 受け入れ基準

- [ ] ツール呼び出し処理が multi-tier 化され、backend capability に応じて native / json / tag / repair fallback を選択できる
- [ ] `file.edit` の代替として patch または anchor ベースの編集方式が追加され、小規模ローカルモデルでも編集成功率を上げられる
- [ ] system prompt に small/local model 向け tier が追加され、モデル特性に応じて prompt サイズと詳細度を切り替えられる
- [ ] 長セッション向けに構造化 working memory と状態ベース compaction が導入される
- [ ] retrieval が path/name/content の単純一致中心から改善され、repo 探索効率が向上する
- [ ] `shell.exec` の実行ポリシーが細分化され、offline mode の安全性が強化される
- [ ] sub-agent が探索特化の設計に改善され、親エージェントとの連携が構造化される

## 設計メモ（任意）
優先順は以下を想定する。

1. multi-tier tool protocol
2. file editing 拡張
3. prompt tiering と model adaptation
4. structured working memory / compaction
5. retrieval upgrade
6. shell policy / offline hardening
7. sub-agent redesign

最初のスコープでは README 先行ではなく runtime の実効改善を優先する。
差別化ポイントは「最強モデルで最も賢い」ではなく、「限られたローカル計算資源でも実務に耐える」ことに置く。

## 代替案（任意）

- 強いモデル前提で prompt や parser を増やさず進める
- retrieval や sub-agent を後回しにして UI 改善を優先する

ただし、local-first の競争力を上げるには、UI よりも先に runtime の成功率と長セッション耐性を改善する方が効果が大きい。

## 追加情報（任意）

- 元分析では、Anvil の強みは console clarity と architecture cleanliness にあり、弱みは long-session maturity と large-repo retrieval にある
- 差別化軸は「弱いローカルモデルでも壊れにくい」「長いセッションでも方針を保ちやすい」「ローカル実行の安全策が設計に組み込まれている」こと
