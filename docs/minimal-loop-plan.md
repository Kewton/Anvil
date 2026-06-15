# Anvil Minimal Loop 移行計画（Codex 実行用 v2）

このドキュメントは AI コーディングエージェント（Codex 等）に直接読み込ませて実行させることを前提に書かれている。
人間のレビュアーとエージェントの共通の参照点であり、リポジトリの `docs/minimal-loop-plan.md` に置く。
Phase 3/4 の triage・admission・ablation・eval report の運用規律は
`docs/minimal-loop-operating-discipline.md` を参照する。
現行実装の責務分割と思想は `docs/minimal-loop-architecture.md` を参照する。

---

## 0. エージェントへの実行指示（最初に読むこと）

- **作業前に §2「現状認識」の各事実を必ず自分でコードを読んで再検証すること。** 本ドキュメント作成時点のリビジョンは `46b06b0` であり、参照はシンボル名で行っている。コードが変わっていて事実が成立しない場合は、作業を進めず人間に報告する
- **1 タスク = 1 PR。** 各タスクの「完了条件」をすべて満たすまで次のタスクに進まない
- **§4 の Non-Goals に該当する変更は、たとえ改善に見えても行わない**
- 「人間タスク」と明記されたタスク（ベンチマーク実行など、Ollama とローカル GPU が必要なもの）は実行を試みず、依頼メッセージを出力して停止する
- 設計判断に迷ったら §5 のデザイン・プリンシプルに照らす。原則同士が衝突したら番号の若い方が優先

---

## 1. 背景と目的

### 背景

Anvil は Ollama 直結の local-first コーディングエージェントである。ローカル LLM（8B〜30B クラス）が苦手とする tool call 崩れ・no-edit ループ・検証不足・コンテキスト汚染を、CLI 側のプロトコルと安全ガードで支えることを方針としてきた。

しかし開発が進むにつれ、`src/agent/loop_run/` 配下だけで約 200 モジュール、`repair_job.rs` 約 1 万行、`task_contract.rs` 約 8 千行という規模に膨張した。モデルの失敗のたびに「決定論的な制御とプロンプト注入を追加する」対応を重ねた結果である。

分析の結果、この方向には構造的欠陥があると結論した。**小型モデルはコンテキスト内の指示・制約が増えるほど追従率が下がる**ため、「失敗 → テキスト制約追加 → さらに失敗」という負のスパイラルが発生している。さらに、ガード機構自体が会話履歴を汚染し、当初解決しようとした「コンテキスト汚染」を悪化させている（§2 参照）。

### 目的

同一リポジトリ内に**最小構成のエージェントループ（minimal loop）を新設**し、ベンチマークで両者を並走比較しながら、**実証された機構だけ**を旧ループ（legacy）から移植する。最終的に minimal をデフォルトとし、legacy を削除する。

ゼロからの書き直しではない理由: `src/ollama/`、`src/tools/`、`src/session/store.rs` 等の基盤は品質が高く再利用すべき資産であり、また旧 loop_run の各機構は「ローカル LLM の実際の失敗事例カタログ」として移植判断の参考価値があるため。

---

## 2. 現状認識（コード上の事実と、それが引き起こす問題）

エージェントは着手前に以下を自分で確認すること。各項目は「事実（コード参照）→ 引き起こす問題」の形式。

### F1. コンパクションがメッセージ数で発火する

- **事実**: `src/agent/loop_run/lifecycle.rs` の `should_compact` は `messages.len() > keep_tail + 4` **または** トークン超過で発火する。`DEFAULT_KEEP_TAIL = 24`（`src/agent/loop_run.rs`）なので、履歴が 29 通を超えると トークン予算（デフォルト 24,000）をほぼ使っていなくても圧縮される
- **問題**: エージェントループではツール呼び出し 1 回で assistant + tool の 2 メッセージ増えるため、1〜2 ターンで閾値を超え、ほぼ常時コンパクションが走る。直前に Read したファイル内容や Edit のアンカー文字列が要約で失われ、モデルが編集対象を見失う。**README が課題として挙げる「no-edit ループ」の主因はガード不足ではなくこの圧縮設計**と分析している

