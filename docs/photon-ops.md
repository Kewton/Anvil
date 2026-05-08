# Photon サイドカー連携 運用手順

## クイックスタート

```bash
# 1. photon サイドカーを起動（photon-action-memory リポジトリを参照）
photon-action-memory serve --port 3030

# 2. Anvil をフル送信モードで起動
ANVIL_PHOTON_ENABLED=true \
ANVIL_PHOTON_SHADOW_MODE=false \
ANVIL_PHOTON_CANARY=1000 \
anvil

# 3. ログで送信確認
SESSION=$(ls -t ~/.local/state/anvil/sessions/ | head -1)
jq 'select(.event | startswith("agent.photon"))' \
  ~/.local/state/anvil/sessions/$SESSION/logs/llm-io.jsonl
```

---

## 1. photon sidecar の起動方法

[photon-action-memory](https://github.com/Kewton/photon-action-memory) リポジトリのドキュメントを参照して起動する。

デフォルトポートは `3030`。Anvil は `http://127.0.0.1:3030` に接続する。

起動後のヘルスチェック:

```bash
curl http://localhost:3030/health
# {"ok":true} が返れば正常
```

利用可能なエンドポイント:

| エンドポイント | メソッド | 用途 |
|--------------|---------|------|
| `/health` | GET | 死活確認 |
| `/v1/context/pack` | POST | context_pack 送信 |
| `/v1/evaluate` | POST | turn 評価 |

---

## 2. Anvil 側 env vars

以下の環境変数または `.anvil/config` で設定する。

| 変数名 | デフォルト | 有効範囲 | 説明 |
|--------|-----------|---------|------|
| `ANVIL_PHOTON_ENABLED` | `false` | `true` / `false` | photon 連携を有効化する（`--offline` 時は強制 false） |
| `ANVIL_PHOTON_URL` | `http://127.0.0.1:3030` | localhost / 127.0.0.1 / ::1 + http/https のみ | photon サーバーの URL |
| `ANVIL_PHOTON_SHADOW_MODE` | `true` | `true` / `false` | shadow mode（後述） |
| `ANVIL_PHOTON_CANARY` | `0` | `0`–`1000` | canary サンプリング率（permille） |
| `ANVIL_PHOTON_TIMEOUT_MS` | `200` | `1`–`60000` | HTTP リクエストタイムアウト（ミリ秒） |

`.anvil/config` 例:

```toml
photon_enabled = true
photon_url = "http://127.0.0.1:3030"
photon_shadow_mode = true
photon_canary = 100
photon_timeout_ms = 500
```

---

## 3. shadow mode の確認方法

`ANVIL_PHOTON_SHADOW_MODE` はデフォルト `true`（safe default）。

**2 経路の動作の違い（重要）**:

| 経路 | shadow_mode=true | shadow_mode=false |
|------|-----------------|------------------|
| pre-turn hook（プロンプト注入） | HTTP **SKIP**（context_pack を送信しない） | canary gate → 通過時に送信してプロンプトに注入 |
| mapper（build_request_messages） | **常時送信**（fire-and-forget、response discarded） | canary gate → 通過時に送信 |

shadow_mode=true はプロンプト注入を無効化しつつ、mapper 経路でのデータ収集は継続する。
shadow_mode=false にすると context_pack の結果が実際のプロンプトに注入される。

ログで確認:

```bash
# shadow_mode=true のとき pre-turn が skip されていることを確認
jq 'select(.event == "agent.photon_context_pack.skipped") | .payload.reason' \
  ~/.local/state/anvil/sessions/$SESSION/logs/llm-io.jsonl
# → "shadow_mode" が返れば正常

# shadow_mode=false で注入が行われた場合
jq 'select(.event == "agent.photon_context_pack.completed") | .payload.injected_bytes' \
  ~/.local/state/anvil/sessions/$SESSION/logs/llm-io.jsonl
```

---

## 4. canary mode の確認方法

`ANVIL_PHOTON_CANARY` はデフォルト `0`（常時スキップ）。

| canary 値 | 動作 |
|-----------|------|
| `0` | 常時スキップ（HTTP 送信なし） |
| `1`–`999` | ターン単位の確率的サンプリング |
| `1000` | 常時送信 |

サンプリングはターン単位で決定論的: SHA-256(session_id + turn_index) の下位 8 バイトを permille 値 (0–999) に変換し、`canary` 値未満なら送信する。

ログで canary 制御を確認:

```bash
# canary_gate でスキップされたターン数を確認
jq 'select(.event == "agent.photon_context_pack.skipped" and .payload.reason == "canary_gate") | .payload.turn_index' \
  ~/.local/state/anvil/sessions/$SESSION/logs/llm-io.jsonl | wc -l
```

---

## 5. ログの見方

ログは XDG_STATE_HOME 準拠のパスに出力される:

```
~/.local/state/anvil/sessions/<session_id>/logs/llm-io.jsonl   # イベントログ
~/.local/state/anvil/sessions/<session_id>/logs/eval.jsonl     # turn-level 評価ログ（photon eval 含む）
```

`XDG_STATE_HOME` が設定されている場合は `$XDG_STATE_HOME/anvil/sessions/...` になる。

**photon 関連イベント**:

| イベント | 内容 |
|---------|------|
| `agent.photon_context_pack.skipped` | context_pack 送信スキップ（`payload.reason`: `shadow_mode` / `canary_gate`） |
| `agent.photon_context_pack.completed` | context_pack 送信完了（`failed` / `truncated` / `injected_bytes` / `duration_ms`） |
| `agent.photon_evaluate.skipped` | evaluate スキップ |
| `agent.photon_evaluate.completed` | evaluate 完了 |

jq 例:

```bash
# photon イベント一覧
jq 'select(.event | startswith("agent.photon"))' \
  ~/.local/state/anvil/sessions/$SESSION/logs/llm-io.jsonl

# context_pack が注入されたターンの bytes 量
jq 'select(.event == "agent.photon_context_pack.completed" and .payload.failed == false) | {turn: .payload.turn_index, bytes: .payload.injected_bytes}' \
  ~/.local/state/anvil/sessions/$SESSION/logs/llm-io.jsonl

# eval.jsonl で photon 評価結果を確認
jq '.photon_eval' \
  ~/.local/state/anvil/sessions/$SESSION/logs/eval.jsonl
```

---

## 6. photon に送信されるデータ

context_pack には **2 つの独立した送信経路**がある。

### 経路 1: pre-turn hook（プロンプト注入用）

| 項目 | 内容 |
|------|------|
| トリガー | shadow_mode=false かつ canary gate 通過 |
| ペイロード | `session_id`、`turn_index` のみ（minimal） |
| shadow_mode=true | **送信しない**（HTTP SKIP） |
| レスポンス | `render_context_pack` でフィルタ・サニタイズ後にプロンプト注入 |

### 経路 2: mapper（データ収集用）

| 項目 | 内容 |
|------|------|
| トリガー | canary gate 通過（shadow_mode=true は常時通過） |
| ペイロード | `task`、`repo_path`、`working_memory`、`touched_files`、`recent_tool_summary`、`selected_case_ids`、`selected_anti_pattern_ids`、`selected_precaution_ids` |
| shadow_mode=true | **常時送信**（fire-and-forget、response discarded） |
| レスポンス | 無視（fire-and-forget） |

### マスク処理

両経路とも、送信前に以下のマスク処理が適用される:

- `mask_secrets`: トークン・KV・URL 形式のシークレット文字列を `***` に置換
- `mask_payload_inplace`: ペイロード全体に対する最終防衛線マスク

これはシークレット情報の**除去**であり匿名化ではない。非シークレットの機微情報は送信される可能性がある。

mapper 経路では追加の保護として:
- `touched_files` の `..` コンポーネント・絶対パスを除去
- `RecentToolCall.name` を `[A-Za-z0-9_-]` 以外は `_` に置換

---

## 7. 障害時の切り分け

### fail-open

photon HTTP 通信が失敗しても Anvil は通常通り動作を継続する（fail-open）。
エラーは `tracing::warn!` でログに記録される。

### photon を強制無効化

```bash
# --offline フラグで photon_enabled を強制 false に
anvil --offline

# または env var で明示的に無効化
ANVIL_PHOTON_ENABLED=false anvil
```

### 疎通確認

```bash
# photon サーバーへの HTTP 疎通確認
curl -s http://localhost:3030/health | jq .

# context_pack エンドポイントの確認
curl -s -X POST http://localhost:3030/v1/context/pack \
  -H 'Content-Type: application/json' \
  -d '{"session_id":"test","turn_index":0}' | jq .
```

### ログで確認

```bash
# photon 関連の warn ログ確認（tracing warn は stderr に出る）
anvil 2>&1 | grep -i photon

# photon イベントの失敗フラグを確認
jq 'select(.event | startswith("agent.photon")) | select(.payload.failed == true)' \
  ~/.local/state/anvil/sessions/$SESSION/logs/llm-io.jsonl
```

### photon 関連テストの実行

```bash
cargo test --lib photon
cargo test --test config_tests photon
cargo test --test photon_client_smoke
cargo test --test photon_mapper_smoke
cargo test --test photon_prompt_smoke
cargo test --test photon_turn_hook_smoke
cargo test --test photon_eval_log_smoke
cargo test --test photon_schema_smoke
cargo test --test photon_fixture_smoke
```
