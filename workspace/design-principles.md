# Anvil 設計方針

## 1. 基本方針

Anvil は、ローカルLLMで日常的に使えるコーディングエージェントを Rust で実装する。  
最優先は「多機能」ではなく、以下の両立である。

- 安定性
- 監査可能性
- 低遅延
- 保守性

20GB弱・最大20万トークン級のローカルモデルを前提とするが、上限性能を前提に設計しない。  
通常の主戦場は 32k〜64k 程度とし、上限付近は例外ケースとして扱う。

## 2. プロダクト原則

### 2.1 ローカルファースト

- Ollama / LM Studio を第一級サポート対象とする
- クラウド依存の前提を持ち込まない
- 外部サービスがなくても主要機能が完結することを重視する

補足:

- 第一級サポート対象とは長期的な正式対応方針を意味する
- Phase 1 の実装必須対象は Ollama を優先し、LM Studio は Phase 2 で同格対応へ拡張する

### 2.2 ツール実行中心

- エージェントは自然言語の応答器ではなく、作業実行器として設計する
- 読む、探す、編集する、実行する、差分を見る、というループを最適化する
- system prompt ではなく state machine と policy で挙動を制御する

### 2.3 Fail Closed

- 壊れた tool call は実行しない
- 曖昧な権限状態では許可しない
- 危険なコマンドは自動拡大解釈しない
- trust boundary を越える入力は常に疑う

## 3. アーキテクチャ原則

### 3.1 Provider 抽象を先に固定する

- Ollama と LM Studio の API 差分は provider 層で吸収する
- agent loop, policy, ui は provider 依存コードを持たない
- NDJSON / SSE の違いは stream normalizer に閉じ込める

### 3.2 文字列ではなく構造化データでつなぐ

- tool call
- permission request
- audit event
- subagent report
- custom slash command

これらはすべて構造化型で扱う。  
自由文や ad-hoc JSON 修復に依存しない。

### 3.3 UI と実行ロジックを分離する

- Claude Code ライクな UI は欲しいが、terminal 制御が core を汚してはならない
- interactive renderer と one-shot renderer は分離する
- UI は event を購読し、agent loop は event を発行する

## 4. コンテキスト管理原則

### 4.1 文脈は資源として扱う

- トークンを使い切る設計にしない
- 大きいログや大きいファイルはそのまま保持しない
- 古い履歴は summary 化する
- 生ログは artifact に逃がし、履歴には参照だけを残す

### 4.2 サブエージェントで文脈を隔離する

- 探索・比較・要約のような膨らみやすい作業はサブエージェントへ送る
- メインセッションには短い report だけを戻す
- ただし、監査に必要な情報は event log に残す

補足:

- サブエージェントは重要な設計要素だが、Phase 1 の実装必須要件ではない
- Phase 1 では subagent を後付け可能な拡張点として設計し、Phase 2 で有効化する
- つまり core は subagent 対応可能に作るが、初期実装は単一エージェントを優先する

### 4.3 コンテキスト上限より性能予算を優先する

- 20万トークンは能力上限であって通常運用目標ではない
- summary / truncation / subagent 起動は、上限到達前に発動する
- 性能劣化は仕様上のバグとして扱う

## 5. セキュリティ原則

### 5.1 Trust Boundary を明示する

以下は信頼できるが、実行権限を直接持たない。

- `ANVIL.md`
- `ANVIL-MEMORY.md`
- カスタム slash command 定義
- ファイル内容
- コマンド出力
- モデル出力

最終的な実行可否は常に policy 層が判定する。

### 5.2 権限は段階的に扱う

- read
- edit
- exec-safe
- exec-sensitive
- exec-dangerous
- subagent-read
- subagent-write

この分類は UI、CLI、subagent、custom command のすべてで共通化する。

補足:

- `PermissionMode` はユーザー向けの簡略プリセットである
- 実際の判定は `PermissionPolicy` と実行コンテキストから導く
- 対話/非対話、hard-confirm、危険コマンド例外はプリセットだけで表現しない
- `hard-confirm` は非対話で自動許可に倒さない

### 5.3 監査できない操作は許可しない

- だれが承認したか
- 何が実行されたか
- どのファイルが変わったか

これが追えない設計は採用しない。

最低限の監査粒度:

- 承認要求と承認結果
- 実行コマンドの argv
- 実行結果の出力参照
- 変更ファイル一覧
- 変更前後の digest
- サブエージェントの親子関係
- schema version

変更検出方針:

- 変更ファイル検出は毎回全ファイル走査を前提にしない
- 優先順位は `tool reported -> targeted snapshot diff -> git diff`
- 大規模リポジトリでは性能劣化を避けるため、影響範囲が推定できる方法を優先する

## 6. 保守性原則

### 6.1 MVP を削る勇気を持つ

- Phase 1 では Ollama + 単一エージェント + 基本ツールに集中する
- LM Studio、custom command、subagent は後段で入れる
- 「作れる」より「壊れにくい」を優先する

### 6.2 単一巨大実装を避ける

- provider
- tools
- policy
- state
- ui
- instructions
- slash

責務ごとに分ける。  
単一ファイルへの集約は避ける。

### 6.3 テストしやすい境界を作る

- HTTP 層はモック可能にする
- tool executor は差し替え可能にする
- parser と executor を分ける
- performance budget は設定値化してテスト対象にする
- permission preset と effective policy を分離し、表駆動テスト可能にする
- audit event の schema 整合性を型で担保する

## 7. 潜在バグを減らすための方針

### 7.1 修復より拒否を優先する

- 壊れた tool call は自動修復しない
- 曖昧な slash command 引数は拒否する
- 不正な permission transition はエラーにする
- event kind と payload kind の不整合を許さない

### 7.2 並行性は制御下に置く

- サブエージェントは session/state を分離する
- append-only audit log は順序保証を意識する
- UI 更新と core state 更新を分離する

### 7.3 破壊的操作は例外扱いにする

- `bypass-permissions` でも hard-confirm を残す
- 危険コマンドは allowlist ではなく専用ルールで扱う
- rollback 可能な場合でも、破壊操作の軽視につなげない

## 8. 拡張方針

### 8.1 追加機能は registry と schema で受ける

- slash commands
- tools
- providers
- memory processors

拡張点は registry 化し、文字列ベースの特殊分岐を増やさない。

補足:

- custom command の引数は型付きで受ける
- audit log payload も schema 化し、自由構造 JSON を乱用しない
- 監査ログは人間可読性より機械検証可能性を優先する
- 監査用の escape hatch は恒久仕様にしない

### 8.2 ローカルLLMの癖は隔離して扱う

- provider ごとの差分
- model ごとの tool call 崩れ
- stream の欠損

これらは adapter 層へ閉じ込め、core に滲ませない。

## 9. 最後の判断基準

設計判断で迷ったら、次の順に優先する。

1. セキュリティ
2. 監査可能性
3. 保守性
4. 性能
5. UI の見た目
6. 機能追加のしやすさ

Anvil は「ローカルLLMでも安心して使えること」が価値であり、「とにかく何でもできること」は価値ではない。
