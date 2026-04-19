# v0.1.0 Heavy Task Experiments And Lessons

作成日: 2026-04-19
対象: `anvil` v0.1.0、heavy 条件(スペースインベーダー + Next.js + TDD)の収束性改善

参照:
- `08_runtime_findings_and_problem_statement.md`
- `09_smoke_then_5run_procedure.md`

## 目的

2 日間で 20 件以上の改修を試し、heavy 条件(user-facing UI まで届くか)を改善した記録。採用/非採用の結果と、そこから得た教訓を残す。

## 成果サマリ

### 到達した水準(`d0c3e97` + N1' on qwen3.5:122b)

| 指標 | 作業前 | 現在 |
|---|---|---|
| rc=0 完走率 | 0/5 | 2-3/5(変動あり) |
| `/api/chat` 500 致命 | 2-8/run | **0/5**(構造的消失) |
| W+E 合計 | ~13 | 20-27 |
| **`page.tsx` がゲーム実装に書き換わる** | **0/5** | **3/5**(122b)、1/5(35b-a3b) |
| `/lib/` に独立したゲームロジック | rare | 2/5(122b の run 3, 5 で完全実装) |

### commit された改修(8 本)

| commit | 効果 |
|---|---|
| `f5e6b85` | first-write test infra 修正(init_logging バグ) |
| `d531efb` | project root 絶対パス常時注入(P2) |
| `1301ba9` | Planner/Actor 分離撤廃(P4) |
| `83de356` | orchestration dead code 削除(A1) |
| `eed3c00` | findings + smoke/5-run procedure 追加 |
| `2bc1cdf` | Ollama 5xx/429 を transport error として retry(B2) |
| `2006277` | `/api/chat` → `/api/generate` raw + XML fallback 強化(N1') |
| `d0c3e97` | vibe-local `LOCAL_SYSTEM_PROMPT` を移植、WRONG/RIGHT example 追加 |

## 試行と結果一覧(全 20+ 件)

### 採用された改修(◯)

- `P2`: runtime_context_messages で work_root 絶対パスを常時注入。model の path hallucination を激減。
- `P4`: handle_user_message の Planner 呼び出し + Verifier-driven restart を撤去。一本道の actor loop に戻す。
- `A1`: orchestration.rs の dead code(ActorPlan, FailureClass, strip_restart_heavy_messages 等)整理。
- `B2`: `is_transport_error` に `Ollama /api/chat failed: 5xx` と `429` を追加、extra_transport_retries を 0→2 / 2→4 に増量。
- `N1'`: endpoint を `/api/chat` から `/api/generate` raw=true + ChatML に切替。server-side XML parser を構造的に bypass、500 完全消失。
- `vibe-local prompt`: LOCAL_SYSTEM_PROMPT を移植(17 rule + WRONG/RIGHT example)。page.tsx 初到達。

### 棄却された改修(×、全て revert)

勝因特定のための実験が多数。以下カテゴリ別に記録。

#### loop 末端の retry gate 系

| 改修 | 内容 | 結果 |
|---|---|---|
| `P3` prose-stop retry | "let me…" で離脱する agent を retry で矯正 | prose を繰り返し iter 消費、max_iter 到達で悪化 |
| `U1` deliverables contract | 最初に agent に成果物を宣言させ最後に照合 | 宣言は行われるが UI を含まない。宣言内容に引きずられる |
| `U2` end-of-session self-audit | 完了直前に振り返りを強制 | audit 後 agent が Write/Edit 必須引数を忘れて全 error |
| `U3` sidecar goal-check | sidecar に「主要 deliverable 実装済み?」を問う | repo_change_retries 先に枯渇、発動せず |
| `U4` prompt-term coverage | user prompt の内容語が成果物に反映されたか | max_iter 到達で発動せず |

→ **共通失敗: retry note を増やしても model は同じ config/test-first 行動を繰り返す**。

#### state machine / 予防ガード系

| 改修 | 結果 |
|---|---|
| `P1` Planner に context | ハルシネーションは直るが path 失敗が支配 |
| `B3` compaction 閾値緩和 | context 肥大で 500 頻発 2→4 |
| `B1` rework ブロック(同一 path+content Write 抑止) | 10 runs 中 0 回発動、数値改善は変動ノイズ |
| `D1` user goal anchor(compaction 時) | anchor 発火するも outcome 最悪 |

→ **共通失敗: model の事前分布バイアスは後付けガードで上書きできない**。

#### briefing 系(layer 追加型)

| 改修 | 結果 |
|---|---|
| `V1` Next.js briefing を system note で注入 | 発火するも "src/lib/ に書く許可証" として解釈され逆効果 |
| `L2` workspace briefing(marker 検出 + 注入) | smoke 1/1 ok だが 5-run で W+E 23→7 悪化 |
| `L4` implementation briefing | 発動せず(条件に到達前に離脱) |
| `A 実験` tool schema を raw ChatML に埋込み | **page.tsx 3/5→0/5(完全退行)**、仮説どおり "production mode" に切替わるが page.tsx neglect |
| `B 実験` /api/chat に戻し + anti-XML prompt | rc=0 0/5、page.tsx 0/5(完全失敗) |

→ **共通失敗: system prompt 本体に既存の指示と重複する briefing は dilution または "許可証" 誤解釈で逆効果**。

#### プロンプト軽量変更

| 改修 | 結果 |
|---|---|
| TDD 削除(user prompt から "TDDで" 除去) | layout.tsx meta は 2/5 で編集されるが page.tsx は 0/5 のまま |

## 教訓(L1-L10)

### L1: **retry gate は local LLM の挙動を変えない**

- 失敗検出後に system note で再指示しても、model は同じパターンを繰り返す
- P3-re, U1-U4, 6 件の retry 系で一貫して確認
- **根本原因: model のバイアスは訓練時の事前分布に由来し、1 turn の prompt では上書きできない**

### L2: **state machine / 予防ガードは挙動を劣化させがち**

- Planner/Verifier、compaction threshold、rework ブロック、goal anchor など「制御を足す」系は全て revert
- **"直すための状態機械" より "失敗しにくい一本道"**(doc 01 の原則)が再確認された

### L3: **briefing は事実提供、命令ではない**

- rule("Do X")は訓練と衝突すると無視
- briefing("X is a Next.js project, page.tsx is the UI entry")は事実として受容される
- ただし **後付け briefing は既存 system prompt との重複で dilution する**
- **system prompt 本体に組み込まれた briefing が最強**(vibe-local 移植で実証)

### L4: **endpoint 選択は model の挙動モードを決定する(本日最大の発見)**

```
/api/chat + tools=[]  → "production mode"
                        → components/utils/types/tests を分離、page.tsx neglect
/api/generate + raw   → "simple mode"
                        → flat 構造、page.tsx 直行
```

- A 実験(tool schema を raw に埋込み)で production mode に切り替わり page.tsx 3/5 → 0/5
- vibe-local(/api/chat)も同じ現象(smoke 1/1 成功だが 5-run で 1/5)
- **prompt 工夫より endpoint 選択の方が影響が大きい**

### L5: **transport 層と挙動モードはトレードオフ**

- `/api/chat`: 500 リスクあり / 構造化能力あり
- `/api/generate` raw: 500 ゼロ / 構造化能力低い
- **"動く UI" を優先するなら N1' が正解**
- "production 品質のコードベース" を優先するなら `/api/chat`

### L6: **Ollama の server-side XML parser がクリティカル**

- `/api/chat` に tools=[] で request すると、Ollama は model の出力を server-side で tool_calls に解析
- qwen3.6:35b-a3b などは heavy context で `</function>` 等の破損 XML を出す
- これが "XML syntax error: unexpected end element `</function>`" 500 の真因
- **client 側で tools を送らなくても、Ollama は model template で勝手に解析を試みる**(N1 実験で確認)
- 構造的回避は `/api/generate` へ移る必要あり(N1')

### L7: **Model スケールはレバレッジが大きい**

- qwen3.6:35b-a3b: page.tsx 1/5
- qwen3.5:122b: page.tsx **3/5**
- **同じ system prompt で 3 倍の到達率**
- prompt 工夫で 0→1 を達成するより、model 切替で 1→3 が得られる方が安上がり(コード変更ゼロ)
- コスト: 122b は 35b-a3b の約 2.3x の推論時間(500→920s)

### L8: **"local LLM + sophisticated prompt" の限界**

- 18 件の矯正試行すべてで「agent が間違った後に retry/briefing で直す」は失敗
- **"最初から正しい方向へ" しか効かない**
- その手段は: system prompt 本体の書換(L1)、endpoint 選択(L4)、model 選択(L7)

### L9: **Observation ≠ Briefing**

- 当初 "briefing = domain knowledge 注入" と仮説立てた
- L4 設計で "live observation(現在 page.tsx は template)" が別カテゴリだと気付いた
- Observation は **file system の事実**を注入するので、model がそれを推論材料にできる
- 今回 Phase A' で部分実装したが完成せず。**次回検討対象**

### L10: **測定系は常に疑え**

- 初日の最大の投資対効果: **first-write test の `init_logging` バグ発見と修正**
- これが直るまで全ての改修の効果測定が不能だった
- 改修より先に **測定の健全性を検証する**

## 決定的な対照: vibe-local と Anvil の差分

同じ model(qwen3.5:122b)、ほぼ同じ system prompt でも構造が違う理由:

| | vibe-local | Anvil(d0c3e97 + N1') |
|---|---|---|
| endpoint | `/api/chat` with tools | `/api/generate` raw |
| tool schema 露出 | Ollama が structured で注入 | 我々が text 列挙 |
| 挙動モード | production | simple |
| **page.tsx 到達率** | **1/5** | **3/5** |
| 構造成熟度(components/tests/types) | 高 | 低 |
| 500 error | 0 | 0 |
| 500 回避手段 | prompt で "Do NOT output XML" | endpoint で bypass |
| rc=0 率 | 5/5 | 2-3/5 |

**結論: vibe-local が構造化能力で勝り、Anvil が user-facing 成果物の到達で勝る**。両者は異なる最適点にいる。

## 本日確定した "効く改修の共通属性"

1. **model に "事実" を提供する**(rule 羅列でなく)
2. **最初から効かせる**(後付け retry ではない)
3. **構造は単純に保つ**(state machine を増やさない)
4. **endpoint / model 選択という "大きい変数" を優先的に動かす**(prompt tweak より効果大)
5. **測定の健全性を疑う**

## 現時点の未解決課題

### page.tsx 到達率 3/5 の上限

残る 2/5 は:
- rc=1 で途中離脱(repo_change_retries 枯渇)
- UI shell のみで完全実装に至らない(118B 等の placeholder)

### 可能な次手(未試行)

1. **max_iterations を 40 → 80** へ倍増 — agent の大きな Write が cut-off される run で救済可能性
2. **SubAgent delegation** — main agent が scaffold + logic を終えた後、fresh context SubAgent に "page.tsx を実装しろ" を委譲(vibe-local の SubAgent 相当)
3. **完全 observation layer(L5)** — "page.tsx は現在 template" を毎 request で fact として注入
4. **user prompt 解析で UI 明示** — heavy + Next.js 検出時に user prompt に `"src/app/page.tsx にゲーム本体を実装"` を prepend

### 汎用性の限界

L2/L4 briefing 設計は:
- 拡張可能だが project type の事前列挙が必要
- 真の汎用化には sidecar による自動判定、あるいは user による `.anvil/project.yaml` 明示が必要
- vibe-local の platform briefing も 3 種限定(macOS/Linux/Windows)、同じ限界を持つ

## 推奨: 今後の作業方針

### 短期(次回 session で着手可能)

- **`max_iterations` のデフォルトを 40 → 80 に**(コード変更 1 行、コスト 0 円、heavy で cut-off 救済の可能性)
- **model 推奨を `qwen3.5:122b` に明記**(doc 09 の example を書換、model_registry tier に追加)

### 中期

- **Live observation layer の実装**(L2 の marker 検出 + L5 の template status 観測を合体)
- **SubAgent delegation** を heavy 条件限定で試験実装

### 長期(Phase F 以降)

- **`/api/chat` + `/api/generate` のハイブリッド**: tool schema の見せ方を動的切替(task 種別で endpoint を切替)
- **RAG** / **auto-test loop** を optional 機能として復活(Phase A scope 外で延期していた)
- **複数 project type 対応**(Rust CLI, Python, Vue 等)の観測 template 追加

## 原則の再確認(doc 01 との対比)

doc 01 の原則がほぼそのまま本日再確認された:

| doc 01 原則 | 本日の実証 |
|---|---|
| scaffold 後リカバリに複雑な状態機械を足し続けるのは悪手 | P3/U1-U4 で再現、全 revert |
| "失敗後の矯正" より "失敗しにくい形" | L4/L8 として確立 |
| ローカルLLM向けに多機能すぎるのは避ける | tool catalog を 6 に絞ったままで正解 |
| protocol を複数抱えない | N1' で XML fallback 専用化、成功 |
| 弱いモデルに後段リカバリを重ねても成功率は上がらない | retry gate 系 6 件失敗で再確認 |

## 結論

**2 日間の最大の学び**:

> prompt / state machine / retry gate といった "内側の工夫" より、**endpoint / model / system prompt 本体** という "外側の大きな変数" の方が local LLM の挙動を支配する。

次回以降の改修は、まず **外側の変数を動かせないか** を検討してから内側の工夫に入ること。
