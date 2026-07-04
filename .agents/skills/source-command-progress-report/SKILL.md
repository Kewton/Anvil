---
name: "source-command-progress-report"
description: "プロジェクトの進捗レポートを生成"
---

# source-command-progress-report

Use this skill when the user asks to run the migrated source command `progress-report`.

## Command Template

# 進捗レポートスキル

## 概要
プロジェクトの現在の進捗状況を分析し、レポートを生成します。

## 使用方法
- `/progress-report`

## 実行内容

**共通プロンプトを読み込んで実行します**:

```bash
cat .Codex/prompts/progress-report-core.md
```

↑ **このプロンプトの内容に従って、進捗レポートを生成してください。**

## サブエージェントモード

サブエージェントとして呼び出す場合：

```
Use progress-report-agent to generate a progress report.
```

## 完了条件
- レポートが生成されていること
- Issue/PR/ブランチの状況が含まれていること
- 今後のアクションが提案されていること
