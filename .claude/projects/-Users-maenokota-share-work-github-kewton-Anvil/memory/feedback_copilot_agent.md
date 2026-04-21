---
name: copilot agent opus model
description: commandmatedevでOpus 4.6を使うには --agent copilot --model claude-opus-4.6 を指定する
type: feedback
---

commandmatedev send で Opus 4.6 を使うには `--agent copilot --model claude-opus-4.6` を指定する。

**Why:** `--agent copilot` のみではSonnetが使われる。`--model claude-opus-4.6` を追加するとOpus 4.6に切り替わる。ステータスバーに `Claude Opus 4.6 (3x) (high)` と表示されることで確認可能。

**How to apply:** /cause-analysis や深層分析でOpus 4.6を使いたい場合は必ず `--agent copilot --model claude-opus-4.6` を両方指定する。
