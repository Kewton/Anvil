# Anvil: ローカルLLM特化コーディングエージェント仕様

## 1. 目的

Anvil は、Rust で実装するローカルLLM特化のコーディングエージェントである。  
対象は Ollama / LM Studio 上で動くローカルモデルで、クラウド依存なしに、コード読解・編集・コマンド実行・進捗提示を行う。

設計上の前提:

- メインモデルは `gpt-5-mini` 相当の実用品質を狙えるローカルモデル
- スループットは `gpt-5-mini` の半分程度を想定
- モデルサイズは 20GB 弱を想定
- コンテキスト長は最大 20 万トークンを想定
- そのため、推論コストの高い無駄な往復、過剰な説明、過大なコンテキスト投入を避ける
- ローカル実行なので、接続不安定・API差分・モデルごとのツール呼び出し癖を吸収する必要がある

## 2. 参考元 `vibe-local` の評価

参照元: `/Users/maenokota/share/work/github_kewton/vibe-local`

### 良いところ

- ローカル完結で依存が少なく、導入障壁が低い
- ツール実行中心のエージェントループが明確
- 権限確認、セッション保存、Plan/Act、Git checkpoint など実運用で必要な機能が一通り揃っている
- Ollama のストリーミングとツール呼び出し差分を吸収しようとしている
- モデル性能差を前提に、温度やサイドカーモデルを切り替える発想がある
- TUI で「考え中」「実行中」を見せるための配慮がある

### 悪いところ

- 単一巨大ファイルに責務が集中し、保守性が低い
- 挙動の多くを巨大な system prompt に依存しており、再現性が弱い
- 機能追加のたびに分岐が増え、モデル差分吸収が場当たり的になっている
- XML tool call fallback など、モデルの崩れを後段で無理に救済している
- TUI、モデル接続、セッション、ツール、権限、Git 操作が密結合
- 「多機能」だが、ローカル20GB級モデル向けには機能過多で文脈コストが重い
- Web 検索や RAG など、コーディングコア体験に対して優先度の低い機能が混ざっている

### 踏襲する点

- ローカル完結
- ツール実行中心のエージェントループ
- 明示的な権限モデル
- ストリーミング前提のUI
- セッション永続化
- Plan と Act の分離
- Git checkpoint / rollback

### 改善する点

- 単一ファイルではなく、責務ごとに Rust crate/module を分離する
- system prompt に寄せすぎず、実行ポリシーをコードで担保する
- プロバイダ差分は `Provider` 抽象で吸収する
- ツール呼び出しは structured tool call を第一にし、fallback は最小限に抑える
- MVP では機能を絞り、20GB級モデルでも破綻しにくい構成にする
- コンテキスト制御を明示設計し、長時間セッションでも劣化しにくくする

## 3. プロダクト方針

Anvil は「高機能な何でも屋」ではなく、ローカルLLMで実用になるコーディング作業を安定提供することを優先する。

優先順位:

1. 安定した編集・実行・確認ループ
2. 低遅延のストリーミング体験
3. 権限・失敗時挙動の明確さ
4. 長時間セッションの劣化抑制
5. 拡張性

非目標:

- 初期段階での Web 検索・RAG・画像解析のフル対応
- 単一プロンプトで何でも解決する設計
- モデル固有ハックに依存した過剰な互換維持

## 4. 必須要件

### 4.1 モデル接続

- Ollama に対応
- LM Studio に対応
- 両者を同じ内部インターフェースで扱う
- チャット補完
- ストリーミング応答
- ツール呼び出し
- モデル一覧取得
- ヘルスチェック

### 4.2 ストリーミング

- テキストの逐次表示
- ツール呼び出しの逐次検出
- キャンセル可能
- ストリーム終了時に usage を取り込めることが望ましい

### 4.3 コーディングエージェント機能

- 対話モード
- ワンショットモード (`-p`)
- セッション保存 / 復元
- ファイル読取
- ファイル編集
- コマンド実行
- Diff 表示
- Plan モード
- Git checkpoint / rollback
- サブエージェント実行

### 4.4 安全性

- ツールごとの権限レベル
- 危険コマンドの追加確認
- 作業ディレクトリ境界の制御
- プロンプトインジェクションをデータとして扱う規則
- 失敗時に同じ危険操作を無限再試行しないこと
- tool call は strict parse を原則とし、曖昧修復した引数では実行しないこと
- 監査ログを残し、誰が何を承認し何を変更したか追跡できること

