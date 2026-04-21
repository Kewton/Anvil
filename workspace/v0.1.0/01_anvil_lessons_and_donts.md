# Anvil から得た教訓と、今後のべからず集

作成日: 2026-04-16
対象: `anvil` のローカルLLM向け実装と、2026-04-16 の実運用テスト結果

参照:
- `workspace/orchestration/runs/2026-04-16/plan-local-bootstrap-phase.md`
- `workspace/orchestration/runs/2026-04-16/summary-local-bootstrap-phase.md`
- `/Users/maenokota/share/work/localwork/test0416_02_a/artifacts-short/aggregate-summary.json`
- `/Users/maenokota/share/work/localwork/test0416_02_a/artifacts-short/run1-anvildev.log`

## 1. 事実整理

直近の外部5-runテストでは、以下が再現した。

- 5/5 run で Next.js scaffold 自体は成功した
- 5/5 run で `page.tsx` / `layout.tsx` / `globals.css` は存在した
- 5/5 run で `npm run dev -- --port 3011` は起動した
- 5/5 run で `anvildev` はタイムアウト終了した
- 5/5 run で `page.tsx` は初期テンプレートのままで、Space Invaders 実装に未到達だった
- run 代表ログでは scaffold 後に `tool_calls=0`, `streamed_chars=0` の follow-up が続き、`local bootstrap lock: empty no-tool response` が繰り返された

つまり、Anvil は「ローカルLLMで scaffold させる」ところまでは持っていけたが、「scaffold 後にアプリ実装を継続させる」ことに失敗した。

## 2. 学んだこと

### 2.1 テストが通っても、実利用の失敗は隠せない

- targeted regression harness は通っていた
- しかし実運用の5-runでは、ユーザー要求の本丸である「ゲーム実装」に1回も到達しなかった

教訓:

- ローカルLLM向けでは、内部状態機械の単体/統合テストより、実プロンプトE2Eの比重をもっと上げるべき
- 「起動する」「ファイルがある」だけでは成功ではない
- 実アプリの意味的完成度を見ないと、誤った安定化に投資し続ける

### 2.2 scaffold 後リカバリに複雑な状態機械を足し続けるのは悪手

- `PreBootstrap` / `PostBootstrap`
- required-target tracking
- phase-aware completion/recovery
- target-specific tool catalog
- plan drift / shell drift / final gate 抑止

これらは個別には妥当でも、ローカルLLMには制御面が重すぎた。

教訓:

- 弱いモデルに対して、後段リカバリの複雑さを増すほど成功率が上がるとは限らない
- 「失敗した後にどう矯正するか」より、「最初から失敗しにくい形にするか」のほうが重要
- ローカルLLMには、分岐数より一本道の実行経路が効く

### 2.3 ローカルLLMは「曖昧な follow-up」を嫌う

実ログでは scaffold 後に no-tool response が続き、tool 実行に復帰できなかった。

教訓:

- ローカルLLMは「続きやって」「今度は mutate して」のような follow-up に弱い
- 長い履歴の上で phase だけ切り替える設計は、文脈が濁りやすい
- scaffold と実装を同じ大きな会話の中で扱うと、初期テンプレートに吸い戻される

### 2.4 ローカルLLM向けに多機能すぎた

現 Anvil は以下の責務が多い。

- provider abstraction
- completion / final gate
- phase estimator
- termination semantics
- multiple tool protocols
- MCP
- hooks
- retrieval
- skills
- telemetry
- sub-agent / worker routing

教訓:

- 「全部入りの汎用エージェント」をローカルLLMに最適化するのは難しい
- ローカルLLM専用版は、目的機能だけを最短経路で持つべき
- 特に v0.1.0 では、拡張性より再現性を優先すべき

### 2.5 completion 判定はファイル存在ではなく意味で見るべき

今回の実運用では、必要ファイルが揃って起動も成功したが、中身はテンプレートだった。

教訓:

- `page.tsx` が存在することは成功条件ではない
- `globals.css` が書き換わったことも成功条件ではない
- 生成物が「依頼に対して意味的に合っているか」を必ず見る

### 2.6 ローカルLLMでは prompt と protocol を減らすほうが強い

Anvil は JSON tool protocol, tag protocol, native tool calling, follow-up prompt shortening など多層化していた。

教訓:

- protocol が複数あると、ローカルLLMでの失敗モードも複数になる
- fallback を増やすほど、全体の理解負荷も増える
- MVP は protocol を絞るべき

## 3. べからず集

### 3.1 設計べからず

- ローカルLLM向けMVPで、汎用クラウド向け抽象化を先に作ってはいけない
- scaffold 後の矯正ロジックを何段も積み上げてはいけない
- state machine を後付けで増築してはいけない
- protocol を複数抱えたまま MVP を進めてはいけない
- E2E で落ちているのに unit/integration の通過だけで安心してはいけない

### 3.2 プロンプトべからず

- 長い system prompt に役割を詰め込みすぎてはいけない
- Plan / Act / bootstrap / repair / completion を同じ巨大プロンプトで面倒見てはいけない
- 「説明するな、plan するな、これをしろ」を後段で何度も注入してはいけない
- 履歴を保持したまま、期待行動だけを切り替えてはいけない

### 3.3 実行制御べからず

- no-tool response を長時間ループさせてはいけない
- 実装未達なのに server 起動だけで成功扱いしてはいけない
- required file の存在を completion とみなしてはいけない
- 1 run の中で「作る」「直す」「起動する」を全部 recover しようとしてはいけない

### 3.4 プロダクトべからず

- v0.1.0 から MCP / hooks / retrieval / advanced delegation を前提にしてはいけない
- まず成功すべきコア経路より先に拡張機構を移植してはいけない
- 「既存 Anvil の資産を温存すること」を最優先にしてはいけない

## 4. 次版に持ち込むべき原則

- ローカルLLM専用としてゼロベースで作る
- scaffold と実装を分離しすぎず、最短で app コード生成に入る
- まずは少数ツールで成功率を作る
- 会話/状態/プロトコルは最小化する
- 実プロンプトE2Eを主テストに据える
- 「起動した」ではなく「依頼どおりのものができた」を成功条件にする

## 5. 結論

Anvil から得た最大の教訓は、ローカルLLM向けに「正しさを制御ロジックで回収する」方向は限界があるということだった。

次の v0.1.0 では、Anvil の部分修正ではなく、

- 構造を小さくする
- 実行経路を減らす
- ローカルLLMが素直に従いやすい形に寄せる
- 実E2Eで改善を確認する

この4点を最優先にする。
