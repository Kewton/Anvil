# v0.1.0 Smoke Then 5-Run Procedure

作成日: 2026-04-17
対象: `anvil` v0.1.0 local-first rebuild

## 目的

新しい対策を 1 つ入れたときに、
必ず同じ順番で評価するための具体手順を固定する。

原則は次のとおり。

1. `1-run smoke` を回す
2. 効果が見えた場合だけ `5-run` を回す
3. 効果が無ければ差分を戻す
4. 効果があればコミットして次の対策へ進む

## 評価の基本ルール

- 1 回に試す対策は 1 つだけ
- 複数の改善を同時に混ぜない
- `smoke` で改善が見えないものは `5-run` に進めない
- `5-run` の結果は必ず保存し、次の比較基準にする

## 1-run smoke の目的

`smoke` は「その対策が狙った局面に効いているか」を最短で確認するためのもの。

heavy 最終課題を 5 回回す前に、次を確認する。

- transport error が減ったか
- `Write/Edit` 到達が改善したか
- 無駄な `Bash` / `Read` 反復が減ったか
- `rc=0` の質が悪化していないか

## 1-run smoke の実行方法

### A. first-write benchmark を使う場合

最初の `Write/Edit` 到達率を見たいときは、次を使う。

```bash
ANVIL_E2E_MODEL='qwen3.5:122b' \
cargo test --test e2e_local_llm live_ollama_reaches_first_write_on_scaffolded_nextjs -- --ignored --nocapture
```

用途:

- initial-turn 改善
- transport retry/backoff 改善
- planner/actor 軽量化
- first write 到達率の確認

### B. heavy benchmark を 1 回だけ回す場合

最終課題に近い挙動を見たいときは、手動実行で 1 回だけ回す。

```bash
WORKDIR=/tmp/anvil-heavy-smoke-$(date +%Y%m%d-%H%M%S)
mkdir -p "$WORKDIR"

./target/release/anvil \
  --cwd "$WORKDIR" \
  --model 'qwen3.6:35b-a3b' \
  --sidecar-model 'qwen3.5:9b' \
  --max-iterations 40 \
  --chat-retries 2 \
  --debug \
  --yes \
  --oneshot \
  -p "あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとしてTDDで開発してください。" \
  > "$WORKDIR/anvil.stdout.log" 2>&1
```

確認すべきもの:

- `$WORKDIR` 内に `package.json` / `src/app/page.tsx` 等が生成されたか
- `.anvil/logs/llm-io.jsonl` に `Write` / `Edit` ツール呼び出しがあるか
- `anvil.stdout.log` の末尾に transport error / `/api/chat 500` が出ていないか

用途:

- heavy 実運用に対する収束性確認
- support file 側への拡散確認
- `page.tsx` 相当の user-facing file に diff が入るか確認

## smoke の判定基準

### 進めてよい

次のどれかが明確に改善した場合だけ `5-run` に進める。

- `Write/Edit` 到達率が上がった
- `transport error` / `500` の発生が減った
- install loop や探索 loop が減った
- user-facing file への diff が増えた
- support file 側への拡散が減った

### 進めない

次なら `5-run` に進めず、差分を戻す。

- 狙った局面に入る前に同じ失敗で止まる
- `Write/Edit` 到達が変わらない
- `max iterations` や prose stop が増える
- support file 側へ流れるだけで質が改善しない
- 速度だけ変わって意味的進捗が変わらない

## 5-run の目的

`5-run` は「たまたま 1 回良かった」を排除し、
再現性を確認するためのもの。

見るべきもの:

- 成功率
- app 生成率
- `3011` 起動率
- `Write/Edit` 到達率
- user-facing file diff 率
- support-only diff 率
- test file / test script 率
- `500` / transport error / prose stop / max iterations の内訳

## 5-run の実行方法

### A. first-write benchmark の 5-run