### 4.5 プロジェクト指示とメモリ

- `ANVIL.md` をプロジェクト指示ファイルとして読み込めること
- `ANVIL-MEMORY.md` をユーザー指摘・出力矯正・思考癖補正の永続メモリとして扱えること
- `ANVIL-MEMORY.md` を更新するスラッシュコマンドを提供すること
- ユーザー定義のカスタムスラッシュコマンドを追加可能にすること
- 指示ファイル、メモリファイル、カスタムコマンド定義ファイルの trust boundary を明示すること

## 5. 対象モデル前提から導く設計制約

20GB弱・`gpt-5-mini`相当・スループット半分程度のモデルでは、以下が重要になる。

- 長文の前置きは不要
- 1ターンでのツール呼び出し回数を抑える
- 不要な履歴を積まない
- 大きいファイルは必要範囲だけ読む
- 失敗時の再試行は回数制限を設ける
- 小タスクを軽量モデルに逃がす設計は有効だが、MVP では必須にしない
- 最大 20 万トークンを使えても、常に埋めるのではなく応答速度優先で使う
- 探索・要約・比較のような汚れやすいタスクはサブエージェントへ逃がし、メイン文脈を清潔に保つ
- 大きいコンテキストは「使える」ことと「常用する」ことを分けて設計する

このため Anvil は、`vibe-local` よりも「少ないツール」「短いプロンプト」「強い実行制約」を採用する。

## 6. 推奨アーキテクチャ

### 6.1 モジュール構成

- `anvil-cli`
  - CLI 引数、起動モード、設定読み込み
- `anvil-core`
  - Agent loop、セッション、Plan/Act、実行ポリシー
- `anvil-models`
  - Provider 抽象、Ollama / LM Studio 実装、ストリーム正規化
- `anvil-tools`
  - Read / Write / Edit / Exec / Diff / Search の各ツール
- `anvil-policy`
  - 権限、パス制約、危険操作判定
- `anvil-state`
  - セッション保存、サマリ、checkpoint metadata
- `anvil-ui`
  - TUI / 非TUI 出力

最初は単一 workspace 内の複数 module で十分。crate 分割は後でもよい。重要なのは責務分離である。

### 6.2 中核インターフェース

```rust
trait ModelProvider {
    async fn health(&self) -> Result<ProviderHealth>;
    async fn list_models(&self) -> Result<Vec<ModelInfo>>;
    async fn chat(&self, req: ChatRequest) -> Result<ChatResponse>;
    async fn chat_stream(&self, req: ChatRequest) -> Result<StreamHandle>;
}
```

```rust
enum StreamEvent {
    TextDelta(String),
    ToolCallDelta(ToolCallDelta),
    ToolCallComplete(ToolCall),
    Usage(Usage),
    Done,
}
```

```rust
trait Tool {
    fn spec(&self) -> ToolSpec;
    async fn execute(&self, call: ToolCall, ctx: &ToolContext) -> ToolResult;
}
```

```rust
trait SubAgentRunner {
    async fn run(&self, task: SubAgentTask) -> Result<SubAgentReport>;
}
```

### 6.3 Provider 実装方針

#### Ollama

- 優先API: `/api/chat`
- 補助API: `/api/tags`, `/api/version`
- ストリーム形式: NDJSON
- keep-alive, context window, temperature などのオプションを扱う

#### LM Studio

- 優先API: OpenAI互換 `/v1/chat/completions`
- 補助API: `/v1/models`
- ストリーム形式: SSE
- OpenAI 互換だが実装差がある前提で、strict ではなく tolerant parser を持つ

### 6.4 ストリーム正規化

Ollama と LM Studio は返却形式が異なるため、内部では `StreamEvent` に正規化する。

- NDJSON/SSE の差分は provider 層で吸収する
- UI と agent loop は provider 差分を知らない
- tool call が分割到着するケースに対応する
- malformed JSON は表示テキストについてのみ限定的に修復してよい
- tool call JSON は strict parse + fail-closed を採用し、修復後の引数で実行しない
- XML fallback は text recovery 用の最終手段に留め、書込み系ツールの発火根拠にしない

## 7. エージェントループ仕様

### 7.1 基本フロー

