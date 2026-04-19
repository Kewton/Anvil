# v0.1.0 Runtime Findings And Problem Statement

作成日: 2026-04-17
対象: `anvil` v0.1.0 local-first rebuild

## 目的

2026-04-16 以降に行った heavy live benchmark の試行内容、
そこから分かったこと、
現時点の問題認識をまとめる。

この文書は、`workspace/v0.1.0` の方針に対して
「実装はどこまで進んだか」と
「今どこで詰まっているか」を整理するための追補である。

## 1. これまでやったこと

### 1.1 コア再建

- `src/` を core-only に整理した
- post-core 機能は `workspace/v0.1.0/post-core-src/` へ退避した
- `loop_run.rs` を分解し、small loop に寄せた
- `Plan/Actor/Verifier` の軽量な構造も試した
- native tool calling と fallback の責務を整理した
- `Ollama client` を `transport / parsing / fallback` に分割した

### 1.2 live E2E と heavy benchmark

- semantic Next.js E2E は一度 `5/5` 通過した
- 一方で heavy prompt
  - `最高に面白くかっこいいスペースインベーダーゲーム`
  - `Next.js`
  - `3011`
  - `TDD`
  の条件では未だ安定していない
- `qwen3.5:122b`
- `qwen3.6:35b-a3b`
- `qwen3.5:27b`
- `gemma4:31b`
  で比較を行った
- `first-write benchmark` を追加し、
  「最初の `Write/Edit` に届くか」を独立して測れるようにした

### 1.3 試した対策

効果があり採用したもの:

- late-turn 圧縮
- install loop 抑止
- restart 時の changed-files 収束
- first-write benchmark 追加

効果が弱いか逆効果で戻したもの:

- max-iterations 前の収束制御
- page entry を見た個別再誘導
- budget shaping
- state-based orchestration
- deterministic done gate
- artifact roles
- Verifier-driven restart 強化
- success levels
- post-write hard trim
- support-file divergence control
- error-class specific recovery
- recent-diff continuity
- tool-class cooldown
- initial-turn retry/backoff 強化
- initial-turn minimal context
- first-write mode
- planner/sidecar のさらなる軽量化

## 2. 分かったこと

### 2.1 改善したこと

- scaffold はかなり安定した
- `3011` 起動確認もかなり安定した
- `Read/Write` 到達率は初期より改善した
- setup loop はかなり減らせた
- support file や test file までは進める run が増えた

### 2.2 依然として失敗すること

- heavy 条件では user-facing 実装に収束しない
- `page.tsx` 相当の主要表示面が未変更のまま終わる run が多い
- `rc=0` が出ても意味的完遂ではないことがある
- `first write` 自体に届かないモデル条件もある

### 2.3 failure mode の変化

初期:

- empty response
- no-tool prose
- scaffold 前後の parser failure

現状:

- transport error
- Ollama `/api/chat` の `500`
- prose で締める
- support file だけ増えて主要 UI に着地しない
- `max iterations`

つまり、問題は
「何もできない」段階から
「途中までは進むが主要成果物へ収束しない」段階へ移っている。

## 3. モデル別に分かったこと

### qwen3.5:122b

- scaffold は通る
- heavy 条件では `first-write benchmark` が `0/5`
- `first write/edit` に届く前に止まるケースが多い
- 速度は悪くないが、初手から transport 側で落ちやすい

### qwen3.6:35b-a3b

- `122b` より軽く、`Write` や test file まで届く run がある
- ただし `page.tsx` 実装完了には未達
- support file へ流れて止まる傾向がある

### qwen3.5:27b / gemma4:31b

- app 生成と `3011` 起動はできる
- ただし heavy 条件では `max iterations` に到達しやすい
- 主要実装面への着地は確認できていない

## 4. 現時点の問題認識

### 4.1 本質的な問題

現時点の本丸は、
「scaffold できるか」ではなく
「主要な user-facing 実装へ収束できるか」である。

特に heavy 条件では、

- scaffold
- install
- dev 起動
- support file 追加

までは進んでも、

- 主要 entry
- 主要 UI
- 実際にユーザーが触る面

へ diff を入れる前後で止まる。

### 4.2 12 turn / max-iterations が足りない理由

単に turn 数が少ないのではない。

実際には次が起きている。

- 序盤で `Read/Glob/Bash` に turn を使う
- support file 側へ diff が拡散する
- restart や recovery が入っても主要成果物へ戻らない
- late-turn で `500` や transport error が出る
- そのまま prose stop か `max iterations` で終わる

つまり
「turn 数不足」より
「1 turn あたりの前進が主要成果物に向いていない」ことが問題である。

### 4.3 構造面の問題

`workspace/v0.1.0` の方針に沿って small loop 化は進んだが、
heavy 実運用ではまだ次が残っている。

- protocol が still `native + fallback` の二層
- `Ollama` 側 parser failure を client 側だけでは救い切れない
- actor の停止をそのまま task 完了とみなしやすい
- user-facing 成果物と support file を十分に区別できていない

### 4.4 実運用上の問題

heavy benchmark では、
release acceptance に対して次が未達である。

- `page.tsx` 相当の主要 UI が変わること
- TDD の実ファイルが継続的に生成されること
- semantic completion が安定して通ること

要するに、
`v0.1.0 core` は軽い semantic E2E では成立しているが、
heavy local-LLM 条件ではまだ acceptance を満たしていない。

## 5. 現時点の結論

現状は次のように整理できる。

- architecture は `workspace/v0.1.0` の方針へかなり近づいた
- scaffold/dev 起動は安定した
- `first write` と support file 追加までは到達しやすくなった
- しかし heavy 条件では、主要な user-facing 実装へまだ収束しない

したがって次の改善対象は、

- turn 数を増やすこと
- recovery note を増やすこと

ではなく、

- 主要成果物への収束性
- late-turn transport 安定性
- support file 側への拡散抑制

である。

## 6. 次の検討軸

次に有効そうなテーマは次の 3 本である。

- user-facing 実装と support file 実装を、より汎用的に区別する方法
- first-write 到達率を上げる、より軽い initial-turn 実行戦略
- late-turn の `500` / transport error を減らす transport 側の改善

今後の benchmark では、
semantic completion だけでなく
`first write`、
user-facing diff、
support-only diff
も分けて観測する。
