---
model: opus
description: "Issue単位の設計方針書を作成"
---

# 設計方針書作成スキル

## 概要
Issue単位での設計方針書を作成するスキルです。Anvilプロジェクトのアーキテクチャに沿った設計判断を文書化し、実装前の合意形成を支援します。

## 使用方法
- `/design-policy [Issue番号]`
- 「Issue #XXXの設計方針書を作成してください」

## 前提条件
- 対象Issueの内容が明確であること
- GitHubリポジトリにアクセス可能

## 実行内容

あなたはソフトウェアアーキテクトとして、以下の設計方針書を作成します。

### 1. Issue情報の取得

```bash
gh issue view {issue_number} --json number,title,body,labels,assignees
```

### 2. システムアーキテクチャ概要

Anvilの全体アーキテクチャを踏まえた設計を行います：

```
┌─────────────────────────────────────────────────┐
│                    CLI (rustyline)                │
│                   src/app/cli.rs                  │
├─────────────────────────────────────────────────┤
│              Application Orchestrator             │
│                  src/app/mod.rs                    │
│         ┌──────────┬──────────┬────────┐         │
│         │ agentic  │  plan    │ render │         │
│         └──────────┴──────────┴────────┘         │
├─────────────────────────────────────────────────┤
│               Agent Loop / Protocol               │
│                src/agent/mod.rs                    │
├──────────────┬──────────────┬───────────────────┤
│   Provider   │   Tooling    │    Extensions      │
│ src/provider/│ src/tooling/ │  src/extensions/   │
│  ┌────────┐  │              │                    │
│  │ ollama │  │              │                    │
│  │ openai │  │              │                    │
│  │transport│ │              │                    │
│  └────────┘  │              │                    │
├──────────────┴──────────────┴───────────────────┤
│  State Machine  │  Session   │  Config  │  TUI   │
│  src/state/     │ src/session│src/config│src/tui │
├─────────────────┴────────────┴─────────┴────────┤
│              Contracts (共通型定義)                 │
│               src/contracts/mod.rs                │
└─────────────────────────────────────────────────┘
```

### 3. レイヤー構成と責務

| レイヤー | モジュール | 責務 |
|---------|-----------|------|
| **CLI** | `src/app/cli.rs` | ユーザー入力の受付、rustylineによる対話 |
| **App** | `src/app/` | アプリケーションロジックの統合、ツール実行ループ |
| **Agent** | `src/agent/` | LLMとの対話プロトコル、エージェントループ |
| **Provider** | `src/provider/` | LLMバックエンド抽象化（Ollama/OpenAI互換） |
| **Tooling** | `src/tooling/` | ツール定義・実行・結果検証 |
| **Extensions** | `src/extensions/` | スラッシュコマンド、拡張機能 |
| **State** | `src/state/` | 状態マシン、状態遷移管理 |
| **Session** | `src/session/` | セッション永続化（JSONファイル） |
| **Config** | `src/config/` | 設定管理（TOML/環境変数） |
| **TUI** | `src/tui/` | ターミナルUI描画 |
| **Contracts** | `src/contracts/` | 共通型定義（Message, Tool, Response等） |

### 4. 技術選定

| カテゴリ | 選定技術 | 選定理由 |
|---------|---------|---------|
| 言語 | Rust (Edition 2024) | メモリ安全性、パフォーマンス |
| ビルド | Cargo | Rust標準ビルドシステム |
| LLMバックエンド | Ollama, OpenAI互換API | ローカル/クラウド両対応 |
| HTTP | curl subprocess | 外部依存最小化 |
| CLI入力 | rustyline | 行編集・履歴サポート |
| データ永続化 | JSONファイル | シンプル、外部DB不要 |
| テスト | cargo test | 統合テスト中心 |

### 5. 設計パターン

#### 5-1. Provider抽象化（trait + enum dispatch）

```rust
/// LLMプロバイダーの共通インターフェース
trait LlmProvider {
    fn chat(&self, messages: &[Message]) -> Result<Response, ProviderError>;
    fn list_models(&self) -> Result<Vec<String>, ProviderError>;
}

/// Ollamaプロバイダー実装
struct OllamaProvider {
    endpoint: String,
    model: String,
}

impl LlmProvider for OllamaProvider {
    fn chat(&self, messages: &[Message]) -> Result<Response, ProviderError> {
        // curl subprocess でOllama APIを呼び出し
    }
    // ...
}
```

