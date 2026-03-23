# CLAUDE.md

このドキュメントはClaude Code向けのプロジェクトガイドラインです。

---

## プロジェクト概要

### 基本情報
- **プロジェクト名**: Anvil
- **説明**: ローカルターミナルで動作するコーディングエージェント（Ollama/OpenAI互換バックエンド対応）
- **リポジトリ**: https://github.com/kewton/Anvil

### 技術スタック
| カテゴリ | 技術 |
|---------|------|
| **言語** | Rust (Edition 2024) |
| **ビルド** | Cargo |
| **LLMバックエンド** | Ollama, OpenAI互換API |
| **HTTP** | reqwest (blocking, rustls-tls) |
| **テスト** | cargo test (統合テスト中心) |
| **CI入力** | rustyline |

---

## ブランチ構成

### ブランチ戦略
```
main (本番) <- PRマージのみ
  |
develop (受け入れ・動作確認)
  |
feature/*, fix/*, hotfix/* (作業ブランチ)
```

### 命名規則
| ブランチ種類 | パターン | 例 |
|-------------|----------|-----|
| 機能追加 | `feature/<issue-number>-<description>` | `feature/123-add-tool-plugin` |
| バグ修正 | `fix/<issue-number>-<description>` | `fix/456-fix-symlink-escape` |
| 緊急修正 | `hotfix/<description>` | `hotfix/critical-security-fix` |
| ドキュメント | `docs/<description>` | `docs/update-readme` |

---

## 標準マージフロー

### 通常フロー
```
feature/* --PR--> develop --PR--> main
fix/*     --PR--> develop --PR--> main
hotfix/*  --PR--> main (緊急時のみ)
```

### PRルール
1. **PRタイトル**: `<type>: <description>` 形式
   - 例: `feat: add tool plugin trait`
   - 例: `fix: resolve symlink sandbox escape`
2. **PRラベル**: 種類に応じたラベルを付与
   - `feature`, `bug`, `documentation`, `refactor`
3. **レビュー**: 1名以上の承認必須（main向けPR）
4. **CI/CD**: 全チェックパス必須

### コミットメッセージ規約
```
<type>(<scope>): <subject>

<body>

<footer>
```

| type | 説明 |
|------|------|
| `feat` | 新機能 |
| `fix` | バグ修正 |
| `docs` | ドキュメント |
| `style` | フォーマット（機能変更なし） |
| `refactor` | リファクタリング |
| `test` | テスト追加・修正 |
| `chore` | ビルド・設定変更 |
| `ci` | CI/CD設定 |
| `perf` | パフォーマンス改善 |

---

## コーディング規約

### Rust
- `cargo clippy --all-targets` で警告ゼロを維持
- `cargo test` で全テスト通過を維持
- `unsafe` は使用禁止（明確な理由がない限り）
- エラー型は構造化（`String` ではなく専用enum）を推奨