1. ユーザー入力を受け取る
2. 必要なコンテキストのみ組み立てる
3. モデルへ問い合わせる
4. ストリーム表示しながら tool call を収集する
5. tool call があれば権限確認後に実行する
6. 結果を履歴へ追加して再ループする
7. 最終応答を返す

### 7.2 ループ制御

- 最大反復回数を持つ
- 同一ツールの同一引数連打を検出して停止する
- エラー時は「同じ方法の再試行」ではなく「別手段への切替」を優先する
- 書き込み前に任意で Git checkpoint を切れる

### 7.3 Plan / Act

`vibe-local` の Plan/Act は良い。Anvil でも採用する。ただしより小さく始める。

- Plan モードでは read-only ツールのみ許可
- 計画書は `workspace/` か `.anvil/plans/` に保存
- Act モード移行時に計画書を短く要約して注入
- 計画全文を毎ターン入れ直さない

### 7.4 サブエージェント方針

Anvil は、メインエージェントのコンテキストを汚しやすい作業をサブエージェントへ分離する。

主な用途:

- 広めのコード探索
- 複数ファイル比較
- テスト失敗原因の切り分け
- 実装案の比較検討
- Plan 作成の下調べ
- 長いコマンド出力の要約

基本原則:

- メインエージェントは「指揮」と「最終判断」に集中する
- サブエージェントは「探索」と「中間要約」に集中する
- サブエージェントの生ログ全文はメイン履歴へ入れない
- メインには要約済み report だけを返す

返却形式:

- `summary`
- `key_findings`
- `referenced_files`
- `recommended_next_action`
- 必要なら `artifacts`

制約:

- サブエージェントにも権限層を適用する
- デフォルトでは read-heavy にする
- write/exec を許可する場合も task 単位で明示する
- サブエージェントによる write/exec は、Claude Code のようにユーザーが都度 `yes / no` で選択できること
- 無限にネストさせない
- サブエージェントの実行結果は監査イベントとして main session に必ず記録する
- 少なくとも `task`, `granted_permissions`, `executed_tools`, `changed_files` は残す

## 8. ツール仕様

MVP の built-in tools:

- `Read`
  - テキストを範囲指定で読む
- `Write`
  - 新規作成・全書換え
- `Edit`
  - 部分置換
- `Exec`
  - コマンド実行
- `Glob`
  - ファイル列挙
- `Search`
  - 文字列検索
- `Diff`
  - git diff か内部 diff を表示

初期MVPでは除外または後回し:

- WebSearch
- WebFetch
- Notebook
- RAG
- 画像/PDF解析
- 並列サブエージェント

ただし単一サブエージェントによる探索・要約は早めに導入候補とする。理由は、メインのトークン汚染を抑える効果が大きいため。

理由: ローカル20GB級モデルでは、まずコーディング主ループの安定化が重要だから。

## 9. 権限モデル

3段階で十分:

- `allow`
  - Read, Glob, Search, Diff
- `ask`
  - Write, Edit, Exec
- `deny/default-block`
  - ワークスペース外操作、破壊的コマンド、未知ツール

追加ルール:

- `rm -rf /`, `sudo`, デバイス書込系は常に再確認
- `Exec` は作業ディレクトリと許可パスを明示
- 永続ルールとセッション内一時許可を分ける
- `--yes` があっても危険コマンドは別扱いにする

### 9.1 `permission-mode` 動作表

`permission-mode` は、対話 / 非対話の両方で一貫した実行ポリシーを与える。

対象モード:

- `ask`
- `accept-edits`
- `bypass-permissions`
- 将来拡張: `read-only`

ツールカテゴリ:

- `read`
  - `Read`, `Glob`, `Search`, `Diff`
- `edit`
  - `Write`, `Edit`
- `exec-safe`
  - 非破壊で局所的なコマンド実行
  - 例: `cargo test`, `git status`, `ls`, `rg`
- `exec-sensitive`
  - 状態変更を伴うコマンド実行
  - 例: `git commit`, `cargo add`, `npm install`
- `exec-dangerous`
  - 破壊的または高リスクなコマンド実行
  - 例: `rm`, `sudo`, デバイス書込、権限変更、広範囲削除
- `subagent-read`
  - read-heavy なサブエージェント
- `subagent-write`
  - write/exec を伴うサブエージェント

判定表:

