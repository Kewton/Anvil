# Anvil

ローカルターミナルで動作するコーディングエージェント。Ollama や OpenAI 互換サーバーを LLM バックエンドとして使用し、ファイル操作やシェルコマンドの実行をエージェント的に行います。

## クイックスタート

### 1. バイナリのインストール

[GitHub Releases](https://github.com/Kewton/Anvil/releases) からビルド済みバイナリをダウンロード:

```bash
# macOS (Apple Silicon)
curl -L https://github.com/Kewton/Anvil/releases/download/v0.0.2/anvil-darwin-arm64.gz -o anvil.gz
gunzip anvil.gz
chmod +x anvil
sudo mv anvil /usr/local/bin/

# インストール確認
anvil --help
```

### 2. LLM バックエンドの準備

Anvil は LLM の推論を外部サーバーに委託します。以下のいずれかを用意してください。

#### Ollama（推奨・無料）

```bash
# インストール: https://ollama.com
ollama serve                     # サーバー起動
ollama pull qwen3.5:latest       # モデル取得（例）
```

#### OpenAI 互換 API（LM Studio, vLLM 等）

```bash
# LM Studio 等でサーバーを起動し、URL とモデル名を指定
anvil --provider openai --provider-url http://localhost:1234 --model your-model
```

### 3. 起動

```bash
# プロジェクトのディレクトリで起動
cd /path/to/your/project
anvil --model qwen3.5:35b
```

起動すると対話プロンプトが表示されます:

```
    ___              _ __
   /   |  ____ _   _(_) /_
  / /| | / __ \ | / / / __/
 / ___ |/ / / / |/ / / /_
/_/  |_/_/ /_/|___/_/\__/

  local coding agent for serious terminal work

  Model   : qwen3.5:35b
  Context : 200k
  Mode    : local / confirm

  [U] you >
```

## 使い方

### 基本操作

```bash
# 対話モードで起動（前回のセッションを自動復元）
anvil --model qwen3.5:35b

# 新しいセッションで開始（履歴をリセット）
anvil --model qwen3.5:35b --fresh-session

# 全ツール自動承認モード（承認プロンプトをスキップ）
anvil --model qwen3.5:35b --no-approval

# 非対話モード（パイプ入力・スクリプト向け）
echo "src/main.rsを読んで要約して" | anvil --model qwen3.5:35b --no-approval --oneshot
```

### 対話例

```
[U] you > このプロジェクトの構造を教えて

  $ ls -la                              ← shell.exec がリアルタイムで実行される
  Cargo.toml  README.md  src/  tests/
  ...

[A] anvil > このプロジェクトは Rust で構築されており...

[U] you > src/main.rs にエラーハンドリングを追加して

  Allow file.write: src/main.rs? [y/n] y    ← ファイル変更前に承認を求める

[A] anvil > エラーハンドリングを追加しました。変更内容は...

[U] you > /exit
```

### 承認フロー

通常モードでは、ファイル書き込み (`file.write`) とシェルコマンド (`shell.exec`) の実行前にインラインで確認を求めます:

```
  Allow shell.exec: npm test? [y/n]
```

| 入力 | 動作 |
|------|------|
| `y` / `yes` | ツールを実行 |
| `n` / その他 | 拒否（LLMに「denied by user」として通知） |

`--no-approval` で起動すると全ツールが承認なしで実行されます。

### ツール一覧

Anvil は LLM に以下のツールを提供します:

| ツール | 権限 | 説明 |
|--------|------|------|
| `file.read` | Safe (自動実行) | ファイル読み取り・ディレクトリ一覧 |
| `file.search` | Safe (自動実行) | ファイル名・内容で検索 |
| `file.write` | Confirm (承認必要) | ファイル作成・上書き |
| `shell.exec` | Confirm (承認必要) | シェルコマンド実行（出力はリアルタイム表示） |

### スラッシュコマンド

セッション中に `/` で始まるコマンドを入力できます:

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
| `/reset` | Ready 状態に戻す |
| `/exit` | セッション終了 |

### 安全性

以下のコマンドは承認モードに関係なくブロックされます:
- `rm -rf /` / `rm -rf ~` (再帰削除)
- `mkfs` (フォーマット)
- `dd if=` (rawディスク書き込み)
- `:(){` (fork bomb)

パスはサンドボックス内に制限され、絶対パス・`..`・シンボリックリンクによる脱出を防止します。

## セッションと履歴

### 自動保存・復元

Anvil はプロジェクトディレクトリごとにセッションファイル (`.anvil/sessions/`) を自動保存します。同じディレクトリで再起動すると前回の会話が自動復元されます。

```bash
# セッション復元（デフォルト）
anvil --model qwen3.5:35b

# 新しいセッションで開始
anvil --model qwen3.5:35b --fresh-session
```

### コンテキストウィンドウ管理

LLM に送信するメッセージは**トークンバジェット**で自動制御されます:

- 最新のメッセージから優先的にバジェット内に収まる分だけ送信
- メッセージ数が閾値（デフォルト64）を超えると古い履歴を自動要約
- `/compact` コマンドで手動圧縮も可能

長い会話でも最近のやり取りが常に優先され、古い会話は要約として保持されます。

## 設定

### 設定ファイル

プロジェクトルートの `.anvil/config` に `key=value` 形式で記述:

```ini
provider = ollama
model = qwen3.5:35b
provider_url = http://127.0.0.1:11434
context_window = 200000
stream = true
```

### 環境変数

```bash
ANVIL_PROVIDER=ollama             # プロバイダー (ollama / openai)
ANVIL_MODEL=qwen3.5:35b           # モデル名
ANVIL_PROVIDER_URL=http://...     # プロバイダーURL
ANVIL_CONTEXT_WINDOW=200000       # コンテキストウィンドウサイズ
ANVIL_CONTEXT_BUDGET=50000        # トークンバジェット明示指定
ANVIL_MAX_AGENT_ITERATIONS=10     # agenticループの最大反復数
ANVIL_CURL_TIMEOUT=300            # LLMリクエストタイムアウト（秒）
ANVIL_API_KEY=sk-...              # OpenAI互換APIキー
```

### CLI オプション

```
anvil [オプション]

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
```

優先順位: CLI > 環境変数 > 設定ファイル > デフォルト値

### APIキーのセキュリティ

APIキーは設定ファイルではなく**環境変数**で設定することを推奨します:

```bash
export ANVIL_API_KEY=sk-...        # OpenAI互換APIキー
export SERPER_API_KEY=...          # Serper Web検索APIキー
```

設定ファイル（`.anvil/config`）にAPIキーが記載されている場合、起動時に警告メッセージが表示されます。また、`.anvil/` ディレクトリが `.gitignore` に登録されていない場合も警告が表示されます。

設定ファイルの誤コミットによるAPIキー漏洩を防ぐため、`.gitignore` に `.anvil/` を追加してください:

```
# .gitignore
.anvil/
```

## カスタムコマンド

`.anvil/slash-commands.json` で独自のスラッシュコマンドを定義できます:

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

| プロバイダー | 設定例 |
|-------------|--------|
| Ollama | `anvil --model qwen3.5:35b` (デフォルト) |
| OpenAI互換 | `anvil --provider openai --provider-url http://localhost:1234 --model your-model` |
| API キー認証 | `ANVIL_API_KEY=Bearer sk-...` を環境変数に設定 |

---

## 開発者向け

### ソースからビルド

```bash
# Rust toolchain (1.85+) が必要: https://rustup.rs
git clone https://github.com/Kewton/Anvil.git
cd Anvil
cargo build --release
```

### 開発コマンド

```bash
cargo build                       # デバッグビルド
cargo test                        # 全テスト実行（108件）
cargo clippy --all-targets        # 静的解析
cargo fmt                         # フォーマット
cargo run -- --model qwen3.5:35b  # デバッグ実行
```

### プロジェクト構造

```
src/
├── main.rs              # エントリポイント
├── app/                 # アプリケーション層
│   ├── mod.rs           # オーケストレータ
│   ├── agentic.rs       # agenticツール実行ループ
│   ├── cli.rs           # CLI入力ループ
│   ├── plan.rs          # プラン管理
│   └── render.rs        # コンソール描画
├── agent/mod.rs         # LLMプロトコル・パーサー
├── provider/            # LLMプロバイダー
│   ├── ollama.rs        # Ollamaクライアント
│   ├── openai.rs        # OpenAI互換クライアント
│   └── transport.rs     # HTTPトランスポート（curl）
├── tooling/mod.rs       # ツール実行・検証・サンドボックス
├── session/mod.rs       # セッション永続化
├── config/mod.rs        # 設定管理
├── state/mod.rs         # 状態マシン
└── extensions/mod.rs    # スラッシュコマンド拡張
tests/                   # 統合テスト（108件）
```

### コントリビューション

1. Issue を作成
2. `feature/<issue>-<description>` ブランチを作成
3. 実装 → テスト → clippy 通過を確認
4. Pull Request を作成（develop ブランチ向け）

詳細は [CLAUDE.md](CLAUDE.md) を参照してください。

## ライセンス

[MIT](LICENSE)