### モジュール構成
```
src/
├── main.rs              # エントリポイント
├── lib.rs               # モジュール宣言
├── agent/
│   ├── mod.rs           # エージェントループ・プロトコル
│   ├── model_classifier.rs # モデル分類・ToolProtocolMode判定
│   ├── subagent.rs      # サブエージェント実行ループ（Explore/Plan、構造化payload・JSON ANVIL_FINAL対応）
│   ├── tag_parser.rs    # タグベースツール呼び出しパーサー（多層プロトコル対応）
│   └── tag_spec.rs      # ツールタグ仕様テーブル（TOOL_TAG_SPECS）
├── app/
│   ├── mod.rs           # アプリケーションオーケストレータ
│   ├── agentic.rs       # agenticツール実行ループ（ANVIL_FINALガード・再試行ロジック含む）
│   ├── cli.rs           # CLI入力ループ
│   ├── context.rs       # コンテキスト注入（@file展開・サンドボックス検証）
│   ├── loop_detector.rs # ループ検出（リングバッファ・段階的対応）
│   ├── write_fail_tracker.rs # file.write連続失敗トラッキング（ヒント提供・閾値2）
│   ├── plan.rs          # プラン管理
│   ├── policy.rs        # offlineポリシーチェック（共通ヘルパー）
│   ├── render.rs        # コンソール描画
│   └── mock.rs          # テスト用モック
├── config/mod.rs        # 設定管理
├── contracts/
│   ├── mod.rs           # 共通型定義（TerminationReason, Finding, SubAgentPayload含む）
│   └── tokens.rs        # トークン推定（CJK対応ヒューリスティック・モデル実測値ベースEMA補正）
├── extensions/
│   ├── mod.rs           # スラッシュコマンド・拡張
│   └── skills.rs        # SKILL.mdベースのスキルシステム
├── hooks/
│   └── mod.rs           # ライフサイクルフック（HooksConfig, HookRunner, HooksEngine）
├── logging.rs           # 構造化ロギング（tracing初期化）
├── mcp/
│   ├── mod.rs           # MCPクライアント（McpManager, McpConnection, McpError）
│   └── transport.rs     # STDIOトランスポート（McpTransport trait, StdioTransport）
├── metrics/mod.rs       # ベンチマーク
├── provider/
│   ├── mod.rs           # プロバイダー抽象化
│   ├── ollama.rs        # Ollamaクライアント
│   ├── openai.rs        # OpenAI互換クライアント
│   └── transport.rs     # HTTPトランスポート
├── retrieval/mod.rs     # リポジトリ検索（オンデマンドコンテンツ読込・軽量キャッシュ）
├── session/mod.rs       # セッション永続化（名前付きセッション・一覧・切替・削除・マイグレーション・構造化WorkingMemory）
├── spinner.rs           # スピナーUI
├── state/mod.rs         # 状態マシン
├── tooling/
│   ├── mod.rs           # ツール実行・検証・CheckpointStack（undo用チェックポイント管理）
│   ├── diff.rs          # 差分プレビュー生成（file.write/file.edit承認時）
│   ├── file_cache.rs    # ファイル読み取りキャッシュ（FileReadCache: LRUエビクション・sandbox境界検証）
│   └── shell_policy.rs  # ShellPolicy分類（ReadOnly/BuildTest/General）・offline用ネットワークコマンド検出
├── tui/mod.rs           # TUI描画
└── walk.rs              # 共通ディレクトリウォーカー（.gitignore対応・統一スキップ/バイナリ除外）
tests/
├── cli_session.rs       # CLIセッションテスト
├── config_bootstrap.rs  # 設定テスト
├── provider_integration.rs # プロバイダーテスト
├── runtime_flow.rs      # ランタイムフローテスト
├── state_session.rs     # 状態・セッションテスト
├── tooling_system.rs    # ツールシステムテスト
├── mcp_integration.rs   # MCP統合テスト
├── tui_console.rs       # TUIテスト
├── skills_system.rs     # スキルシステムテスト
├── hooks_system.rs      # フックシステムテスト
├── loop_detection.rs    # ループ検出テスト
├── context_inject.rs    # コンテキスト注入テスト
└── walk_system.rs       # ディレクトリウォーカーテスト
```

---

## 品質チェック

| チェック項目 | コマンド | 基準 |
|-------------|----------|------|
| ビルド | `cargo build` | エラー0件 |
| Clippy | `cargo clippy --all-targets` | 警告0件 |
| テスト | `cargo test` | 全テストパス |
| フォーマット | `cargo fmt --check` | 差分なし |

---

## スラッシュコマンド（Claude Code用）

| コマンド | 説明 |
|----------|------|
| `/work-plan` | Issue単位の作業計画立案 |
| `/tdd-impl` | テスト駆動開発で実装 |
| `/pm-auto-dev` | Issue開発を完全自動化（TDD→テスト→報告） |
| `/bug-fix` | バグ調査・修正を自動化 |
| `/create-pr` | PR自動作成（タイトル・説明自動生成） |
| `/worktree-setup` | Issue用Git Worktree環境構築 |
| `/worktree-cleanup` | Worktree環境のクリーンアップ |
| `/progress-report` | 開発進捗レポート作成 |
| `/refactoring` | コード品質改善 |
| `/acceptance-test` | 受入テスト検証 |

---

## サブエージェント

| エージェント | モデル | 役割 |
|-------------|--------|------|
| tdd-impl-agent | opus | TDD実装スペシャリスト |
| acceptance-test-agent | opus | 受入テスト検証 |
| refactoring-agent | opus | コード品質改善 |
| progress-report-agent | sonnet | 進捗レポート作成 |
| investigation-agent | opus | バグ原因調査 |

---

## 禁止事項

- `main` への直接プッシュ禁止
- `force push` 禁止（自分のブランチを除く）
- `unsafe` コード禁止（明確な理由なし）
- テストなしのマージ禁止
- clippy警告の放置禁止