| Category | ask | accept-edits | bypass-permissions | read-only |
|---|---|---|---|---|
| `read` | allow | allow | allow | allow |
| `edit` | ask | allow | allow | deny |
| `exec-safe` | ask | ask | allow | deny |
| `exec-sensitive` | ask | ask | allow or soft-confirm | deny |
| `exec-dangerous` | hard-confirm | hard-confirm | hard-confirm | deny |
| `subagent-read` | ask | ask | allow | deny |
| `subagent-write` | ask | ask | ask or hard-confirm | deny |

補足:

- `hard-confirm` は `bypass-permissions` でも自動許可しない最終確認
- `soft-confirm` は設定で無効化可能な確認
- 非対話環境で `ask` / `hard-confirm` が必要になった場合は、確認不能エラーで失敗させる
- `accept-edits` は名前の通り edit 系に限定して緩和し、 exec は原則緩和しない

### 9.2 コマンド分類ルール

`Exec` は最低限以下の分類器を持つ。

- allowlist ベースの `exec-safe`
- denylist + pattern ベースの `exec-dangerous`
- どちらにも入らないものは `exec-sensitive`

分類例:

- `git status`, `git diff`, `cargo test --lib`, `pytest -q`, `rg foo src/`
  - `exec-safe`
- `git commit -m ...`, `cargo add serde`, `npm install`, `pip install`
  - `exec-sensitive`
- `rm -rf`, `sudo ...`, `chmod -R`, `dd of=/dev/...`, `mkfs`, `git reset --hard`
  - `exec-dangerous`

実装方針:

- 完全自動分類に依存せず、曖昧なものは高リスク側へ倒す
- シェル文字列全体だけでなく argv 単位でも判定する
- custom command や subagent 経由の exec でも同じ分類器を使う

## 10. コンテキスト管理

`vibe-local` の問題の一つは、長セッションでの prompt 肥大化である。Anvil では最初から管理する。

方針:

- システムプロンプトは短く保つ
- ツール結果は全文保存せず、必要なら要約して履歴化
- 大きいコマンド出力は truncate + 保存先参照にする
- 古いターンは rolling summary に圧縮する
- `tool_call` と `tool_result` の対応関係は保持する
- 最大 20 万トークンを上限として扱うが、通常運用では安全余白を持って 60〜70% 程度を目安に制御する
- サブエージェントの結果は「圧縮済み report」として取り込み、生の思考過程や冗長ログは持ち込まない

性能予算:

- 通常ターンの投入文脈は 32k〜64k 程度を主戦場とする
- 96k を超える投入は「大型タスク」とみなし、summary または subagent を検討する
- 128k を超える投入は例外扱いとし、明示理由がない限り避ける
- 20 万トークン近辺は「到達可能な上限」であり、通常運用の目標値ではない
- ツール出力は既定でサイズ上限を持つ
- rolling summary の発火条件はトークン量とターン数の両方で決める

推奨しきい値:

- 1回の tool output を履歴へ残す上限: 2k〜4k tokens
- 直近生履歴の目安: 12〜20 turns
- summary 発火: 40k〜60k tokens または 20 turns 超
- subagent 検討: 読み取り対象が多数ファイル、またはログが長大な場合

推奨:

- 実メッセージ履歴
- 圧縮済みサマリ
- 実ファイル状態は都度 `Read` で取り直す
- 探索系タスクのサブエージェント report

### 10.3 サブエージェントによるコンテキスト隔離

サブエージェントは、メインエージェントのコンテキスト節約のための重要機構として扱う。

設計方針:

- メインは大きな探索を直接抱え込まない
- 一時的に膨らむ文脈はサブエージェント側へ閉じ込める
- サブエージェント終了時に、短い structured report へ圧縮して返す

例:

- 「このリポジトリの構成を調べて」
  - サブエージェントが探索
  - メインには構成要約と重要ファイル一覧だけを返す
- 「テスト失敗の原因を調べて」
  - サブエージェントがログとコードを読む
  - メインには原因候補と修正案だけを返す

避けること:

- サブエージェントの全会話履歴をメインへコピーする
- 毎回些細な作業までサブエージェント化して遅延を増やす
- サブエージェントの未検証推論をそのまま最終回答に使う

補足:

- サブエージェントが破壊的または状態変更を伴う操作を要求した場合、メインUI上で `yes / no` 確認を出す
- この確認はサブエージェント内で閉じず、必ずユーザー可視の承認イベントとして扱う
- 承認後に実行された内容は監査ログへ記録し、要約だけで隠蔽しない

