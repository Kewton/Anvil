# Anvil Usage Image

## Purpose

This document sketches the intended terminal experience of Anvil before detailed UI implementation.
The goal is to make the product feel concrete in actual use, not only at the architecture level.

The examples below are not final UI specs.
They are concept mockups for interaction design.

---

## 1. 起動直後

起動直後の印象は重要です。
この時点でユーザーに伝わるべきことは下記です。

- ローカルLLM向けのプロダクトであること
- 今どのモデルで動くのか
- 入力待ち状態であること
- ユーザー入力欄とシステム状態欄が分離されていること

### イメージ

```text
    ___              _ __
   /   |  ____ _   _(_) /_
  / /| | / __ \ | / / / __/
 / ___ |/ / / / |/ / / /_
/_/  |_/_/ /_/|___/_/\__/

  local coding agent for serious terminal work

  Model   : qwen3-coder-next
  Context : 262k
  Mode    : local / confirm
  Project : /Users/maenokota/share/work/github_kewton/Anvil

  --------------------------------------------------------------
  Ready.
  Ask for a task, or use /help, /model, /plan, /status
  --------------------------------------------------------------

  [U] you >
```

### この場面の狙い

- ASCIIアート風ロゴでプロダクトの顔を出す
- 上部は「環境の把握」、下部は「対話の開始」に役割分離する
- `[U] you >` を明示し、入力主体が一目で分かるようにする

---

## 2. 指示入力

ユーザーが最初の指示を入力する場面では、会話ではなく作業指示を出している感覚が必要です。
そのため、チャットアプリ風よりも operator console に近い表現が望ましいです。

### イメージ

```text
  [U] you > このリポジトリを調査して、RustでCLIとして再設計する方針をまとめて

  --------------------------------------------------------------
  Ready.
  Enter to send / """ for multi-line / ESC to interrupt agent
  --------------------------------------------------------------
```

### この場面の狙い

- ユーザー入力は常に `[U] you >` で始まり、見失わない
- 追加の説明なしで送信ルールが分かる
- 入力内容が長くても、誰の発話か見た瞬間に認識できる

---

## 3. エージェント考え中

考え中の状態は「何も起きていない」ように見せてはいけません。
ユーザーが知りたいのは下記です。

- 全体の作業計画が何か
- そのうち今どこを実施しているか
- いま何を根拠に進めているか
- エージェントが今動いているか
- 何をしている途中か
- 中断可能か

### イメージ

```text
  [U] you > このリポジトリを調査して、RustでCLIとして再設計する方針をまとめて

  [A] anvil > plan
              1. inspect repository structure
              2. map runtime and tool flow
              3. summarize constraints and strengths
              4. write redesign direction

  [A] anvil > working on 2/4: map runtime and tool flow
              reading project structure
              checking runtime flow
              collecting tool and session model

  [A] anvil > thinking
              - main() is wiring startup, session, tools, and interactive loop
              - Config owns model selection and context defaults
              - Agent loop appears tool-driven rather than chat-driven
              - next: confirm how permissions and plan mode change execution

  --------------------------------------------------------------
  Thinking... 12s   model:qwen3-coder-next   ctx:18%   active:2/4
  ESC stop  /status  /plan  typeahead enabled
  --------------------------------------------------------------
```

### この場面の狙い

- `[A] anvil >` を出して、エージェントの動作主体を視覚的に分離する
- 全体計画と現在のステップを同時に見せる
- 思考内容を短い reasoning log として見せ、完全なブラックボックスにしない
- 思考中でも完全な無言にしない
- 下部ステータスで「時間」「モデル」「コンテキスト」「段階」を見せる
- `typeahead enabled` により、次の入力を始めてよいことが分かる

---

## 4. エージェント回答

回答時には、本文とアクション結果が混ざって見えないことが重要です。
特にコーディングエージェントでは、下記の分離が必要です。

- エージェントの説明
- ツール実行
- 完了状態

### イメージ

```text
  [A] anvil > 調査結果を整理しました。現状は単一ファイル構成に機能が集中しており、
              local-first の強みはありますが、拡張性に制約があります。
              Rust化では runtime, session, tool system, tui を分離するのが有効です。

  [T] tool  > Read   vibe-coder.py
  [T] tool  > Grep   class | def | main
  [T] tool  > Read   README.md
  [T] tool  > Write  workspace/anvil-architecture-notes.md

  [A] anvil > `workspace/anvil-architecture-notes.md` に方針メモを書きました。
              次は、アーキテクチャのモジュール境界を具体化できます。

  --------------------------------------------------------------
  Done. 31s   4 tools   session saved
  /diff  /open-notes  /continue
  --------------------------------------------------------------

  [U] you >
```

### この場面の狙い

- `[A] anvil >` と `[T] tool  >` を分け、説明と実行を混ぜない
- 回答本文は人間向け、ツール行は操作ログとして扱う
- 完了後は `Done` を出し、待機状態に戻ったことを明示する

---

## 5. 追加指示

2回目以降のやり取りでは、文脈の継続感が必要です。
ただし、履歴に埋もれて現在位置が不明になるのは避けるべきです。

### イメージ

```text
  [U] you > いいですね。次に tool system の責務分離案を出して

  [A] anvil > 了解。既存の `vibe-local` で混在している責務を、
              command execution / permission policy / tool registry / tool I/O
              に分割する前提で整理します。

  --------------------------------------------------------------
  Working...   follow-up task   ctx:24%
  ESC stop  /status  /compact  /checkpoint
  --------------------------------------------------------------
```

### この場面の狙い

- 追加指示でも会話感より作業継続感を優先する
- フォローアップであることが自然に分かる
- コンテキスト使用率や checkpoint のような長時間作業向け情報を見せる

---

## UX Principles Shown By These Examples

- ユーザー入力は常に `[U] you >`
- エージェント出力は常に `[A] anvil >`
- ツール実行は常に `[T] tool  >`
- 下部ステータス領域は「現在の状態」を示す専用領域
- 作業状態は `Ready / Thinking / Working / Done` のように短く明示する
- 考え中は全体計画と現在の実施ステップが見える
- 考え中は短い reasoning log も見え、何を見て次に何を確認するかが分かる
- ロゴ、入力、出力、ツール、状態表示が役割ごとに分離されている

## Summary

Anvil の利用イメージは、チャットアプリ風ではなく、ローカルLLM向けの operator console に寄せる。
そのうえで、ユーザー、エージェント、ツール、状態をひと目で識別できることを最重要のUX要件とする。