### F2. コンパクションで「ノイズが生き残り、シグナルが消える」

- **事実**: `src/session/compact.rs` の `compact_messages_with_strategy` は、head 部分について `head.retain(|m| m.role == "system" && !is_compact_summary(m))` を行う。つまり蓄積した system ノート（リカバリー指示等）は**全件永久保存**され、user/assistant/tool の実作業内容だけが最大 4,000 文字の要約 1 本に潰される。要約はサイドカーモデル（timeout 8 秒、max_predict 384、220 語以内指示）で生成され、失敗時は決定論的要約にフォールバックする
- **問題**: セッションが長くなるほどプロンプトがメタ指示の山になる。設計意図（「構造化状態を履歴ノイズから分離」）と実装が逆転している

### F3. 1 リクエストあたりの注入が小型モデルの追従容量を超えている

- **事実**: `src/agent/loop_run/build_request_messages.rs` の `build_request_messages` は、system prompt（`src/system_prompt.rs`、CORE RULES 19 項目 + profile/stage 別ルール）に加え、mode policy / objective contract / runtime capability / contract-bound generation / test expectation audit / working memory / case memory / anti-pattern / repo context / scaffold・recovery ノート等、**最大 10 本前後の独立した system メッセージ**を 1 リクエストに積む
- **問題**: 8B クラスのモデルは同時制約数の増加で全制約の遵守率が急落する。また会話途中の system ロールは Ollama の chat template によってレンダリングがモデルごとに揺れる。さらに system prompt が stage/policy で毎ターン変化するため Ollama の prefix cache が効かず、毎回フル prefill となり遅い（遅延 → timeout → リトライの二次被害もある）

### F4. 出力上限がタスク要求と衝突している

- **事実**: `src/ollama/client.rs` のデフォルトは `num_ctx = 24_000`、`num_predict = 2_048`
- **問題**: `benchmarks/heavy-space-invaders.yaml` が要求する数百行のコンポーネント生成は 2,048 トークンで確実に途中切断される。切断 → truncated-tool-call リカバリー → 強制 small edit → Edit の完全一致アンカー探し（小型モデルが最も苦手。しかも F1 によりファイル抜粋が消えている）という最悪経路に入る

### F5. ガードの応答が「さらにテキスト制約を足す」一方向

- **事実**: リカバリーノート（`src/agent/recovery.rs` に約 30 種）は `push_system_note` で**セッション履歴に永続化**される。repair/verifier/contract 機構がターンごとの許可ツールを絞り、追加ノートを注入する
- **問題**: F2 と組み合わさり、失敗するほど恒久的なノイズが増える。`lower.contains("modepolicy")` のようなキーワードヒューリスティックも多数あり誤発火する

### F6. 計測が複雑性に追いついていない

- **事実**: `benchmarks/` のシナリオは 3 本のみ。一方で制御フローは 23 万行
- **問題**: どの機構が純増益でどれが純害かを判定する手段がない。改善ループが成立していない

### 既存の好材料（活用する）

- `scripts/bench.sh` には `--no-precautions` / `--no-case-memory` / `--no-auto-test` の ablation フラグが既にある（環境変数 `ANVIL_NO_REMINDER` 等）
- `bench.sh` は llm-io.jsonl ログを常時保存する設計になっている
- `OllamaClient::clone_with_overrides` で `num_predict` をループ側から変更する経路が既にある

---

## 3. 課題の整理（問題 → 課題 → 対応フェーズ）

