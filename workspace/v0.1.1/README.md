# workspace/v0.1.1

このディレクトリは、**2026-04-27 時点の実装コード**を読んで整理した
Anvil の現行設計メモである。

`Cargo.toml` 上の package version はまだ `0.1.0` だが、
ここでは「次に引き継げる整理済みの理解」を `v0.1.1` としてまとめる。

## ファイル構成

- `01_current_design_philosophy.md`
  - 現在の設計思想と、何を優先している実装なのか
- `02_current_architecture.md`
  - レイヤー構成、主要モジュール、実行フロー
- `03_runtime_controls_and_tradeoffs.md`
  - 回復制御、安全境界、運用上の特徴とトレードオフ

## 先に一言でいうと

現在の Anvil は、

- **local-first / Ollama 専用**
- **Plan/Act を表に出した対話型ランタイム**
- **ローカル LLM の失敗を runtime 制御で吸収する実装**
- **TUI / session / safety まで含めて一体設計された CLI**

として整理できる。
