# anvil bench metrics

本ドキュメントは `scripts/analyze_run.py` が出力する標準 metric JSON の仕様、計算方法、バージョン管理ポリシーを定義する。

## 1. 概要

`scripts/analyze_run.py <run-dir>` は、`scripts/bench.sh` が生成する run-dir を入力として標準 metric JSON を stdout に出力する。

- python3 >= 3.10 / 標準ライブラリのみ
- stdout は辞書順キーの JSON（1 オブジェクト）
- `schema_version: 1` を含む

## 2. run-dir 構造

```
<run-dir>/
├── session.json              必須
├── meta.json                 任意（なければ rc/elapsed_s = null）
├── stdout.log                任意（未使用）
├── workdir/                  任意
│   └── src/app/page.tsx      任意（なければ page_tsx_* = null）
└── logs/
    └── llm-io.jsonl          任意（なければ error_500_count = null）
```

## 3. 出力 schema (v1)

```json
{
  "compact_events": 2,
  "elapsed_s": 120,
  "error_500_count": 1,
  "files_modified": ["src/app/page.tsx", "src/lib/game.ts"],
  "iter_count": 2,
  "keywords_version": 1,
  "page_tsx_has_game_keywords": true,
  "page_tsx_touched": true,
  "rc": 0,
  "run_id": "test-session-id",
  "schema_version": 1,
  "tool_calls": {"Bash": 1, "Edit": 1, "Read": 1, "Write": 1},
  "we_total": 2,
  "xml_parser_errors": null
}
```

### 安定性仕様

- キーは辞書順（ASCII ソート）
- 取得不可な指標は `null`（キー欠損でなく `null`）
- 整数は int、秒数は int
- `schema_version: 1`、`keywords_version: 1` を含む

## 4. 指標定義

| 指標 | 計算式 / 取得先 |
|------|---------------|
| `run_id` | `session.json["id"]`（存在しない、または空文字列なら `null`） |
| `schema_version` | 固定値 `1` |
| `keywords_version` | 固定値 `1` |
| `rc` | `meta.json["rc"]`（なければ `null`） |
| `elapsed_s` | `meta.json["elapsed_s"]`（int、なければ `null`） |
| `iter_count` | `len([m for m in messages if m["role"] == "assistant"])` |
| `tool_calls` | `{name: count}` — role==assistant の tool_calls を name でカウント（動的キー） |
| `we_total` | `sum(tool_calls.get(t, 0) for t in WRITE_EDIT_TOOLS)` |
| `error_500_count` | `llm-io.jsonl` の `event=="ollama.generate.error" && payload.kind=="status" && payload.status==500` 件数 |
| `xml_parser_errors` | 常に `null`（未計測 — 将来 schema v2 で実装予定） |
| `compact_events` | `len([m for m in messages if m["role"]=="system" and m["content"].startswith("[compact-summary]")])` |
| `files_modified` | role==assistant の Write/Edit tool_call の `arguments.path`（正規化・dedup） |
| `page_tsx_touched` | `"src/app/page.tsx" in files_modified`（厳密一致） |
| `page_tsx_has_game_keywords` | `workdir/src/app/page.tsx` 内容に game_keywords のいずれかが含まれれば `true` |

### 書き込み系ツール定義

```python
WRITE_EDIT_TOOLS = ["Write", "Edit"]
```

将来 `MultiEdit` 等が追加される場合は本リストを更新し `schema_version` をバンプする。

### game_keywords (v1 固定)

```python
GAME_KEYWORDS_V1 = [
    "game", "score", "lives", "bullet", "enemy",
    "ship", "invader", "player", "level", "wave"
]
```

`keywords_version: 1` として出力。キーワード変更時は `keywords_version` をインクリメント。

### compact_events の依存注記

`compact_events` は `src/session/compact.rs` の `[compact-summary]` マーカーに依存する。
Rust 側でマーカー文字列を変更する際は `analyze_run.py` と本ドキュメント、fixture を同時に更新すること。

### files_modified 正規化アルゴリズム

1. 絶対パス → `workdir` 相対に変換（workdir 外は除外）
2. 相対パス先頭 1 セグメントが workdir basename と一致する場合は除去
3. `..` を含むパスは除外
4. 変換後に dedup（初出順に保持）

### xml_parser_errors の位置付け

`xml_parser_errors` は schema v1 に含まれるが常に `null` を返す「未計測」フィールド。
`null` は「0 件」ではなく「計測不可」を意味する。
Rust 側 (`src/ollama/xml_fallback.rs`) に structured log が実装された時点で schema v2 にバンプして実際の値を返す。

## 5. meta.json フォーマット（bench.sh 書き出し）

```json
{
  "rc": 0,
  "elapsed_s": 120,
  "model": "qwen3:30b",
  "start_ts": "2025-01-01T00:00:00Z"
}
```

## 6. exit code 仕様

| exit code | 意味 |
|-----------|------|
| 0 | 正常終了（部分的なファイル欠落・破損は null として許容） |
| 1 | 引数エラー（run-dir 未指定） |
| 2 | run-dir が存在しない、または session.json が見つからない／symlink／不正 |
| 3 | session.json の JSON パースエラー、必須キー `messages` 欠損、またはサイズ上限超過 |

### 部分失敗の扱い（exit 0 + null フォールバック）

| 状況 | 振る舞い |
|------|---------|
| `meta.json` が存在しない / 破損 / 上限超過 | `rc`, `elapsed_s` を `null` |
| `llm-io.jsonl` が存在しない / 上限超過 | `error_500_count` を `null` |
| `llm-io.jsonl` に破損行 | 当該行をスキップ、他は継続 |
| `workdir/src/app/page.tsx` が読み取り不可 / 上限超過 | `page_tsx_*` を `null` |
| `session.json` の `id` が欠損 / 空文字列 | `run_id = null` |
| 任意ファイルが symlink / special file / 上限超過 | 当該 optional 指標を `null` |

## 7. 読み取りサイズ上限

| ファイル | 上限 | 超過時 |
|---------|------|-------|
| `session.json` | 10 MiB | exit 3 |
| `meta.json` | 256 KiB | `rc`, `elapsed_s = null` |
| `workdir/src/app/page.tsx` | 2 MiB | `page_tsx_* = null` |
| `logs/llm-io.jsonl` | (streaming) 1 行 1 MiB | 異常行スキップ、上限超過時 `error_500_count = null` |

## 8. セキュリティ境界

- run-dir は「半信頼入力」として扱う
- `session.json` は canonical path と regular file を検証（失敗時 exit 2）
- `meta.json` / `llm-io.jsonl` / `workdir/src/app/page.tsx` は、canonicalize 後に期待ディレクトリ配下でない、symlink、または regular file でない場合は「存在しない」と同様に扱う
- `files_modified` に workdir 外・絶対パス・親ディレクトリ遡りを含むパスを出力しない

## 9. バージョン管理ポリシー

| 変更種別 | バージョン処理 |
|---------|--------------|
| 破壊的変更（キー削除・型変更） | `schema_version` をインクリメント |
| 後方互換の追加 | `schema_version` 据え置き |
| キーワード変更 | `keywords_version` をインクリメント |

変更時は以下を同時に更新する:
- `scripts/analyze_run.py`
- 本 `docs/metrics.md` のバージョン表
- `tests/golden/` の golden fixture