### 10.1 `ANVIL.md`

`CLAUDE.md` 相当のプロジェクト指示ファイルとして `ANVIL.md` を採用する。

役割:

- リポジトリ固有の開発方針
- コード規約
- テスト方針
- 禁止事項
- レビュー観点
- 出力トーンや言語指定

読み込み方針:

- グローバル設定よりプロジェクト設定を優先
- カレントディレクトリから親ディレクトリへ探索し、最も近い `ANVIL.md` ひとつだけを使う
- 必要ならユーザー設定ディレクトリにもグローバル `ANVIL.md` を置けるようにする
- nearest only を採用し、複数階層の `ANVIL.md` はマージしない
- カレントツリー内に `ANVIL.md` がない場合のみ、グローバル `ANVIL.md` を fallback として使う

安全方針:

- `ANVIL.md` の内容は system prompt 直結ではなく、内部 InstructionSet に正規化する
- ツール実行命令や権限昇格命令はそのまま信頼しない
- `ANVIL.md` 内の文言も prompt injection と同様に検証対象とする

trust boundary:

- `ANVIL.md` は「高信頼の人間向け方針ファイル」として扱うが、実行権限を直接与えない
- `ANVIL.md` はツール実行そのものを命令できず、最終的な実行可否は policy 層が判定する
- `ANVIL.md` の内容は構造化した制約に落とし込み、自由文をそのまま実行器へ渡さない

### 10.2 `ANVIL-MEMORY.md`

`ANVIL-MEMORY.md` は、ユーザーからの指摘や矯正を継続的に反映するためのメモリファイルである。

保存対象:

- 思考の癖に対する修正
- 出力フォーマットの修正
- 口調や簡潔さの調整
- ツール実行時の注意点
- そのプロジェクトで繰り返し守るべきルール

保存しないもの:

- 秘密情報
- 一時的な作業メモ
- 長大な会話ログ全文

運用方針:

- 単なる会話履歴ではなく「再利用価値のある恒久指示」に限る
- 項目は短く、重複なく、編集可能な形で保持する
- 読み込み時は全文をそのまま投げず、必要に応じて構造化してシステム指示へ反映する

trust boundary:

- `ANVIL-MEMORY.md` はユーザーの明示指示を保持するための補助記憶であり、権限昇格の根拠にしない
- `ANVIL-MEMORY.md` から shell command や危険操作ポリシーを直接生成しない
- メモリ更新時は秘密情報、トークン、鍵、認証情報らしき文字列を拒否または警告する

推奨フォーマット:

```md
# ANVIL Memory

## Output Preferences
- 回答は簡潔にする
- 実装前に1文で何をするか述べる

## Reasoning Corrections
- 推測で進めず、まずリポジトリを確認する
- テスト失敗時は原因を切り分けてから修正する

## Project Rules
- Rust code は clippy を通す
- 破壊的操作は必ず確認する
```

## 11. UI / UX 方針

`vibe-local` の「ストリーミングで今何をしているか見える」は良い。一方で TUI 実装の複雑さは抑える。

MVP:

- プレーンCLI出力
- ストリーミングテキスト表示
- 現在状態表示: `thinking`, `executing`, `waiting_permission`
- Ctrl+C で中断
- 入力・出力の見た目は Claude Code のUIに近い情報設計を目指す

### 11.1 対話型 UI 方針

対話型起動時の UI は Claude Code に近づける。ただし、完全コピーではなく、ローカル実装として再現性と安定性を優先する。

重視する点:

- 起動時に現在ディレクトリ、モデル、サンドボックス/権限状態が一目で分かる
- ユーザー入力とアシスタント出力の区切りが明確
- ストリーミング中に「何をしているか」が分かる
- ツール呼び出し、承認待ち、実行結果が視認しやすい
- セッション継続感がある

Claude Code ライクに寄せる要素:

- シンプルなヘッダ
- 明確なプロンプト記号
- 実行中ステータス表示
- ツール呼び出しのコンパクト表示
- 権限確認の対話導線
- `/help` でスラッシュコマンド一覧

避ける点:

- 過度に複雑なターミナル制御
- 環境依存の強い装飾
- UI 実装のために本体ロジックが複雑化すること

### 11.2 CLI モード

