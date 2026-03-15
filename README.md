# Anvil

ローカルターミナルで動作するコーディングエージェント。Ollama や OpenAI 互換サーバーを LLM バックエンドとして使用し、ファイル操作やシェルコマンドの実行をエージェント的に行います。

## インストール

```bash
# Rust toolchain (1.85+) が必要
cargo build --release
# バイナリは target/release/anvil に生成されます
```

### 前提条件

- **Ollama**（デフォルトバックエンド）: https://ollama.com
  ```bash
  ollama serve           # サーバー起動
  ollama pull qwen3.5    # モデル取得
  ```
- または **OpenAI 互換 API** サーバー（LM Studio, vLLM 等）

## 基本的な使い方

```bash
# 対話モードで起動
cargo run

# モデルを指定
cargo run -- --model qwen3.5:35b

# 新しいセッションで開始（履歴を引き継がない）
cargo run -- --model qwen3.5:35b --fresh-session

# 承認なしモード（全ツール自動実行）
cargo run -- --model qwen3.5:35b --no-approval

# 非対話モード（パイプ入力向け）
printf 'このリポジトリを分析して\n/exit\n' | cargo run -- --model qwen3.5:35b --no-approval
```

## ツール

Anvil は LLM に以下のツールを提供します:

| ツール | 権限 | 説明 |
|--------|------|------|
| `file.read` | Safe (自動実行) | ファイル読み取り・ディレクトリ一覧 |
| `file.search` | Safe (自動実行) | ファイル名・内容で検索 |
| `file.write` | Confirm (承認必要) | ファイル作成・上書き |
| `shell.exec` | Confirm (承認必要) | シェルコマンド実行 |

### 承認フロー

通常モードでは `file.write` と `shell.exec` の実行前にインラインで確認を求めます:

```
  Allow shell.exec: gog --help? [y/n] y
```

`--no-approval` で起動すると全ツールが承認なしで実行されます。

### 安全性

以下のコマンドは承認モードに関係なくブロックされます:
- `rm -rf /` / `rm -rf ~` (再帰削除)
- `mkfs` (フォーマット)
- `dd if=` (rawディスク書き込み)
- `:(){` (fork bomb)

パスはサンドボックス内に制限され、絶対パス・`..`・シンボリックリンクによる脱出を防止します。

## スラッシュコマンド

| コマンド | 説明 |
|----------|------|
| `/help` | コマンド一覧を表示 |
| `/status` | 現在の状態を表示 |
| `/plan` | 現在のプランを表示 |
| `/plan-add <項目>` | プランに項目を追加 |
| `/plan-focus <番号>` | アクティブなステップを変更 |
| `/plan-clear` | プランをクリア |
| `/checkpoint <メモ>` | チェックポイントを保存 |
| `/repo-find <クエリ>` | リポジトリ内を検索 |
| `/timeline` | セッションのタイムラインを表示 |
| `/compact` | 古い履歴を圧縮 |
| `/model` | 現在のモデル情報 |
| `/provider` | プロバイダー情報 |
| `/approve` | 保留中の承認を許可 |
| `/deny` | 保留中の承認を拒否 |
| `/reset` | Ready 状態に戻す |
| `/exit` | セッション終了 |

## コンテキスト引き継ぎ

### セッション永続化

Anvil はプロジェクトごとにセッションファイル (`.anvil/sessions/<hash>.json`) を自動保存します。次回起動時に同じディレクトリで起動すると、前回の会話履歴が自動的に復元されます。

```bash
# 履歴を引き継いで起動（デフォルト）
cargo run -- --model qwen3.5:35b

# 履歴をリセットして新規セッション
cargo run -- --model qwen3.5:35b --fresh-session
```

### コンテキストウィンドウ管理

LLM に送信するメッセージは**トークンバジェット**で制御されます:

- **バジェット計算**: `context_window / 4`（最小256、最大 `context_window / 2`）
- **メッセージ選択**: 最新のメッセージから逆順にバジェット内に収まるだけ含める
- **自動圧縮**: メッセージ数が閾値（デフォルト64）を超えると古い履歴を要約に圧縮
- **手動圧縮**: `/compact` コマンドで即座に圧縮

つまり、長い会話でも最近のやり取りが優先的に LLM に送信され、古い会話は要約として保持されます。

### 引き継がれる情報

| 情報 | 永続化 | LLM に送信 |
|------|:------:|:----------:|
| ユーザーメッセージ | はい | バジェット内 |
| アシスタント応答 | はい | バジェット内 |
| ツール実行結果 | はい | バジェット内 |
| プラン状態 | はい | スナップショットとして |
| プロバイダーエラー | はい | いいえ |
| 圧縮された古い履歴 | はい（要約） | バジェット内 |

## 設定

### 設定ファイル

`.anvil/config` に `key=value` 形式で記述:

```ini
provider = ollama
model = qwen3.5:35b
provider_url = http://127.0.0.1:11434
context_window = 200000
stream = true
```

### 環境変数

```bash
ANVIL_PROVIDER=ollama
ANVIL_MODEL=qwen3.5:35b
ANVIL_PROVIDER_URL=http://127.0.0.1:11434
ANVIL_CONTEXT_WINDOW=200000
ANVIL_CONTEXT_BUDGET=50000        # 明示的にバジェット指定
ANVIL_MAX_AGENT_ITERATIONS=10     # agenticループの最大反復数
ANVIL_MAX_CONSOLE_MESSAGES=5      # 表示するメッセージ数
ANVIL_AUTO_COMPACT_THRESHOLD=64   # 自動圧縮の閾値
ANVIL_TOOL_RESULT_MAX_CHARS=8000  # ツール結果の最大文字数
ANVIL_CURL_TIMEOUT=300            # curlタイムアウト（秒）
ANVIL_SHELL_TIMEOUT=0             # shell.execタイムアウト（0=無制限）
ANVIL_API_KEY=sk-...              # OpenAI互換APIキー
```

### CLI オプション

```
--provider <名前>           プロバイダー (ollama / openai)
--model <名前>              モデル名
--provider-url <URL>        プロバイダーURL
--context-window <数値>     コンテキストウィンドウサイズ
--context-budget <数値>     トークンバジェット明示指定
--max-iterations <数値>     agenticループ最大反復数
--no-approval               全ツール自動承認
--no-stream                 ストリーミング無効
--fresh-session             新規セッションで開始
--oneshot                   非対話モード
--debug                     デバッグログ有効
--reasoning-visibility <値> 推論表示 (hidden / summary)
```

優先順位: CLI > 環境変数 > 設定ファイル > デフォルト値

## カスタムコマンド

`.anvil/slash-commands.json` でカスタムスラッシュコマンドを定義できます:

```json
{
  "commands": [
    {
      "name": "/review",
      "description": "コードレビューを実行",
      "prompt": "このリポジトリの最近の変更をレビューして、改善点を指摘してください。"
    }
  ]
}
```

## プロバイダー対応

| プロバイダー | 設定 |
|-------------|------|
| Ollama | `--provider ollama --provider-url http://127.0.0.1:11434` (デフォルト) |
| OpenAI互換 | `--provider openai --provider-url http://localhost:1234` |
| API キー認証 | `ANVIL_API_KEY=Bearer sk-...` を設定 |

## ライセンス

MIT
