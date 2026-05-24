# 1. 現状を凍結して整理

## 目的

いま入っている変更を、次の 4 種類に分類する。

- **採用する修正**: 汎用制御として残す価値が高い。
- **条件付き採用**: runtime adapter としては有効だが、main path に置くと危険。
- **実験的・暫定的な修正**: 評価観測用に残してよいが、次の設計では主役にしない。
- **削除・降格候補**: 旧方式または特化ルールとして、段階的に production path から外す。

## 採用する修正

### FailurePacket / authority evidence を diagnostic payload に渡す

採用する。

理由:

- verifier output の raw text だけに依存しない。
- observed / expected、candidate artifacts、prior attempts を構造化できる。
- LLM diagnostic の入力を狭くできる。
- FastAPI / pytest 固有ではない。

注意:

- `FailurePacket` は authority ではなく evidence。
- verifier observation は「起きたこと」であり、「仕様の正しさ」ではない。

### `repair_authority.rs`

採用する。

理由:

- LLM の提案を authority として扱わない境界を作っている。
- `FixGeneratedTestExpectation` のような危険な repair を source of truth なしで拒否できる。
- `AllowedChangeKind` と target role の不一致を止められる。

改善点:

- `AuthorityEvidence` はまだ粗い。
- status code や response body など assertion-level authority までは表現できていない。
- v0.4.16 では `RepairPlan` validation の一部として再配置する。

### owned test artifact に bound した structured verifier

採用する。

理由:

- verifier が workspace 全体を勝手に scan しにくくなる。
- current task の test artifact と verifier の結びつきが明確になる。
- shell-shaped verifier command より安全。

注意:

- runtime ごとの verifier adapter は必要。
- ただし main decision logic に runtime 固有分岐を混ぜない。

## 条件付き採用

### structured Python pytest dependency setup

条件付きで採用する。

理由:

- `No module named pytest` で verifier 前に止まる問題を減らす。
- shell の `pip install && pytest` を直接実行するより構造化されている。

制約:

- `pyproject.toml` は untrusted artifact。
- dependency 名は必ず safe package name validation を通す。
- install は workspace 配下 `.anvil-state/verifier-python/site` に閉じ込める。
- network / package install は verifier policy として明示的に扱う。

降格条件:

- dependency setup が repair decision と混ざる場合は runtime adapter へ分離する。
- arbitrary dependency install に見える場合は safe stop にする。

### missing setup manifest candidate

条件付きで採用する。

理由:

- dependency_missing / config_or_verifier_error のときだけ `pyproject.toml` を候補に出すのは妥当。

制約:

- assertion failure では setup target を出さない。
- missing setup path は allowlist ではなく `RuntimeSetupCandidate` として表現する。
- まずは `pyproject.toml` のみだが、将来は runtime adapter が候補を返す形へ移す。

## 実験的・暫定的な修正

### Python/pytest framework findings

暫定扱い。

現状:

- pytest lifecycle mismatch
- pytest setup NameError
- shared state leak
- imported state rebind mismatch
- disconnected fixture state assertion

問題:

- `turn.rs` に大量の Python/pytest 解析関数が集まっている。
- failure meaning を deterministic pattern で解釈し始めている。
- 新しい失敗ごとに関数を増やす誘惑が強い。

方針:

- production main path からは外し、`RuntimeDiagnosticHintProvider` のような adapter に閉じ込める。
- diagnostic LLM への補助 evidence としてのみ使う。
- authority 判定には使わない。

### controller repair candidate for pytest setup

暫定扱い。

対象:

- missing `pytest` import
- bare setup state reset
- provider mutable state isolation

評価:

- 直近の smoke では一部前進に寄与した。
- ただし、この方向で増やすと rule-based repair が肥大化する。

方針:

- main path ではなく fallback / validator assist に降格する。
- `RepairPlan` が `test_bug` かつ setup/isolation repair を明示した場合のみ使う。
- 新しい pytest pattern は追加しない。

### observed assert expected literal update

暫定扱い。

問題:

- `201 vs 200` のような曖昧仕様で test expectation を直すべきか判断できない。
- literal の置換だけを見ると test weakening と区別しにくい。

方針:

- assertion-level authority がある場合だけ許可する。
- user request / README / BehaviorContract / public interface evidence と照合する。
- authority 不足なら patch せず actionable safe stop。

## 削除・降格候補

### `synthesized_missing_implementation_target_path_for_request`

降格候補。

現状:

- `FastAPI` / `Python` / `.py` を request から検出して `main.py` を返す。

問題:

- 特定 runtime / framework に寄っている。
- artifact target selection と task contract extraction が混ざっている。

方針:

- `TaskContract` または runtime adapter が target candidate を返す構造に置き換える。
- production path から request substring 判定を外す。

### `turn.rs` 内の runtime-specific parser 群

降格候補。

方針:

- `turn.rs` は repair lifecycle の orchestration に集中させる。
- Python/pytest parsing は runtime adapter module へ移す。
- adapter output は `DiagnosticHint` であり、authority ではない。

## FastAPI / ToDo / CRUD 特化の棚卸し

現時点で production path に直接 ToDo 専用テンプレートを出す処理は入れていない。

ただし、次は特化リスクとして扱う。

- request に `FastAPI` があると `main.py` を合成する処理。
- tests 内に FastAPI / todo / items を使った regression tests が多数あること。
- pytest state reset の deterministic repair。

結論:

FastAPI/ToDo 専用テンプレート化には戻っていない。
ただし、Python/pytest runtime 特化の補修が `turn.rs` に増えすぎており、汎用性の観点では整理が必要。

