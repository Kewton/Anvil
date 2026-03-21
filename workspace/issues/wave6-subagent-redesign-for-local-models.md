## 概要
親 Issue #127 の Wave 6 として、sub-agent をローカルモデル前提の探索専用ワーカーへ再設計する。

## 背景・動機
現状の sub-agent は補助的な read-only ループとしては機能するが、固定反復・固定 timeout・返却 payload の薄さにより、ローカルモデル向けの探索強化としてはまだ弱い。
親エージェントとの役割分担も、より明確に設計し直す余地がある。

## 提案する解決策

- sub-agent を探索特化ワーカーとして再設計する
- 親エージェントへ返す payload を構造化する
- main agent と sub-agent の責務分担を local-first 前提で見直す

## 受け入れ基準

- [ ] sub-agent の返却 payload が構造化される
- [ ] sub-agent の役割が探索特化として整理される
- [ ] iteration / timeout / result handling の設計が見直される
- [ ] 親エージェントとの連携フローに対するテストが追加または更新される
- [ ] 既存の `agent.explore` / `agent.plan` の移行方針が明確になる

## 設計メモ（任意）

- 主な対象: `src/agent/subagent.rs`, `src/app/agentic.rs`
- いきなり高機能化するより、探索結果を親が使いやすい構造にすることを優先する

## 追加情報（任意）

- Parent: #127