| # | 問題（§2） | 課題 | 対応 |
| --- | --- | --- | --- |
| 1 | F1, F2 | 作業文脈を破壊しないトークンベースのコンパクションが必要 | Phase 1: T1-4 |
| 2 | F2, F5 | リカバリー指示を履歴に永続化しない仕組みが必要 | Phase 1: T1-5 |
| 3 | F3 | system メッセージを 1 本に統合し、注入を最大 4 ブロックに制限 | Phase 1: T1-2, T1-3 |
| 4 | F4 | num_predict のタスク適応的な引き上げ | Phase 1: T1-6 |
| 5 | F6 | engine 比較が可能なベンチ基盤（20 本以上 + 自動成功判定） | Phase 0, Phase 2 |
| 6 | F5 全般 | 機構追加に歯止めをかける運用ルール | §5 プリンシプル + Phase 3 admission 基準 |

---

## 4. Goals / Non-Goals

### Goals

1. `anvil --engine minimal` で動作する最小ループ（コア 1,000 行以内）を新設する
2. legacy と minimal を同一ベンチハーネスで比較できる状態を作る
3. ベンチで実証された機構のみを minimal に移植する
4. 最終的に minimal をデフォルト化し、legacy を削除する

### Non-Goals（エージェントは以下を行わないこと）

1. **旧 `loop_run` 配下のリファクタリング・改善・バグ修正**（比較対象を動かさないため。明示的な人間の指示がある場合を除く）
2. **multi-provider 化**（Ollama 専用の方針は維持）
3. **新しいガード・分類器・verifier の「予防的」追加**（§5 P2 違反）
4. **`src/ollama/`、`src/tools/`、`src/session/store.rs`、`src/safety/` の API 変更**（再利用資産。追加は可、破壊的変更は不可）
5. **TUI の拡張、MCP transport の実装**（README の Not implemented 方針を維持）
6. Phase 2 完了（ベンチ比較データの取得）前に Phase 3（機構移植）へ進むこと

---

## 5. デザイン・プリンシプル

すべての設計判断・PR・移植判断はこの原則に照らす。衝突時は番号の若い方が優先。

- **P1. トークン予算が第一の資源である。** プロンプトへの注入を増やす変更は「注入トークン数」と「ベンチ成功率の変化」をセットで提示する。実証できない注入は入れない
- **P2. ガードレールは「ベンチでの敗北」によってのみ獲得される。** 「念のため」での機構追加を禁止。追加できるのは、再現可能な失敗シナリオが存在し、その機構が改善を示せた場合のみ。旧 loop_run からの移植も同基準
- **P3. コンパクションではシグナルが生き残り、ノイズが死ぬ。** 保持優先度は「ユーザーの目標 > 直近の作業内容（Read 結果・編集アンカー）> 過去の要約 > メタ指示」。リカバリー注入は ephemeral（次の 1 リクエストのみ、履歴非永続）を原則とする
- **P4. system メッセージは常に 1 本。** ターン固有のフィードバックは user ロールで渡す。system prompt はターン間で不変に保ち、prefix cache を効かせる
- **P5. 決定論でできることをモデルに頼まない。モデルの仕事を決定論で縛らない。** パス検証・出力切り詰め・checkpoint は Rust 側で黙って行い、プロンプトでルール化しない。コード生成の判断はモデルに任せる
- **P6. すべての機構は単独でオフにできる。** ablation 比較ができない機構はマージしない
- **P7. minimal loop のコアは 1,000 行以内。** 1 リクエストの注入ブロックは最大 4（system prompt / ephemeral フィードバック / working memory 相当 / ユーザーメッセージ＋履歴）
- **P8. 計測なき改善は改善ではない。** 全リクエストの最終プロンプト・トークン数・注入内訳をログする。性能の主張はベンチ（最低 20 シナリオ × 5 runs）でのみ行う

---

## 6. アーキテクチャ

### 再利用（変更しない）

