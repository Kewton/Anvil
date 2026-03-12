# Anvil vs vibe-local Roadmap

日付: 2026-03-12

目的:

- 速度
- 安定性
- 品質
- 柔軟性

の 4 軸で `vibe-local` を上回るための実装順序を定義する。

## 現状整理

`vibe-local` の強み:

- tool streaming がすでに実装済み
- sidecar compaction が実運用レベル
- Plan/Act、checkpoint、auto-test、watcher が近接している
- create task での output bias が強い

Anvil の強み:

- requirement / evidence / phase / permission / provider の責務分離
- auditability と fail-closed の設計密度
- fixed workflow に寄せない構造化

結論:

- 短期では `vibe-local` が速く、出力品質も高い
- 長期では Anvil の方が構造的に上回れる

## Roadmap

### Wave 1: 速度で追いつく

対象:

- provider-native tool streaming
- provider runtime tuning
- stale cache invalidation
- sync fallback 最適化

やること:

- Phase 3.8 の `provider capability / runtime policy / fallback provenance` を実装
- `write/edit/mkdir` 後の stale cache を global invalidation でまず解消
- `tool mode` の low-temperature / keep-alive / timeout / retry / backoff を固定
- `stream -> sync fallback` を 1 回で止める

期待効果:

- 1 step あたりの待ち時間短縮
- 同一モデルでも tool use の成功率向上

### Wave 2: 安定性で逆転する

対象:

- requirement truth source
- transcript integrity
- sidecar compaction
- permission state

やること:

- `RequirementState` を唯一の truth source にする
- `tool_calls / tool_results` の pairing integrity を compaction 後も保証
- sidecar compaction に `quality guard` を入れる
- permission snapshot / lifetime / approval compression を loop context に載せる

期待効果:

- `vibe-local` より explainable な stalled / finalize 判定
- 長時間 session での破綻率低下

### Wave 3: 品質で逆転する

対象:

- quality targets
- dynamic stance
- post-write safeguards
- artifact link graph

やること:

- `Task-Type Requirement Profile` を task contract から決定
- `Dynamic Execution Stance` を `phase + evidence delta + budget` で更新
- post-write verification hooks を event-based に導入
- `plan -> evidence -> finding -> final` の artifact link graph を残す

期待効果:

- create/edit task での仕上がり向上
- 生成後の自己検証精度向上

### Wave 4: 柔軟性で差を広げる

対象:

- profile confidence
- fallback ladder
- plan/execution artifact split
- clarification escape hatch

やること:

- `selected_profile / confidence / fallback_profile` を導入
- fallback extractor は read-only 限定で運用
- plan artifact と execution artifact を分離
- ambiguity 時は clarification path へ逃がす

期待効果:

- fixed workflow を増やさずに安定化
- task variety が増えても拡張しやすい

## 重点優先度

### P0

- provider streaming
- runtime tuning
- stale cache invalidation
- phase truth source

### P1

- sidecar compaction
- transcript integrity
- requirement profile
- dynamic stance

### P2

- approval compression
- artifact link graph
- safeguard bundle
- fallback provenance / quality penalty

### P3

- clarification escape hatch
- plan/execution artifact lifecycle の本格化

## 成功指標

### 速度

- `qwen3.5:35b` での create task 平均 step latency
- sync fallback 発生率
- tool streaming 使用率

### 安定性

- max loop 到達率
- stale read に起因する再試行率
- requirement/phase 不整合件数

### 品質

- browser-runnable game の完走率
- generated artifact の verification pass 率
- review finding を反映した final response 率

### 柔軟性

- 同一 task type で許容される tool sequence の種類
- profile fallback 発動時の完走率
- clarification を挟んだ task 完走率

## 最終目標

Anvil は `vibe-local` を単に再現するのではなく、次を備えたローカル coding agent を目指す。

- `vibe-local` と同等以上の速度
- `vibe-local` より高い explanation quality
- `vibe-local` より高い auditability
- `vibe-local` より高い設計一貫性
