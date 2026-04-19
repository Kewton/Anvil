# v0.1.0 Phase E Live Validation

作成日: 2026-04-16
対象: `anvil` v0.1.0 local-first rebuild

## 目的

Phase C/D で small loop と core test inventory へ寄せた実装が、
live Ollama 上でも意味的に完遂できるかを確認する。

## 実施内容

### 1-run smoke

実行コマンド:

```bash
cargo test --test e2e_local_llm live_ollama_can_semantically_edit_and_start_nextjs -- --ignored --nocapture
```

結果:

- `ok`
- 完了時間: `55.19s`

確認内容:

- Next.js scaffold 成功
- `src/app/page.tsx` が marker を含む形へ編集される
- 初期テンプレート文言を含まない
- `.anvil/sessions/session.json` が保存される
- `3011` 起動確認が通る

### 5-run benchmark

実行方法:

- semantic Next.js E2E を 5 回連続実行
- 個別ログは `/tmp/anvil-phase-e/run-{1..5}.log`

結果:

- `5/5 passed`

各 run:

- run 1: `55.93s`
- run 2: `41.72s`
- run 3: `47.30s`
- run 4: `122.64s`
- run 5: `62.44s`

## 観測結果

- 現在の small loop で semantic Next.js E2E は完走する
- scaffold 止まりではなく、実際に `page.tsx` 編集まで到達する
- `3011` 起動確認も 5/5 で通過した
- 実行時間のばらつきはまだある
  - 特に run 4 は `122.64s`
  - run 4 と run 5 では `over 60 seconds` の長時間警告が出ている

## 結論

Phase E の目的は達成した。

- 1-run smoke は通過
- 5-run benchmark も `5/5` 通過
- v0.1.0 コアは、少なくとも semantic Next.js E2E では「scaffold だけで止まる」状態を脱している

## 次の論点

- 実行時間のばらつきの抑制
- `qwen3.5:122b` / `qwen3.5:9b` 条件での再計測
- release candidate としての最終手動受け入れ