```bash
OUTDIR=/tmp/anvil-firstwrite-5run-$(date +%Y%m%d-%H%M%S)
mkdir -p "$OUTDIR"
: > "$OUTDIR/summary.tsv"
printf 'run\trc\telapsed\n' >> "$OUTDIR/summary.tsv"

for i in 1 2 3 4 5; do
  LOG="$OUTDIR/run-$i.log"
  ANVIL_E2E_MODEL='qwen3.5:122b' \
  cargo test --test e2e_local_llm \
    live_ollama_reaches_first_write_on_scaffolded_nextjs \
    -- --ignored --nocapture > "$LOG" 2>&1
  RC=$?
  ELAPSED=$(rg -o 'finished in [0-9.]+s' "$LOG" | tail -n1 | sed 's/finished in //')
  printf '%s\t%s\t%s\n' "$i" "$RC" "$ELAPSED" >> "$OUTDIR/summary.tsv"
done

cat "$OUTDIR/summary.tsv"
```

### B. heavy benchmark の 5-run

```bash
OUTDIR=/tmp/anvil-heavy-5run-$(date +%Y%m%d-%H%M%S)
mkdir -p "$OUTDIR"
: > "$OUTDIR/summary.tsv"
printf 'run\trc\telapsed\n' >> "$OUTDIR/summary.tsv"

for i in 1 2 3 4 5; do
  RUNDIR="$OUTDIR/$i"
  mkdir -p "$RUNDIR"
  START=$(date +%s)

  ./target/release/anvil \
    --cwd "$RUNDIR" \
    --model 'qwen3.6:35b-a3b' \
    --sidecar-model 'qwen3.5:9b' \
    --max-iterations 40 \
    --chat-retries 2 \
    --debug \
    --yes \
    --oneshot \
    -p "あなたが考える最高に面白くかっこいいスペースインベーダーゲームを3011ポートで起動可能なnext.jsアプリとしてTDDで開発してください。" \
    > "$RUNDIR/anvil.stdout.log" 2>&1
  RC=$?

  END=$(date +%s)
  ELAPSED=$((END - START))
  printf '%s\t%s\t%ss\n' "$i" "$RC" "$ELAPSED" >> "$OUTDIR/summary.tsv"
done

cat "$OUTDIR/summary.tsv"
```

## 5-run 後に必ず見るもの

### 1. summary

- `summary.tsv`
- 必要なら `summary.corrected.tsv`

### 2. 各 run の終端

- `anvil.stdout.log`
- `.anvil/logs/llm-io.jsonl`

### 3. 生成物

- user-facing file が変わっているか
- support file だけ増えていないか
- test file / test script があるか
- dev 起動できるか

## 効果あり / なしの判断

### 効果あり

次を満たしたら、その対策は採用候補。

- `smoke` で狙った指標が改善
- `5-run` でも改善傾向が再現
- 悪化した指標が無いか、軽微

その場合は:

1. 差分を保持する
2. コミットする
3. 次の対策へ進む

### 効果なし

次なら不採用。

- `smoke` で改善が見えない
- `5-run` で再現しない
- 成功率より質が悪化する
- user-facing 実装より support-only diff が増える

その場合は:

1. 差分を戻す
2. ワークツリーを clean にする
3. 次の対策へ進む

## 実運用での注意

- `5-run` は必ず同じ条件で比較する
- model を変えた場合は比較対象を分ける
- `rc=0` だけを成功とみなさない
- scaffold / dev 起動だけで良しとしない
- user-facing diff を必ず見る

## 現時点での推奨フロー

現在の `v0.1.0` では、次の順で回すのが最も安定している。

1. `cargo fmt --all`
2. `cargo test --all`
3. `cargo clippy --all-targets -- -D warnings`
4. `1-run smoke`
5. 改善が見えたら `5-run`
6. 効果ありならコミット
7. 効果なしなら差分を戻す

この手順を崩すと、
「たまたま通った 1 回」と
「再現する改善」
が区別できなくなる。