| モジュール | 用途 |
| --- | --- |
| `src/ollama/client.rs` | chat / streaming / `clone_with_overrides` |
| `src/ollama/xml_fallback.rs` | `strip_think_tags` / `extract_tool_calls` / `normalize_tool_call_arguments` |
| `src/ollama/parsing.rs` | レスポンスパース |
| `src/tools/`（registry 含む） | ツール実行・path guard・`truncate_output` |
| `src/session/store.rs` | `ConversationMessage` / セッション永続化 |
| `src/safety/` | path guard / host validation |

### 新設

```
src/agent/minimal_loop/
  mod.rs        # 公開エントリ run_session(...) のみ
  loop.rs       # メインループ（目標 ~200 行）
  prompt.rs     # 固定 system prompt 1 本（ルール 10 個以内 + tool catalog）
  compact.rs    # トークンベースコンパクション（P3 準拠の新実装。session/compact.rs は流用しない）
  feedback.rs   # ephemeral フィードバック（最大 1 ブロック、履歴非永続）
```

切替: CLI `--engine minimal|legacy`（デフォルト legacy）。

### minimal loop 初期仕様

1. system prompt は 1 本・固定・ターン間不変。ルール 10 個以内（tool-first / 同言語応答 / sudo 禁止 / 小さく進める / 失敗時は別手段、程度）
2. コンパクション発火は `approximate_token_count > context_budget * 0.7` のみ。メッセージ数条件は持たない。保持優先度は P3 に従う
3. ephemeral フィードバックは 3 種のみ: 空応答 / tool call なし / Edit アンカー不一致。いずれも次リクエストの user ロールに 1 ブロック、履歴非永続
4. `num_predict` デフォルト 8,192（config 化、minimal engine のみ）。Write 切断時は「続きを Edit で追記」フィードバック 1 種で対応
5. Plan/Act は維持するが、Plan は read-only 制約のみ（stage 制御・plan 構造強制なし）
6. verifier / contract / repair / case-memory / anti-pattern / photon は**初期版に含めない**

---

## 7. 作業計画

担当区分: 【A】= エージェント実行可、【H】= 人間タスク（Ollama + ローカル GPU 必須。エージェントは依頼を出力して停止）

### Phase 0: 計測基盤

- [ ] **T0-1【A】** llm-io ログの検証・拡充
  - 1 リクエストごとに「最終プロンプト全文・概算トークン数・注入ブロック内訳」が取れているか既存ログ実装を確認し、不足分を `src/logging.rs` 経由で追加
  - 完了条件: ログスキーマを `docs/eval/llm-io-schema.md` に記述し、unit test でスキーマを固定
- [ ] **T0-2【A】** ベンチ成功判定の自動化
  - シナリオ yaml に `success_check`（ファイル存在 / コマンド終了コード / 行数下限）を定義可能にし、`bench.sh` / `report.py` で自動判定
  - 完了条件: heavy-space-invaders に success_check を付与し、dry-run でなく判定ロジックの unit test が通る
- [ ] **T0-3【H】** legacy ベースライン取得: 現行 3 シナリオ × 2–3 モデル × 5 runs。結果を `workspace/baseline-YYYYMMDD/` にコミット
- [ ] **T0-4【H】（推奨）** 既存 ablation フラグ（`--no-precautions --no-case-memory --no-auto-test`）での予備比較

### Phase 1: minimal loop 骨格

- [ ] **T1-1【A】** `--engine` フラグ追加（`src/cli.rs` / `src/config.rs`）。デフォルト legacy
  - 完了条件: フラグ未指定時の挙動が完全に不変（既存テスト全通過）。`--engine minimal` は「not implemented」エラーを返す空実装
- [ ] **T1-2【A】** `prompt.rs`: 固定 system prompt
  - 完了条件: ルール数 ≤ 10。スナップショットテストで全文固定。動的要素は tool catalog と作業ルートパスのみ
