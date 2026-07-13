# Data Profile Contract (v0, fixed before B-2 implementation)

Status: fixed (2026-07-13). Changes to this contract are explicit
contract revisions and must be recorded in the mechanism ledger.

## 1. スコープ

対象タスク族: 表形式データ（CSV/TSV）の読み込み→クリーニング→集計→
レポート生成を行う単発パイプライン。入力はワークスペース内のファイル、
出力は pipeline/（再実行可能スクリプト）、output/results.json（全計算値の
機械可読版）、output/report.{html,md}（人間可読レポート）。

## 2. full の意味（最重要・不変条件）

full は「パイプラインが機械的に誠実である」ことを意味し、
「分析の示唆が業務的に正しい」ことを意味しない。後者は本プロファイルの
恒久的なスコープ外であり、assurance のいかなる階層でも主張しない。

## 3. 要求 evidence（full の必須ゲート）

E1 勘定照合 (reconciliation):
入力行数 = 採用行数 + 除外行数、かつ除外は理由別に計上され
output/results.json に記録されること。暗黙の行落ちゼロ。

E2 数値の束縛 (claims binding):
レポート本文中の全数値クレーム（数値・%・増減）が results.json の
対応値と機械照合で一致すること。レポートは計算結果からの補間のみを
許し、本文への数値の直書きを禁止する。照合不能な数値の存在は
claims_binding_violation として fail とする。

E3 再現性 (reproducibility):
pipeline/ の再実行が results.json と同一の値を生むこと
（決定性: seed固定・時刻非依存。非決定要素は fail）。

E4 スキーマ・アサーション:
manifest の [checks] で束縛された宣言的検証（型・範囲・重複キー・
日付境界等）がすべて pass すること。

## 4. assurance 階層

- full    = E1〜E4 全pass（実行プローブによる実測）
- partial = パイプライン実行成功＋E1/E3 pass だが E2 または E4 に未達
- static  = スクリプトは生成されたが実行プローブ未完（構文検証のみ）
- failed  = 実行失敗・E1違反（行の静かな欠落）・再現性違反

## 5. 実行プローブ

隔離実行（ネットワーク遮断・ワークスペース限定・タイムアウト有界）→
stdout/stderr/成果物の捕捉 → E1〜E4 の評価 → evidence/*.json に記録
（reconciliation.json / claims-binding.json / rerun-consistency.json）。
プローブは生成過程を参照せず、成果物のみを裁定する。

## 6. 偽装耐性（conformance ネガティブテストの要求）

- results.json に無い数値をレポートに書いた成果物が E2 で fail すること
- 除外行を計上しない成果物が E1 で fail すること
- 乱数/時刻依存の成果物が E3 で fail すること
- 未実行のプローブから full を投影できないこと（earned assurance 継承）

## 7. スコープ外（明示）

示唆・解釈の正しさ／統計手法の適切性／可視化の質／複数ファイル join の
意味論的正しさ（v0 では単一入力を基本とし、join はシナリオ族追加で
拡張する）。

## 8. 生成側への制約（契約由来のガイダンス）

パイプラインは決定的に書くこと（seed 固定・実行時刻への非依存・
反復順序の安定）。この要件は manifest の plan / guidance 文言に
契約由来のクラス知識として埋め込む。