CLI は少なくとも 2 モードを提供する。

- 対話モード
  - `anvil`
- ワンショットモード
  - `anvil -p "RustでCLIの骨組みを作って"`

権限モード引数を持つ:

- `--permission-mode ask`
- `--permission-mode accept-edits`
- `--permission-mode bypass-permissions`
- `read-only` は将来予約のモード名とし、初期実装の必須対象には含めない

`-p` の期待挙動:

- 非対話で 1 リクエストを処理して終了する
- 必要ならツール実行まで完了する
- 権限が必要な場合の扱いは `--permission-mode` に従う
- 非対話環境でも、指定された権限モードに応じて一貫した挙動を取る
- 出力は人間が読みやすい簡潔な形式にする

権限モードの想定:

- `ask`
  - 対話環境では都度確認
  - 非対話環境では確認不能な操作を失敗として返す
- `accept-edits`
  - ファイル編集系は自動許可
  - より危険な exec / 破壊的操作は確認または拒否
- `bypass-permissions`
  - ほぼ全操作を許可
  - ただし明らかな破壊操作は別扱いにできる余地を残す

非対話補足:

- 非対話時の `ask` / `soft-confirm` / `hard-confirm` の扱いは execution context で明示する
- 既定では `ask` と `hard-confirm` は拒否する
- `bypass-permissions` でも `exec-dangerous` は自動許可しない
- `hard-confirm` は非対話で `allow` に変換しない

推奨デフォルト:

- 対話起動: `ask`
- `-p` 非対話: `ask`
- CI/自動化: 明示指定必須を推奨

Phase 2:

- footer 固定TUI
- type-ahead
- リッチ diff 表示

重要なのは、凝った terminal hack よりも安定性である。

### 11.3 スラッシュコマンド

組み込みスラッシュコマンドとして最低限以下を持つ。

- `/help`
- `/status`
- `/model`
- `/plan`
- `/act`
- `/checkpoint`
- `/rollback`
- `/memory`
- `/memory add <text>`
- `/memory edit`
- `/memory show`

`/memory` 系の期待挙動:

- `/memory add <text>`
  - 指摘内容を `ANVIL-MEMORY.md` に追記または統合する
- `/memory edit`
  - エディタまたは内部編集フローで `ANVIL-MEMORY.md` を修正する
- `/memory show`
  - 現在のメモリ内容を表示する

実装方針:

- モデルに「メモリ更新文」を書かせることはできるが、最終的な保存形式はアプリ側で正規化する
- 重複排除、見出し整理、禁止項目チェックはコード側で実施する
- メモリ更新はユーザー明示コマンドまたは明示許可時のみ行う

### 11.4 カスタムスラッシュコマンド

Anvil はユーザー定義のカスタムスラッシュコマンドを追加可能にする。

目的:

- よく使うプロンプトのテンプレート化
- プロジェクト固有ワークフローのショートカット化
- 複数ツール操作の定型化

定義例:

- `.anvil/commands/*.md`
- `.anvil/commands/*.toml`
- ユーザー設定ディレクトリの `commands/`

最低限必要な項目:

- コマンド名
- 説明
- 実行方式
  - プロンプト展開
  - 内部アクション呼び出し
  - 複合フロー起動
- 引数定義

実行例:

- `/review-pr`
- `/fix-test test_name`
- `/summarize-diff`

設計方針:

- 組み込みコマンドとユーザー定義コマンドを同じ registry で扱う
- 衝突時は組み込み優先、または明示設定で override 可とする
- 危険な内部アクションに直接触れるコマンドは権限層を通す
- 単なる文字列展開ではなく、将来は structured command spec へ移行しやすい形にする

安全性・保守性方針:

- MVP では `.md` ベースの自由文展開を避け、構造化定義を優先する
- 推奨フォーマットは `.toml` 等の schema 付き定義とする
- 引数は名前付き・型付きで扱い、単純な文字列連結で shell command を作らない
- 実行時は validation を通し、未定義引数や余剰引数は拒否する
- custom command も監査ログ対象とする

MVP 制約:

- Phase 1 では組み込みコマンドのみ
- Phase 2 で schema 付き custom command を導入
- 自由文テンプレート展開は Phase 3 以降に再評価する

## 12. Rust 実装方針

推奨ライブラリ:

- `tokio`
- `reqwest`
- `serde`, `serde_json`
- `clap`
- `thiserror`
- `tracing`, `tracing-subscriber`
- `crossterm` または `ratatui` は Phase 2
- `toml` または `serde_yaml` は slash command 定義用に検討可

実装ルール:

- すべて async ベース
- provider ごとのレスポンス差分は enum ではなく内部共通型へ即時変換
- テスト可能性を優先し、HTTP クライアントとツール実行は抽象化する
- prompt ではなく state machine で制御する
- `ANVIL.md` / `ANVIL-MEMORY.md` / custom commands は独立 loader で管理する
- UI は対話モードとワンショットモードで renderer を分離する
- サブエージェントは main session とは別 session/state を持たせる
- main session に戻す情報は report schema に制限する
- 監査ログは state とは別の append-only event log として持つ
- tool call parser は text parser と分離し、実行可能イベント生成前に schema validation を行う
- performance budget を設定値として持ち、トークン閾値をテスト可能にする

### 12.1 監査ログ event schema

監査ログは append-only の JSONL を基本とする。

目的:

- 承認イベントの追跡
- ツール実行の追跡
- サブエージェント実行の追跡
- 変更ファイルの追跡
- 障害解析と再現補助

ファイル例:

- `.anvil/state/audit.log.jsonl`

共通フィールド:

```json
{
  "event_id": "evt_01H...",
  "ts": "2026-03-11T00:00:00Z",
  "session_id": "sess_...",
  "turn_id": "turn_...",
  "event_type": "tool_execution",
  "actor": "main_agent",
  "source": "interactive",
  "cwd": "/abs/path",
  "data": {}
}
```

共通ルール:

- `event_id` は一意
- `schema_version` を持ち、後方互換を扱えるようにする
- `ts` は RFC3339 UTC
- `session_id` は main / subagent を識別可能
- `turn_id` は同一ターン内イベント関連付けに使う
- `actor` は `user`, `main_agent`, `subagent`, `system`
- `source` は `interactive`, `one_shot`, `slash_command`, `replay` など
- `data` は event_type ごとの payload

必須 event_type:

- `session_started`
- `session_ended`
- `permission_requested`
- `permission_resolved`
- `tool_call_received`
- `tool_execution`
- `tool_result`
- `tool_blocked`
- `subagent_started`
- `subagent_permission_requested`
- `subagent_permission_resolved`
- `subagent_finished`
- `memory_updated`
- `custom_command_invoked`
- `error`

#### `permission_requested`

```json
{
  "event_type": "permission_requested",
  "actor": "main_agent",
  "data": {
    "permission_mode": "ask",
    "category": "exec-sensitive",
    "target": "git commit -m ...",
    "reason": "state-changing command",
    "request_id": "perm_..."
  }
}
```

#### `permission_resolved`

```json
{
  "event_type": "permission_resolved",
  "actor": "user",
  "data": {
    "request_id": "perm_...",
    "decision": "allow",
    "scope": "once",
    "applies_to": "git commit -m ..."
  }
}
```

#### `tool_execution`

```json
{
  "event_type": "tool_execution",
  "actor": "main_agent",
  "data": {
    "tool_name": "Exec",
    "tool_call_id": "call_...",
    "category": "exec-safe",
    "args": {
      "cmd": "cargo test"
    }
  }
}
```

#### `tool_result`

```json
{
  "event_type": "tool_result",
  "actor": "system",
  "data": {
    "tool_call_id": "call_...",
    "status": "ok",
    "exit_code": 0,
    "duration_ms": 1842,
    "output_ref": ".anvil/state/artifacts/out_123.txt",
    "changed_files": []
  }
}
```

#### `subagent_started`

```json
{
  "event_type": "subagent_started",
  "actor": "main_agent",
  "data": {
    "subagent_id": "sub_...",
    "task": "テスト失敗原因の調査",
    "granted_permissions": ["read"],
    "input_summary": "tests failed after edit"
  }
}
```

#### `subagent_finished`

```json
{
  "event_type": "subagent_finished",
  "actor": "subagent",
  "data": {
    "subagent_id": "sub_...",
    "executed_tools": ["Read", "Search", "Exec"],
    "changed_files": [],
    "report_summary": "config mismatch found",
    "report_ref": ".anvil/state/artifacts/subagent-report-1.json"
  }
}
```

#### `memory_updated`