- [ ] **T1-3【A】** `loop.rs`: メインループ
  - native tool call → パース失敗時に `xml_fallback` へセッション内ダウングレード（legacy の `disable_native_tools_for_session` の簡略版）
  - ツール実行は `src/tools/registry` を直接呼ぶ。終了条件は「tool call なしの応答」または max_iterations
  - 完了条件: モック client での統合テスト（正常系 / fallback 系 / max_iterations 系）が通る
- [ ] **T1-4【A】** `compact.rs`: トークンベースコンパクション
  - 完了条件: 「直近の Read 結果と編集対象抜粋が要約より優先して残る」「メッセージ数だけでは発火しない」「古い system ノートは保存されない」を unit test で固定
- [ ] **T1-5【A】** `feedback.rs`: ephemeral フィードバック 3 種
  - 完了条件: フィードバックがセッション store に永続化されないことを test で固定
- [ ] **T1-6【A】** `num_predict` の config 化（minimal のみデフォルト 8,192）
- [ ] **T1-7【A】** session 互換: minimal のセッションが `sessions list/show` で表示される
- [ ] **T1-8【H】** 手動スモーク: `anvil --engine minimal` で heavy-space-invaders を 1 回完走（成功は問わない）

### Phase 2: ベンチ拡充と並走比較

- [ ] **T2-1【A】** シナリオを 20–30 本に拡充（success_check 必須）。カテゴリ配分:
  新規ファイル生成（小/大）×5、既存コード修正 ×8、複数ファイル横断 ×4、scaffold ×3、長セッション（コンパクション強制発火）×3、非コーディング ×2
- [ ] **T2-2【A】** `bench.sh` に `--engines legacy,minimal` のマトリクス実行を追加
- [ ] **T2-3【A】** `compare.py` に engine 間比較（成功率 / イテレーション数 / 総トークン / 失敗カテゴリ）を追加
- [ ] **T2-4【H】** 初回フル比較を実行し、結果を `docs/eval/` にコミット
- **ゲート: T2-4 の比較データなしに Phase 3 へ進んではならない**

### Phase 3: 実証ベースの選択的移植（1 機構 = 1 PR）

PR の admission 基準（PR テンプレ化すること）:

1. minimal が負ける再現可能なシナリオ ID を明記
2. 移植機構の注入トークン数を明記
3. 移植後ベンチで該当シナリオが改善し、他カテゴリが悪化しないこと【H】
4. 個別オフフラグがあること（P6）
5. minimal_loop コアが 1,000 行以内であること（P7）

移植候補の予想（基準を満たすまで入れない）: truncated tool call 検出と継続指示 / scaffold root 自動検出 / working memory 最小版

### Phase 4: 切替と削減

- [ ] デフォルト engine を minimal へ → 2–3 リリース後に legacy `loop_run` と依存テスト群を削除 → README を本ドキュメントの方針に更新

---

## 8. リスクと対策

| リスク | 対策 |
| --- | --- |
| 移植フェーズで機構を持ち込みすぎ再膨張 | Phase 3 admission 基準の機械的運用。P2/P7 をレビューで引用 |
| エージェントがベンチ未取得のまま移植に進む | §4 Non-Goal 6 + Phase 2 ゲートで二重に禁止 |
| エージェントが旧コードを「ついでに」改善 | §4 Non-Goal 1 で明示禁止 |
| ベンチの環境揺らぎ | runs ≥ 5、モデル・量子化を記録、比較は同一環境内のみ |
| minimal が初回比較で大敗 | 想定内。負けカテゴリが移植優先順位リストになるため計画は崩れない |

---

## 9. 着手順（最初の 1 週間）

1. T0-1（ログ検証）→ 2. T1-1（フラグ空実装。以降の PR を小さく保つ）→ 3. T0-2（success_check）→ 4. T1-2 + T1-3 → 5. T2-1 のシナリオ作成に並行着手（コード不要のため）

人間側は並行して T0-3（ベースライン取得）を実行する。
