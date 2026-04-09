# Anvil 機能概要と特徴

## プロジェクト概要

**Anvil** はローカルターミナルで動作するコーディングエージェントです。Ollama や OpenAI 互換サーバーを LLM バックエンドとして使用し、ファイル操作・シェルコマンド実行・コード検索などを AI が自律的に行います。

| 項目 | 内容 |
|------|------|
| 言語 | Rust (Edition 2024) |
| バージョン | v0.0.7 |
| 対応LLM | Ollama, OpenAI互換API (LM Studio, vLLM, Azure OpenAI等) |
| 実行モード | 対話モード / ワンショットモード (`--oneshot`) |
| ライセンス | MIT |

---

## アーキテクチャ

```
┌─────────────────────────────────────────────────────┐
│                      TUI / CLI                       │
│  (rustyline REPL, スラッシュコマンド, 承認フロー)      │
├─────────────────────────────────────────────────────┤
│                   App Orchestrator                    │
│  (状態機械, セッション管理, プラン管理)                │
├──────────────┬──────────────┬────────────────────────┤
│ Agentic Loop │  SubAgents   │   Extensions/Skills    │
│ (マルチターン│ (Explore,    │  (スラッシュコマンド,   │
│  ツール実行) │  Plan)       │   カスタムスキル)       │
├──────────────┴──────────────┴────────────────────────┤
│                    Tool Executor                      │
│  (file.read/write/edit, shell.exec, web, git, MCP)   │
├─────────────────────────────────────────────────────┤
│                   LLM Provider                        │
│  (Ollama, OpenAI互換, ストリーミング, リトライ)        │
└─────────────────────────────────────────────────────┘
```

---

## コア機能

### 1. マルチターン Agentic Loop

LLM とツール実行を繰り返すマルチターンループがエージェントの中核です。

- LLM がツール呼び出しを生成 → ツール実行 → 結果をフィードバック → 次のターン
- 最大イテレーション数: デフォルト30ターン
- `ANVIL_FINAL` マーカーによる明示的な完了宣言
- フォールバック完了検出（マーカー未発行時はフェーズ推定で判断）

### 2. ツールシステム

| ツール | 権限 | 説明 |
|--------|------|------|
| `file.read` | Safe | ファイル読取・ディレクトリ一覧・画像base64 |
| `file.write` | Confirm | ファイル作成・上書き（削除比率チェック付き） |
| `file.edit` | Confirm | old_string/new_string による行単位編集 |
| `file.edit_anchor` | Confirm | インデント正規化付き編集 |
| `file.search` | Safe | ファイル名・内容検索（正規表現対応） |
| `shell.exec` | Confirm | シェルコマンド実行（リアルタイム出力） |
| `web.fetch` | Safe | URL取得 |
| `web.search` | Safe | Web検索（DuckDuckGo/Serper） |
| `git.status/diff/log` | Safe | Git操作（読み取り専用） |
| `mcp.tool` | Safe/Confirm | MCP（Model Context Protocol）ツール |
| `agent.explore` | Safe | サブエージェント（コードベース調査） |
| `agent.plan` | Safe | サブエージェント（計画策定） |

### 3. LLM プロバイダー

**Ollama:**
- `/api/chat` エンドポイント対応
- ストリーミング応答
- モデル情報自動取得（コンテキスト長、パラメータ数）
- サイドカーモデルによる要約生成

**OpenAI互換:**
- `/v1/chat/completions` 対応
- APIキー認証
- 画像マルチモーダル対応
- LM Studio, vLLM, Azure OpenAI 等で動作

**共通:**
- 指数バックオフリトライ（最大3回）
- タイムアウト: デフォルト300秒

### 4. プラン管理 (Execution Plan)

- `ANVIL_PLAN` / `ANVIL_PLAN_UPDATE` ブロックで構造化プランを管理
- プラン項目ごとの進捗追跡（Pending → InProgress → Done / Superseded）
- Final Gate: 全項目完了を確認してから `ANVIL_FINAL` を許可
- Late-stage closure mode: 残り1項目で closure-focused ヒントを注入
- Follow-up replan: 作業中のプラン更新に対応

### 5. セッション管理

- 会話履歴の永続化 (`.anvil/sessions/`)
- 名前付きセッション、一覧・切替・削除
- **Working Memory**: 自動維持される構造化メモリ
  - active_task, constraints, touched_files, unresolved_errors, recent_diffs
  - コンテキスト圧縮後も保持される
- トークン予算自動管理: メッセージ数閾値を超えると自動compact

### 6. サブエージェント

| 種別 | 用途 | 使用可能ツール |
|------|------|--------------|
| Explore | コードベース調査 | file.read, file.search, git.* |
| Plan | 計画策定 | file.read, file.search, web.fetch, git.* |

- メインエージェントとは独立したLLMループ
- 読み取り専用（ファイル変更不可）
- タイムアウト・イテレーション制限あり

### 7. MCP (Model Context Protocol)

- 外部ツールサーバーとの連携
- プロトコル: 2024-11-05
- STDIO トランスポート
- ツール名検証（制御文字・ダブルアンダースコア禁止）

