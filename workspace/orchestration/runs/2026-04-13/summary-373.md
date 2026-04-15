## オーケストレーション完了報告 — Issue #373

### 対象Issue

| Issue | タイトル | ステータス |
|-------|---------|-----------|
| #373 | feat: make native tool calling the primary provider path | 完了 |

### 実行フェーズ結果

| Phase | 内容 | ステータス |
|-------|------|-----------|
| 1 | 依存関係分析 | 完了 |
| 2 | Worktree準備 | 完了（feature/issue-373-native-tool-calling） |
| 3 | 並列開発 | 完了（/pm-auto-issue2dev 373、未コミット変更を手動コミット） |
| 5 | 品質確認 | 完了（全Pass） |
| 6 | PR・マージ | 完了（PR #378） |

### 品質チェック

| チェック項目 | 結果 |
|-------------|------|
| cargo build | Pass |
| cargo clippy --all-targets | Pass（警告0件） |
| cargo test | Pass（全テストパス） |
| cargo fmt --check | Pass（差分なし） |

### 変更内容（16ファイル、+1986/-120）

- `src/provider/mod.rs` (+177): tool definitions / tool_calls を ProviderTurnRequest/Response に追加
- `src/provider/ollama.rs` (+222): Ollama native /api/chat tools サポート
- `src/provider/openai.rs` (+819): OpenAI-compatible tools/tool_calls + streaming assembly
- `src/app/agentic.rs` (+35): capability-based routing (native → ANVIL text → repair fallback)
- `src/app/mod.rs` (+62): native tool call 正規化
- `src/agent/mod.rs` (+86): subagent native tool support
- `src/tooling/mod.rs` (+255): tool schema 生成 + ToolInput 変換
- `src/config/mod.rs` (+9): ANVIL_NATIVE_TOOLS env var
- tests/ (+427): provider, runtime, tooling, config, session, CLI テスト

### 成果物

- 実行計画: workspace/orchestration/runs/2026-04-13/plan-373.md
- 統合サマリー: workspace/orchestration/runs/2026-04-13/summary-373.md
- PR: https://github.com/Kewton/Anvil/pull/378
- マージコミット: `a3e912a`
- 開発コミット: `df8ac6b`
