# 2. 目標を再定義

## 現在の問題

現状は「verifier が通れば成功」「通らなければ repair を続ける」という見え方になりやすい。

しかしローカル LLM では、常に verifier pass まで無理に進めると危険がある。

- generated test を弱めて green にする。
- 曖昧仕様を勝手に決める。
- 同じ無効 repair を繰り返す。
- verifier failure を本質的に解釈せず、target だけ変える。

そのため、完了条件を再定義する。

## 正しい完了状態

v0.4.16 では、完了状態を次の 2 種類に分ける。

### 1. Verified Done

条件:

- required artifacts が揃っている。
- verifier が成功している。
- verifier command は current task の owned artifacts に bound されている。
- 直前の repair が安全境界を通っている。

これは従来の成功。

### 2. Actionable Safe Stop

条件:

- verifier は失敗している。
- ただし、Anvil が安全に自動修復できない理由を構造化して説明できる。
- 次に人間または上位 agent が判断すべき選択肢が明確である。
- repo を危険な状態にする test weakening や speculative patch をしていない。

例:

- `POST should return 200 or 201` がユーザー要件や README から判断できない。
- generated test と implementation のどちらが正しいか assertion-level authority が不足している。
- dependency install が必要だが、network / package policy が許可されていない。
- patch proposal が validator で連続 reject され、別 target も安全に選べない。

## 失敗状態

次は失敗として扱う。

- verifier failed の raw output だけで終了する。
- `assistant stopped before repairing` のように制御理由が不明。
- 同じ target / same intent / same rejection を繰り返す。
- test expectation を根拠なく書き換えて green にする。
- evaluator workdir 外の既存 artifacts を current task 成果物として拾う。
- LLM / PAM / README / verifier output を authority として無検証に使う。

## 成功率の指標

評価では次を分けて記録する。

| metric | meaning |
| --- | --- |
| verified_success_rate | verifier pass で完了した割合 |
| actionable_safe_stop_rate | 理由付きで安全停止した割合 |
| unsafe_or_unactionable_failure_rate | 理由不明、無限 retry、危険 patch など |
| repair_convergence_rate | verifier failure 後に有効な次状態へ進めた割合 |
| repeated_invalid_repair_rate | 同じ reject を繰り返した割合 |

目標はまず `verified_success_rate` だけではなく、`verified_success_rate + actionable_safe_stop_rate` を上げること。

## なぜ safe stop を成功とは分けるか

safe stop を成功に混ぜると、agent が実用に耐えるかを見誤る。

そのため表示上は別にする。

- `done`: verifier pass
- `safe_stop`: 自動修復を止めたが、理由と次 action が明確
- `failed`: 制御不能または非安全

この区別により、品質評価が「何回 green になったか」だけではなくなる。