---

## 安全性・品質保証

### 多層的な異常検出

| 検出器 | 機能 |
|--------|------|
| **LoopDetector** | 同一ツール呼び出しの繰り返しをハッシュで検出。3段階エスカレーション (Warn → StrongWarn → Break) |
| **AlternatingLoopDetector** | 交互/循環パターンの検出 |
| **ReadRepeatTracker** | 同一ファイルの繰り返し読み取りを検出 |
| **ReadTransitionGuard** | 長時間の探索フェーズを検出し実装への移行を促す |
| **EditFailTracker** | file.edit連続失敗を追跡、フォールバック (ReRead → WriteFallback) |
| **WriteFailTracker** | file.write連続失敗を追跡 |
| **WriteRepeatTracker** | 同一ファイルへの繰り返し書き込みを検出 |
| **StagnationState** | ターン単位の停滞テレメトリ。ファイル飢餓追跡、ワークセット同一性チェック |
| **PhaseEstimator** | ツール呼び出しパターンからフェーズ推定 (Exploring/Implementing) |

### 承認フロー

- デフォルト: `file.write`, `file.edit`, `shell.exec` は実行前に `[y/n]` 確認
- `--no-approval` で全ツール自動実行
- `/trust` コマンドでツール単位の信頼設定

### コマンドブロックリスト

以下は承認モードに関係なく常にブロック:
- `rm -rf /`, `rm -rf ~`
- `mkfs`, `dd if=`, Fork bomb `:(){`

### サンドボックス

- パスは作業ディレクトリに相対化
- `..` によるディレクトリ脱出防止
- シンボリックリンク検証

---

## モデル適応

### 自動分類

モデル名のパラメータ数から自動的に能力を分類:

| モデルサイズ | プロトコル | プロンプトTier |
|-------------|-----------|---------------|
| Large (>13B) | JSON | Full |
| Medium (7B-13B) | JSON | Compact |
| Small (<=7B) | Tag-based | Tiny |

### Tag-based プロトコル

小モデル向けに `<file.read path="...">` 形式のタグベース呼び出しをサポート。JSONパースが困難なモデルでもツール呼び出しが可能。

### Shell Inspection 統合 (Issue #309)

`shell.exec` によるinspectionコマンド (grep, find, wc, diff, rg等) を PhaseEstimator で Read として分類し、shell-based inspection drift を防止。

---

## 拡張性

### スラッシュコマンド

| コマンド | 説明 |
|----------|------|
| `/help` | ヘルプ表示 |
| `/plan`, `/plan-add`, `/plan-focus`, `/plan-clear` | プラン管理 |
| `/checkpoint` | チェックポイント保存 |
| `/compact` | 履歴圧縮 |
| `/model`, `/model-list`, `/model-switch` | モデル管理 |
| `/session-list`, `/session-switch`, `/session-delete` | セッション管理 |
| `/trust` | ツール信頼設定 |
| `/undo` | ツール実行取り消し |
| `/repo-find` | リポジトリ検索 |
| `/timeline` | セッションタイムライン |

### カスタムコマンド

`.anvil/slash-commands.json` で独自コマンドを定義可能。

### スキルシステム

`SKILL.md` ファイルでプロジェクト・ユーザースコープのスキルを定義。複雑なワークフローをテンプレート化。

### カスタムツール

設定ファイルで最大10個のカスタムツールを定義可能。シェルコマンドテンプレートを展開して実行。

### ライフサイクルフック

`HooksConfig` でイベント駆動のカスタム処理を定義可能。

---

## 設定

### 優先度

CLI引数 > 環境変数 > 設定ファイル (`.anvil/config.toml`) > デフォルト値

### 主要設定項目

| 設定 | デフォルト | 説明 |
|------|-----------|------|
| `provider` | ollama | LLMプロバイダー |
| `model` | - | 使用モデル |
| `max_agent_iterations` | 30 | 最大ターン数 |
| `stream` | true | ストリーミング応答 |
| `loop_detection_threshold` | 3 | ループ検出閾値 |
| `edit_strategy` | EditFirst | 編集戦略 |
| `ui_language` | ja | UI言語 (ja/en) |
| `context_window` | モデル依存 | コンテキストウィンドウサイズ |
| `context_budget` | - | コンテキスト予算 |

---

## 技術的特徴まとめ

1. **完全ローカル動作**: Ollama等のローカルLLMで動作し、データが外部に出ない
2. **多層的な安全機構**: ループ検出、停滞検出、コマンドブロック、サンドボックス、承認フロー
3. **モデル自動適応**: パラメータ数に応じてプロトコル・プロンプトを自動調整
4. **構造化プラン管理**: ANVIL_PLAN/ANVIL_FINAL による計画的なタスク遂行
5. **セッション永続化**: Working Memory による文脈保持、コンテキスト圧縮
6. **拡張可能**: MCP, カスタムツール, スキル, フックによる機能拡張
7. **Rust実装**: 高速・低メモリ・型安全な実装
