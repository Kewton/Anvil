# v0.1.0 Release Acceptance Checklist

作成日: 2026-04-16
対象: `anvil` v0.1.0 local-first rebuild

## 目的

リリース前に、local LLM 実運用で最低限壊れていないことを人手で確認する。
`cargo test` の通過だけでなく、live Ollama 上で意味的に完遂できるかを確認する。

## 前提

- Ollama が localhost で起動している
- `qwen3:8b` 以上の tool-call 可能モデルが入っている
- 任意で sidecar model が入っている
- 作業ディレクトリは Git 管理下

## 0. 静的ゲート

以下が通ること。

```bash
cargo fmt --all
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo build --release
```

## 1. live Ollama E2E

ignored test を live Ollama で実行する。

```bash
cargo test --test e2e_local_llm -- --ignored --nocapture
```

期待結果:

- `live_ollama_can_write_a_file ... ok`
- temp workspace に `e2e-output.txt` が作られる
- 中身が `LOCAL_E2E_OK`

## 2. sidecar compaction

sidecar model を指定して対話を長くする。

```bash
ANVIL_SIDECAR_MODEL=qwen3.5:4b ./target/release/anvil -y
```

確認手順:

1. 長文の要望を数回入力して履歴を伸ばす
2. `/status` で `approx_tokens` が増えるのを確認
3. `/compact` を実行
4. `.anvil/sessions/session.json` を開く

期待結果:

- session に `[compact-summary]` が1つだけ残る
- 古い履歴が要約され、直近 tail は残る
- sidecar が失敗しても deterministic summary にフォールバックし、compact 自体は成功する

## 3. Plan / Act

```bash
./target/release/anvil -y
```

確認手順:

1. `/plan`
2. plan file に調査結果と手順を書かせる
3. template のまま `/approve` を試す
4. 実内容を持つ plan にして `/approve`

期待結果:

- `/plan` で `.anvil/plans/plan-*.md` が作られる
- template-only plan は `/approve` で拒否される
- substantive な plan は `/approve` で Act mode へ進む
- Git repo なら checkpoint が作られる

## 4. no-tool / empty response recovery

確認用の指示を与える。

例:

- `README.md に "acceptance-marker" を追加して`
- `src/main.rs を修正して help 文言を変えて`

期待結果:

- action request に対して、モデルが説明だけ返した場合は再促しが入る
- empty reply が続いた場合は無限ループせず失敗で止まる
- no-tool prose を 3 回以上繰り返さない

## 5. rollback

確認手順:

1. `/checkpoint before-change`
2. ファイルを書き換える
3. `/rollback`

期待結果:

- 変更が checkpoint 時点へ戻る
- untracked file も戻る

## 6. release shape

確認手順:

```bash
cargo build --release
ls -lh target/release/anvil
sed -n '1,220p' .github/workflows/release.yml
```

期待結果:

- `target/release/anvil` が生成される
- workflow が `anvil-*` artifact を gzip 化する
- tag push ベースの GitHub Release 作成フローが維持されている

## 合格条件

以下を満たしたら v0.1.0 の release candidate とみなす。

- 静的ゲート通過
- live Ollama E2E 通過
- sidecar compaction 通過
- Plan / Act 通過
- no-tool / empty response recovery が無限ループしない
- release artifact と workflow が壊れていない