#### 5-2. 状態マシンパターン

```rust
/// エージェントの状態
enum AgentState {
    Idle,
    WaitingForInput,
    Processing,
    ToolExecution(ToolCall),
    Error(AgentError),
}

/// 状態遷移
impl AgentState {
    fn transition(self, event: AgentEvent) -> Self {
        match (self, event) {
            (AgentState::Idle, AgentEvent::UserInput(msg)) => AgentState::Processing,
            (AgentState::Processing, AgentEvent::ToolCall(call)) => AgentState::ToolExecution(call),
            // ...
        }
    }
}
```

#### 5-3. エラー型（構造化enum）

```rust
/// プロバイダーエラー
#[derive(Debug, thiserror::Error)]
enum ProviderError {
    #[error("connection failed: {0}")]
    ConnectionFailed(String),
    #[error("model not found: {0}")]
    ModelNotFound(String),
    #[error("invalid response: {0}")]
    InvalidResponse(String),
}
```

### 6. データモデル

AnvilはセッションデータをJSON形式でファイルに永続化します：

```
~/.anvil/
├── config.toml          # ユーザー設定
└── sessions/
    └── {session_id}.json # セッションデータ
```

#### セッションデータ構造

```rust
struct Session {
    id: String,
    created_at: DateTime,
    messages: Vec<Message>,
}

struct Message {
    role: Role,       // User, Assistant, System, Tool
    content: String,
    tool_calls: Option<Vec<ToolCall>>,
    tool_result: Option<ToolResult>,
}
```

### 7. セキュリティ設計

| 脅威 | 対策 | 優先度 |
|------|------|--------|
| **サンドボックス脱出** | ツール実行時のパス検証、許可ディレクトリ制限 | 高 |
| **コマンドインジェクション** | shell exec時の入力サニタイズ、許可コマンドリスト | 高 |
| **パストラバーサル** | ファイル操作時の正規化とベースディレクトリチェック | 高 |
| **APIキー漏洩** | ログ出力からのマスキング、環境変数での管理 | 高 |
| **unsafe使用** | 原則禁止、必要時はレビュー必須 | 中 |
| **大量リソース消費** | ツール実行のタイムアウト設定 | 中 |

### 8. 設計判断とトレードオフ

Issue #{issue_number} に関する設計判断を記録します：

```markdown
### 設計判断 #1: [判断タイトル]

**選択肢**:
- A: [選択肢Aの説明]
- B: [選択肢Bの説明]

**決定**: 選択肢 A

**理由**:
- [理由1]
- [理由2]

**トレードオフ**:
- メリット: [メリット]
- デメリット: [デメリット]
- リスク: [リスク]
```

### 9. 影響範囲

変更対象のモジュールと影響範囲を明記します：

| モジュール | 変更種別 | 影響度 |
|-----------|---------|--------|
| `src/xxx/` | 新規追加/変更 | 高/中/低 |
| `tests/xxx.rs` | テスト追加 | - |

### 10. 品質基準

| チェック項目 | コマンド | 基準 |
|-------------|----------|------|
| ビルド | `cargo build` | エラー0件 |
| Clippy | `cargo clippy --all-targets` | 警告0件 |
| テスト | `cargo test` | 全テストパス |
| フォーマット | `cargo fmt --check` | 差分なし |

## 出力先

`dev-reports/issue/{issue_number}/design-policy.md`

## 完了条件

- アーキテクチャ図が作成されている
- レイヤー構成と責務が明確である
- 設計パターンが具体的なRustコードで示されている
- セキュリティ要件が記載されている
- 設計判断とトレードオフが記録されている
- 影響範囲が明確である

## 関連コマンド

- `/architecture-review`: アーキテクチャレビュー実行
- `/apply-review`: レビュー結果を設計方針書に反映
- `/multi-stage-design-review`: マルチステージ設計レビュー
- `/work-plan`: 作業計画立案
- `/pm-auto-design2dev`: 設計から開発まで一括実行
