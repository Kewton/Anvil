---
name: "source-command-acceptance-test"
description: "受け入れテストを作成・実行"
---

# source-command-acceptance-test

Use this skill when the user asks to run the migrated source command `acceptance-test`.

## Command Template

# 受け入れテストスキル

## 概要
Issueの受け入れ基準に基づいてテストを作成し、実行します。

## 使用方法
- `/acceptance-test [Issue番号]`

## 実行内容

**共通プロンプトを読み込んで実行します**:

```bash
cat .Codex/prompts/acceptance-test-core.md
```

↑ **このプロンプトの内容に従って、受け入れテストを実行してください。**

## サブエージェントモード

サブエージェントとして呼び出す場合：

```
Use acceptance-test-agent to verify Issue #XXX acceptance criteria.
```

## 完了条件

以下をすべて満たすこと：
- 受け入れ基準ごとにテストが作成されている
- すべてのテストがパス（`cargo test --all`）
- `cargo clippy --all-targets -- -D warnings` で警告ゼロ
- テスト結果のサマリーが出力されている