```json
{
  "event_type": "memory_updated",
  "actor": "user",
  "data": {
    "memory_file": "ANVIL-MEMORY.md",
    "operation": "add",
    "summary": "回答を簡潔にするルールを追加"
  }
}
```

#### `custom_command_invoked`

```json
{
  "event_type": "custom_command_invoked",
  "actor": "user",
  "data": {
    "command_name": "/review-pr",
    "command_source": ".anvil/commands/review-pr.toml",
    "resolved_args": {
      "target": "HEAD~1..HEAD"
    }
  }
}
```

実装ルール:

- stdout/stderr 全文は event 本体に埋め込まず artifact file に退避する
- 秘密情報が含まれる可能性のある payload は redaction を通す
- audit log 自体はユーザー編集対象ではなく system-managed にする
- main session と subagent session の関係が追えるよう parent_session_id を持たせてもよい

## 13. 推奨ディレクトリ構成

```text
src/
  main.rs
  cli/
  config/
  agent/
  instructions/
  slash/
  models/
    mod.rs
    provider.rs
    ollama.rs
    lm_studio.rs
    stream.rs
  tools/
    mod.rs
    read.rs
    write.rs
    edit.rs
    exec.rs
    glob.rs
    search.rs
    diff.rs
  policy/
  state/
  git/
  ui/
tests/
workspace/
```

## 14. フェーズ分割

### Phase 1: MVP

- Ollama 対応
- Claude Code ライクな対話UIの基本形
- `-p` ワンショット実行
- ストリーミング
- Read / Write / Edit / Exec / Glob / Search / Diff
- 権限確認
- `--permission-mode` 対応
- セッション保存
- プレーンCLI
- `ANVIL.md` 読み込み
- `ANVIL-MEMORY.md` 読み込みと `/memory add`
- 組み込み slash command registry
- strict tool-call parsing
- append-only 監査ログ

注記:

- Phase 1 はスコープを抑えるため、LM Studio 対応、custom slash command、subagent 実行は含めない
- まずは Ollama + 単一エージェント + 安全なツール実行ループを固める

### Phase 2: 実用化

- LM Studio 対応
- Plan / Act
- Git checkpoint / rollback
- コンテキスト圧縮
- モデル自動選択
- エラー回復強化
- `/memory edit`, `/memory show`
- schema 付きカスタムスラッシュコマンド読み込み
- 単一サブエージェントによる探索・要約
- report 経由の文脈圧縮連携

### Phase 3: 拡張

- サイドカーモデル
- TUI 改善
- Notebook / Web / RAG
- 並列サブエージェント
- メモリの自動整理支援
- slash command の structured workflow 化

## 15. 受け入れ基準

- Ollama / LM Studio の両方で同じ CLI から会話できる
- 両方でストリーミング表示できる
- 両方で少なくとも基本的な tool calling が動く
- `anvil` で対話起動でき、UI が Claude Code に近い操作感を持つ
- `anvil -p "..."` で非対話実行できる
- `--permission-mode` で権限挙動を切り替えられる
- Read → Edit/Write → Exec → Diff の一連作業が完走できる
- 権限確認が破綻しない
- 長めのセッションでも応答不能になりにくい
- `ANVIL.md` の指示が反映される
- `ANVIL-MEMORY.md` を `/memory add` で更新できる
- 探索系タスクをサブエージェントへ逃がし、メインセッションのコンテキスト増加を抑制できる
- ローカル20GB級モデルで「待てば使える」ではなく「日常作業に耐える」体感を出せる
- 壊れた tool call を誤実行せず fail-closed で停止できる
- 監査ログから承認イベント、実行イベント、変更ファイルを追跡できる

Phase 2 受け入れ基準の追加:

- LM Studio でも同等の基本操作が動く
- schema 付き custom slash command を追加して認識できる
- サブエージェントの承認と実行履歴を監査できる

## 16. 結論

Anvil は `vibe-local` の「ローカル完結」「ツール中心」「ストリーミング重視」という強みを継承しつつ、以下を明確に改善するべきである。

- 巨大単一実装からの脱却
- プロンプト依存の縮小
- provider 差分の明示的抽象化
- 20GB級ローカルモデル向けの軽量設計
- コーディング体験に集中した MVP 設計

要するに、`vibe-local` をそのまま Rust へ移植するのではなく、思想を継承しつつ構造を作り直すのが正しい。
