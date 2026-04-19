# vibe-local の、ローカルLLM特化として強い点

作成日: 2026-04-16
対象: `/Users/maenokota/share/work/github_kewton/vibe-local`

参照:
- `/Users/maenokota/share/work/github_kewton/vibe-local/README.md`
- `/Users/maenokota/share/work/github_kewton/vibe-local/vibe-local.sh`
- `/Users/maenokota/share/work/github_kewton/vibe-local/vibe-coder.py`
- `/Users/maenokota/share/work/github_kewton/vibe-local/tests/test_vibe_coder.py`

## 1. 一言でいうと

`vibe-local` は、

- Ollama 前提
- オフライン前提
- 単一ファイル前提
- 依存最小前提
- ローカルLLMの癖を前提にした prompt / tool / TUI を持つ

という意味で、最初から「ローカルLLMで現実に動かす」ことに重心を置いている。

## 2. ローカルLLM特化として強い特徴

### 2.1 構造が小さい

README の通り、コアは `vibe-coder.py` 単一ファイルで、stdlib only を徹底している。

強み:

- 挙動の追跡がしやすい
- 修正箇所が見えやすい
- ローカルLLM向けのチューニングを一か所で完結しやすい
- 依存関係起因のトラブルが少ない

これは「ローカルLLM用に prompt / tool / TUI / session を一体で最適化する」うえでかなり有利。

### 2.2 Ollama 直結が基本

README と `vibe-local.sh` では、通常経路が `vibe-local -> vibe-coder.py -> Ollama` になっている。

強み:

- provider abstraction の層が薄い
- ローカルモデル固有の癖を直接扱いやすい
- 接続、モデル一覧、pull、起動確認まで一貫してローカル向けに実装できる

Anvil より「抽象化のための抽象化」が少ない。

### 2.3 ローカルLLM前提の prompt 方針がはっきりしている

`anthropic-ollama-proxy.py` にある `LOCAL_SYSTEM_PROMPT` や、`vibe-coder.py` の system prompt 群は、かなり実務寄りにローカルLLMの失敗を先回りしている。

代表例:

- TOOL FIRST
- 先に説明しない
- ユーザーにコマンドを投げない
- 失敗時は別手段を試す
- `<think>` を出さない
- Qwen 系の XML tool call も受ける

強み:

- local model の典型的な逸脱に対する防波堤が明確
- 「きれいな抽象化」より「現実に従う prompt」を優先している

### 2.4 sidecar model の使い方が現実的

README と `vibe-coder.py` では sidecar model を、permission checks, summaries, compaction など軽い処理に使う設計になっている。

強み:

- 重いモデルを毎回使わない
- local 環境の速度低下を抑えやすい
- main model を本筋の生成に集中させやすい

Anvil でも sidecar はあるが、vibe-local はより「軽い仕事を軽いモデルに逃がす」思想が前面に出ている。

### 2.5 モデル自動選択がローカル運用に寄っている

`Config` は RAM とインストール済みモデルを見て main / sidecar を選ぶ。

強み:

- 初回導入時の摩擦が小さい
- ローカルPCごとの差異を吸収しやすい
- ユーザーが毎回適切なモデルを考えなくてよい

ローカルLLMでは「良いデフォルト」が成功率に直結する。

### 2.6 ツール面が実用に寄っている

README 記載の built-in tools は、ローカル作業に必要なものへかなり寄せてある。

- Bash
- Read
- Write
- Edit
- Glob
- Grep
- WebFetch
- WebSearch
- NotebookEdit
- SubAgent
- ParallelAgents
- AskUserQuestion

加えて `ALLOWED_TOOLS` のような絞り込み思想が見える。

強み:

- 「ローカルで本当に使う道具」に寄っている
- ツール群が user task と結びついている
- local model の混乱を避ける意図がある

### 2.7 Qwen 互換の XML fallback を持つ

`vibe-coder.py` には XML tool call extraction がある。

強み:

- local model が function calling を崩しても拾える
- Qwen 系や Ollama 周りの実際の癖に合わせている

これは local LLM 特化としてかなり重要。きれいな API だけを信じていない。

### 2.8 Plan/Act がシンプル

README と `vibe-coder.py` では、

- Plan mode は read-only
- Act mode は実行
- `/approve` で移る
- `/rollback` で戻す

という形で、意味がわかりやすい。

強み:

- local model に対する役割の切替が明快
- UI と mental model が一致している
- 複雑な phase machine を外から説明しやすい

### 2.9 AutoTest / FileWatcher / GitCheckpoint がローカル運用に馴染む

`vibe-coder.py` には

- AutoTestRunner
- FileWatcher
- GitCheckpoint

があり、編集後の local feedback loop を回す設計がある。

強み:

- LLM の自己修復サイクルを短くできる
- 外部変更を拾える
- rollback が早い

ローカルLLMは1回の正答率がクラウドより落ちるため、この「短い修復ループ」は重要。

### 2.10 TUI がローカル利用に優しい

README と `vibe-coder.py` には以下がある。

- DECSTBM 固定フッター
- ESC 即時停止
- type-ahead input
- scroll debug
- TUI debug log

強み:

- 体感が軽い
- ローカル生成の待ち時間に耐えやすい
- デバッグしやすい

local LLM は cloud より待ち時間が長くなりやすいため、TUI 品質の価値が高い。

### 2.11 セキュリティも「ローカルらしい」

`vibe-local.sh` と `anthropic-ollama-proxy.py` では、`OLLAMA_HOST` を localhost 限定で検証している。

強み:

- SSRF 的な誤設定を減らせる
- offline/local utility としての前提を保てる

local-first を言うだけでなく、境界もそれに合わせている。

### 2.12 テスト密度が高い

README では 787 tests と明記され、`tests/test_vibe_coder.py` はかなり広い範囲をカバーしている。

強み:

- 単一ファイルでも回帰を抑えやすい
- local 向けの小さな挙動変更を素早く検証できる

## 3. vibe-local の思想として特に見習うべき点

### 3.1 まずローカルで動くことを優先している

- cloud 互換より Ollama 実動作
- 理想的設計より現実的 fallback
- 拡張性より導入容易性

### 3.2 小さく作って、その中で完結している

- 単一ファイル
- 単純な起動導線
- 設定、モデル選択、TUI、session、tools が近い

### 3.3 local model の弱さを前提にしている

- XML fallback
- sidecar
- tool-first guidance
- auto-test feedback
- ESC / type-ahead / compact session

## 4. Rust 移植時に継承すべき要素

移植で特に継承すべきなのは以下。

- Ollama 直結を基本経路にする
- local model 向け prompt 方針を最初から埋め込む
- function calling 失敗時 fallback を持つ
- model auto-detect + sidecar auto-pick を入れる
- Plan/Act を明快にする
- AutoTest / FileWatcher / GitCheckpoint を core workflow に組み込む
- TUI を local 待ち時間向けに作る
- 構造を小さく保つ

## 5. 結論

`vibe-local` の強さは、機能量そのものより、

- ローカルLLMの制約を受け入れていること
- 失敗しやすい地点に現実的 fallback を置いていること
- 設計の重心が「抽象化」ではなく「実動作」にあること

にある。

Rust 移植版でも、ここを外すと単なる「Anvil を別言語で書き直したもの」になってしまう。狙うべきはそうではなく、「vibe-local の local-first な骨格を Rust で再構成したもの」。
