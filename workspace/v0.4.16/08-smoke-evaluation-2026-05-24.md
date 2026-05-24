# v0.4.16 Smoke Evaluation 2026-05-24

## 条件

共通プロンプト:

```text
FastAPIでcrudのAPIを開発してください。使用方法をREADME.mdに記述してください。テストコードも実装してください。
```

共通コマンド:

```bash
anvildev -m qwen3.6:27b-coding-nvfp4 --sidecar-model qwen3.5:9b -y --fresh-session --no-footer --deterministic-fallback full --max-iterations 50
```

PAM なし:

```bash
ANVIL_PHOTON_ENABLED=false
```

PAM あり:

```bash
ANVIL_PHOTON_ENABLED=true
ANVIL_PHOTON_URL=http://127.0.0.1:18765
ANVIL_PHOTON_SHADOW_MODE=false
ANVIL_PHOTON_CANARY=100
ANVIL_PHOTON_TIMEOUT_MS=500
```

PAM health:

```json
{"status":"ok","schema_version":"action-memory.v1"}
```

## 結果サマリ

| condition | runs | success | failure |
| --- | ---: | ---: | ---: |
| PAMなし | 5 | 0 | 5 |
| PAMあり | 5 | 0 | 5 |
| 合計 | 10 | 0 | 10 |

成功率は 0/10。
PAM 有無で有意な改善は見えなかった。

## Run Details

| run | condition | result | iter | duration | edited count | terminal reason |
| --- | --- | --- | ---: | ---: | ---: | --- |
| `v0416_20260524_nopam_01` | PAMなし | fail | 17 | 682s | 1316 | `verifier_repair_pass_invalid: repair reply must contain a JSON object` |
| `v0416_20260524_nopam_02` | PAMなし | fail | 21 | 449s | 1278 | `verifier_repair_pass_invalid: repair reply must contain a JSON object` |
| `v0416_20260524_nopam_03` | PAMなし | fail | 21 | 414s | 1317 | `verifier_repair_pass_invalid: repair reply must contain a JSON object` |
| `v0416_20260524_nopam_04` | PAMなし | fail | 23 | 398s | 1317 | `verifier_repair_pass_invalid: repair reply must contain a JSON object` |
| `v0416_20260524_nopam_05` | PAMなし | fail | 27 | 495s | 1278 | verifier failed: 5 tests failed; `ItemResponse.price` rejected `None` |
| `v0416_20260524_pam_01` | PAMあり | fail | 23 | 373s | 1278 | `verifier_repair_pass_invalid: repair reply must contain a JSON object` |
| `v0416_20260524_pam_02` | PAMあり | fail | 23 | 376s | 1278 | `verifier_repair_pass_invalid: repair reply must contain a JSON object` |
| `v0416_20260524_pam_03` | PAMあり | fail | 21 | 369s | 1294 | `test edit requires SemanticRepairPlan ... none was constructed` |
| `v0416_20260524_pam_04` | PAMあり | fail | 24 | 455s | 1278 | `diagnostic_unavailable: diagnostic did not identify a safe repair target` |
| `v0416_20260524_pam_05` | PAMあり | fail | 29 | 426s | 1278 | `repair exhausted: no safe repair target remains` |

## 観測された構造的問題

### 1. Verifier 用 dependency sandbox が repo edits として数えられている

全 run で、最終 summary が以下のようになった。

```text
edited 1278-1317 files (.anvil-state/verifier-python/site/_pytest/__init__.py, ...)
```

これはユーザー成果物ではなく、verifier 用 Python site-package の展開物である。
にもかかわらず edited files に混ざっている。

影響:

- edited count が実成果物を表さない。
- changed candidates / diagnostic target selection / artifact ledger が汚染される可能性がある。
- verifier repair の対象選択が `.anvil-state` 由来のノイズに引っ張られる可能性がある。
- 評価ログ上も、実際に編集した成果物が見えにくい。

必要な対策:

- `.anvil-state/` は repo edit / artifact completion / changed candidate / edited summary から除外する。
- verifier sandbox は controller-owned runtime state として扱い、user workspace artifact とは別 ledger にする。

### 2. Repair pass が invalid proposal を繰り返し、safe stop までが遅い

多くの run で以下の繰り返しになった。

```text
Verifier repair
Rejected invalid controller repair proposal; retrying verifier repair with validation ...
```

v0.4.16 で invalid repair を即適用しない方向には進んだが、次の判断がまだ弱い。

- 同じ failure family を十分早く諦めない。
- `RepairJob::next_action()` が production dispatch の唯一の判断元ではない。
- invalid patch -> replan / different target / safe stop の切替がまだ旧分岐に残っている。

必要な対策:

- `RepairJob::next_action()` を verifier repair dispatch の SSOT にする。
- `PatchRejected` の target/key/reason ごとの budget を production path で使う。
- 同一 target / 同一 rejection family が一定回数続いたら、同じ provider に再要求しない。

### 3. Diagnostic plan と patch proposal の境界がまだ完全には閉じていない

PAM あり run で以下が出た。

```text
test edit requires SemanticRepairPlan (spec_authority + repair_hypothesis); none was constructed
```

これは safety gate としては正しい。
ただし、repair pass に進む前に `AcceptedRepairPlan` がない状態を terminal / re-diagnostic に落とせていない。

必要な対策:

- patch provider は必ず `AcceptedRepairPlan` を入力にする。
- `AcceptedRepairPlan` がない場合は patch attempt ではなく `RequestDiagnostic` または `SafeStop` にする。
- legacy validator の late reject に頼らず、dispatch 前に止める。

### 4. PAM は今回の failure mode には効いていない

PAM ありでも 0/5。
failure reason は PAM なしと同系統で、PAM context が main repair control を改善した形跡はない。

解釈:

- 今回の支配的な失敗は memory 不足ではない。
- verifier sandbox の成果物混入、repair dispatch の未統合、invalid proposal の収束性不足が主因。
- PAM は authority ではなく advisory なので、この種の controller bug を解決できない。

## 現時点の評価

v0.4.16 の方向性自体は妥当。
ただし現時点の実装は、まだ production flow を安定化する段階に達していない。

改善した点:

- invalid repair を適用せず reject する挙動は確認できた。
- verifier diagnostic -> repair pass へ進むところまでは安定して到達している。
- unsafe / invalid test edit を通して green にする挙動は見えていない。

未達の点:

- `done` に到達しない。
- actionable safe stop として短く止まらない。
- `.anvil-state` が user repo edits として混ざる。
- same invalid repair family の繰り返しを早く止められない。

## 次の優先順位

1. `.anvil-state/` を user artifact / repo edit / changed candidate から完全に除外する。
2. `AcceptedRepairPlan` なしでは patch provider を呼ばないように production dispatch を閉じる。
3. `RepairJob::next_action()` を verifier repair の dispatch SSOT にする。
4. invalid proposal の repeated reject budget を provider / target / reason 単位で適用する。
5. 上記後に PAM なし 3-5 回 smoke を再実施する。
