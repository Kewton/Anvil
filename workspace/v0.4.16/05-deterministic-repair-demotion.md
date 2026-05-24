# 5. 旧 deterministic repair を段階的に降格

## 方針

旧 deterministic repair をすぐ全削除しない。

ただし、production main path からは段階的に外す。

理由:

- 既存処理の一部は安全境界として有効。
- 一部 runtime-specific repair は短期的に成功率へ寄与している。
- いきなり削除すると regression の原因を追いにくい。

一方で、main path に残し続けると rule-based repair が肥大化する。

## 分類

### 残すもの

安全境界として残す。

- path confinement
- workspace scope admission
- ownership gate
- role mismatch check
- test weakening detection
- assertion deletion detection
- dependency/config safety check
- verifier command structure validation

これらは LLM を信頼しないために必要。

### adapter に移すもの

runtime hint として残す。

- Python import path validation
- pytest output の collection/setup/runtime failure hints
- Python package marker detection
- Python structured verifier setup

これらは authority ではなく hint。

### fallback に降格するもの

accepted `RepairStep` がある場合だけ使う。

- missing `pytest` import 追加
- provider state reset fixture の補修
- observed literal expectation update

制約:

- target / allowed change kind / authority を自分で決めない。
- `RepairStep` に従属する。
- patch admission を必ず通す。

### 削除候補

削除または置換する。

- request substring から `main.py` を直接合成する処理。
- `turn.rs` 内に散らばる runtime-specific diagnostic parser。
- assertion-level authority を見ずに test expectation を直す経路。

## 降格手順

### Phase 1: 観測

既存 deterministic repair が発火したら telemetry に記録する。

記録:

- repair kind
- target path
- accepted `RepairPlan` id
- whether fallback or main path
- patch admission result
- verifier delta

### Phase 2: 接続変更

deterministic repair を `PatchProposalProvider` として接続する。

入力:

- accepted `RepairStep`
- target file contents
- bounded runtime hint

出力:

- patch proposal
- または no proposal

### Phase 3: main path から外す

deterministic repair が target / authority / intent を選ぶ経路を止める。

`RepairJob::next_action()` が選んだ step に対してのみ発火する。

### Phase 4: 削除判定

削除条件:

- 新 pipeline で同等以上の verified success / safe stop が出る。
- 旧 repair がなくても state transition tests が通る。
- 旧 repair が安全境界ではなく pattern patch に過ぎない。
- 同じ機能を LLM patch proposal + admission で表現できる。

残す条件:

- LLM output の検証に必要。
- path / role / security の境界として機能する。
- runtime adapter として小さく閉じ込められる。

## セキュリティ観点

deterministic repair は安全に見えるが、実際には危険になり得る。

例:

- test の assert を削る。
- generated test の期待値を勝手に implementation に合わせる。
- dependency install を無制限に許可する。
- user-controlled README を authority と誤認する。

したがって deterministic であっても、authority validation と patch admission を必ず通す。

## v0.4.16 の実装優先度

1. deterministic repair 発火箇所を inventory 化する。
2. `RepairStep` なしで発火する repair を telemetry 付きで警告する。
3. pytest setup repair を fallback provider に降格する。
4. observed literal update を assertion-level authority 必須にする。
5. request substring target synthesis を task contract / runtime adapter 経由に置き換える。

